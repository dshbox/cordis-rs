//! Patch algebra for entry lists: the composition mechanism behind bundles
//! and profiles.
//!
//! A *patch list* is a bare top-level YAML array of [`PatchOptions`] rows.
//! Each row either inserts entries (`insert`) or overrides an existing entry
//! by `id`. [`apply_entry_patches`] is THE patch semantics — the one routine
//! every consumer (mounting, recomposition, offline config dumps) funnels
//! through, so a dump can never drift from what boots.
//!
//! Two contracts are load-bearing:
//!
//! - **Detachment.** Inputs are never modified and the result shares nothing
//!   with them — even with no patches the returned list is a fresh copy.
//!   Recomposition must always restart from the original patch data; feeding
//!   a materialized composition back in would bake earlier patches into the
//!   base and make a removed or changed patch impossible to revert.
//! - **Single flatten.** Layer lists ([`compose_layers`],
//!   [`compose_with_provenance`]) are flattened into ONE
//!   [`apply_entry_patches`] call — the same single call a boot makes, not
//!   one call per layer. The single-pass id index is built once (base rows
//!   plus inserted rows as the same pass adds them) and never sees rows a
//!   plain `config` replacement introduced inside a group; a per-layer
//!   composition would rebuild the index between layers and let later layers
//!   patch rows boot never mounts.
//!
//! Patch-file IO follows the fail-loud contract: a *named* overlay
//! ([`load_overlay_patches`]) must exist — its absence is a
//! misconfiguration — while an *optional* user layer
//! ([`load_optional_patches`]) treats a missing file as "no layer". A
//! present-but-broken file (unreadable, unparsable, not a top-level array,
//! an entry that is not a mapping) always fails loud: a patch file that
//! cannot apply must never be silently skipped.

use crate::error::{IncludeError, Result};
use crate::node::Node;
use crate::options::{Disabled, EntryOptions, GROUP_NAME};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// One patch row: insert entries, or override an existing entry by id.
///
/// Every field is optional; rows are struct-typed where upstream's JS patches
/// could carry any field. Unknown keys are kept in [`PatchOptions::extra`]
/// (round-trip and diagnostics) and warn-skipped at application time.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PatchOptions {
    /// Target entry id. With `insert` it names the group to insert into;
    /// without `insert` it selects the entry to override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Entries to insert: appended to the target group's children when `id`
    /// names a group, to the top level otherwise. `Some(vec![])` still takes
    /// the insert branch (upstream truthiness); only `None` means "not an
    /// insert".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insert: Option<Vec<EntryOptions>>,
    /// Guard on the target: when present and non-empty it must equal the
    /// target's `name` or the patch warns and skips. It is never written
    /// back — a guard, not an override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whole replacement of the target's `config` (a replacement, not a
    /// merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Node>,
    /// Replacement of the target's `disabled` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// Replacement of the target's `inject` list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<Vec<String>>,
    /// Unknown keys, preserved in file order. Applying a patch warns and
    /// skips them: upstream JS patches could override any field, but the
    /// Rust entry has no slot for them (`group` is the children list, not
    /// upstream's marker; `intercept`/`isolate` are not ported).
    #[serde(flatten)]
    pub extra: IndexMap<String, Node>,
}

/// Whether an entry is a group.
///
/// Upstream marks groups with an explicit `group: true` flag; this port
/// infers them structurally: the built-in group plugin by name, or any entry
/// with children. The name arm keeps inserts working on a declared group
/// that momentarily has no rows (upstream's `target.config = []` branch).
fn is_group(entry: &EntryOptions) -> bool {
    entry.name == GROUP_NAME || !entry.group.is_empty()
}

/// Child positions leading from the root list to one entry.
type EntryPath = Vec<usize>;

/// Index one entry (and, for groups, its subtree) at `path`.
fn index_entry(entry: &EntryOptions, path: &[usize], index: &mut HashMap<String, EntryPath>) {
    if let Some(id) = entry.id.as_deref().filter(|id| !id.is_empty()) {
        index.insert(id.to_owned(), path.to_vec());
    }
    if is_group(entry) {
        for (position, child) in entry.group.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(position);
            index_entry(child, &child_path, index);
        }
    }
}

/// Borrow the entry a path addresses. Paths are built from the live
/// structure and never outlive the mutations that created them.
fn resolve<'a>(data: &'a [EntryOptions], path: &[usize]) -> &'a EntryOptions {
    let mut entry = &data[path[0]];
    for &position in &path[1..] {
        entry = &entry.group[position];
    }
    entry
}

/// Mutably borrow the entry a path addresses.
fn resolve_mut<'a>(data: &'a mut [EntryOptions], path: &[usize]) -> &'a mut EntryOptions {
    let mut entry = &mut data[path[0]];
    for &position in &path[1..] {
        entry = &mut entry.group[position];
    }
    entry
}

/// Warn about inserted rows without ids: they cannot be matched by later
/// layers and are deleted and recreated (fiber restart) on every
/// recomposition.
fn warn_unindexed(entries: &[EntryOptions], warn: &mut impl FnMut(&str)) {
    for entry in entries {
        if entry.id.as_deref().is_none_or(str::is_empty) {
            warn(
                "patch insert: entry has no id; later layers cannot patch it and it restarts on every recomposition",
            );
        }
        warn_unindexed(&entry.group, warn);
    }
}

/// Apply patch rows to an entry list — THE patch semantics of this crate,
/// shared by mounting, recomposition, and offline config tooling so a dump
/// can never drift from what boots.
///
/// Semantics (a direct port of upstream `applyEntryPatches`):
///
/// - The result is fully detached from both inputs, even when `patches` is
///   empty. Recomposition must always restart from the original patch data.
/// - The id index is built once from `data` (recursing into groups) and
///   extended only by `insert` rows as they are added, so a later patch in
///   the same list can target a row an earlier patch inserted — and rows
///   that appear any other way (inside a replaced `config`, say) stay
///   invisible to it.
/// - `insert` with `id` appends to that group's children (the target must
///   be a group); `insert` without `id` appends to the top level.
/// - Non-insert patches require `id`. `name`, when present and non-empty,
///   must equal the target's name. `config`, `disabled`, and `inject`
///   replace the target's fields wholesale.
/// - A patch that matches nothing — missing id, unknown target, name
///   mismatch, non-group insert target — warns through `warn` and is
///   skipped, never an error: one overlay shared across surfaces does not
///   have to match every tree.
/// - Override keys with no Rust slot (`group`, `intercept`, `isolate`,
///   anything unknown) warn and are skipped.
///
/// # Example
///
/// ```
/// use cordis_include::{apply_entry_patches, EntryOptions, Node, PatchOptions};
///
/// let base = vec![EntryOptions::new("adapter-http").with_id("http")];
/// let patches = vec![PatchOptions {
///     id: Some("http".into()),
///     config: Some(Node::from_iter([(
///         "port".to_string(),
///         Node::Int(8080),
///     )])),
///     ..Default::default()
/// }];
/// let composed = apply_entry_patches(&base, &patches, |_| {});
/// assert_eq!(composed[0].config.as_ref().unwrap()["port"], Node::Int(8080));
/// // The input stays untouched: recomposition restarts from the originals.
/// assert!(base[0].config.is_none());
/// ```
pub fn apply_entry_patches(
    data: &[EntryOptions],
    patches: &[PatchOptions],
    mut warn: impl FnMut(&str),
) -> Vec<EntryOptions> {
    let mut data = data.to_vec();
    if patches.is_empty() {
        return data;
    }
    let mut index: HashMap<String, EntryPath> = HashMap::new();
    for (position, entry) in data.iter().enumerate() {
        index_entry(entry, &[position], &mut index);
    }
    for patch in patches {
        // `Some(..)` takes the insert branch even for an empty list —
        // upstream's truthiness check on the insert field.
        if let Some(insert) = patch.insert.as_ref() {
            warn_unindexed(insert, &mut warn);
            match patch.id.as_deref().filter(|id| !id.is_empty()) {
                Some(id) => {
                    let Some(path) = index.get(id) else {
                        warn(&format!("patch insert: entry {id:?} not found"));
                        continue;
                    };
                    let path = path.clone();
                    let target = resolve(&data, &path);
                    if !is_group(target) {
                        warn(&format!("patch insert: entry {id:?} is not a group"));
                        continue;
                    }
                    let start = target.group.len();
                    resolve_mut(&mut data, &path)
                        .group
                        .extend(insert.iter().cloned());
                    for (offset, entry) in insert.iter().enumerate() {
                        let mut child_path = path.clone();
                        child_path.push(start + offset);
                        index_entry(entry, &child_path, &mut index);
                    }
                }
                None => {
                    let start = data.len();
                    data.extend(insert.iter().cloned());
                    for (offset, entry) in insert.iter().enumerate() {
                        index_entry(entry, &[start + offset], &mut index);
                    }
                }
            }
            continue;
        }

        let Some(id) = patch.id.as_deref().filter(|id| !id.is_empty()) else {
            warn("patch: id is required for non-insert patches");
            continue;
        };
        let Some(path) = index.get(id) else {
            warn(&format!("patch: entry {id:?} not found"));
            continue;
        };
        let path = path.clone();
        let target_name = resolve(&data, &path).name.clone();
        if let Some(name) = patch.name.as_deref().filter(|name| !name.is_empty()) {
            if name != target_name {
                warn(&format!(
                    "patch: name mismatch for {id:?} (expected {target_name:?}, got {name:?}), skipping"
                ));
                continue;
            }
        }
        let target = resolve_mut(&mut data, &path);
        if let Some(config) = patch.config.as_ref() {
            target.config = Some(config.clone());
        }
        if let Some(disabled) = patch.disabled {
            // Patches carry an already-evaluated boolean; they overwrite
            // any expression the target declared (later activation
            // re-evaluates nothing).
            target.disabled = Disabled::Flag(disabled);
        }
        if let Some(inject) = patch.inject.as_ref() {
            target.inject = inject.clone();
        }
        if !patch.extra.is_empty() {
            let keys: Vec<&str> = patch.extra.keys().map(String::as_str).collect();
            warn(&format!(
                "patch: skipping unsupported override key(s) [{}] on entry {id:?}",
                keys.join(", ")
            ));
        }
    }
    data
}

/// Compose patch layers into the effective entry list over an empty root.
///
/// **All layers flatten into a single [`apply_entry_patches`] call** — the
/// same single call a boot makes, never one call per layer. The single-pass
/// id index never sees rows a plain `config` replacement introduced inside a
/// group, so a later layer targeting such a row warns and misses; composing
/// layer-by-layer would rebuild the index between layers and produce a tree
/// boot never mounts. Composers must keep this shape.
///
/// # Example
///
/// ```
/// use cordis_include::{compose_layers, EntryOptions, PatchOptions};
///
/// let bundle = vec![PatchOptions {
///     insert: Some(vec![EntryOptions::new("adapter-http").with_id("http")]),
///     ..Default::default()
/// }];
/// let user = vec![PatchOptions {
///     id: Some("http".into()),
///     disabled: Some(true),
///     ..Default::default()
/// }];
/// let entries = compose_layers(&[bundle, user], |_| {});
/// assert_eq!(entries.len(), 1);
/// assert!(entries[0].disabled.is_disabled());
/// ```
pub fn compose_layers(layers: &[Vec<PatchOptions>], warn: impl FnMut(&str)) -> Vec<EntryOptions> {
    let flattened: Vec<PatchOptions> = layers.iter().flatten().cloned().collect();
    apply_entry_patches(&[], &flattened, warn)
}

/// One labeled patch layer, for provenance-aware composition and dumps.
#[derive(Debug, Clone, Copy)]
pub struct DumpLayer<'a> {
    /// Source label shown in dump comments and warning attributions
    /// (a file basename or path).
    pub label: &'a str,
    /// The layer's patches, in application order.
    pub patches: &'a [PatchOptions],
}

/// Where one composed row came from: its origin layer and every later layer
/// that changed it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Provenance {
    /// The label of the layer that contributed the row.
    pub origin: String,
    /// Labels of the layers that patched it, in application order.
    pub patched_by: Vec<String>,
}

/// Compose layers exactly as [`compose_layers`] does (single flatten over
/// `base`) while tracking, per row, which layer contributed it and which
/// layers changed it.
///
/// Provenance uses upstream's prefix-snapshot diff: snapshot *k* applies
/// layers 1..*k* over the same base, and a row is compared positionally
/// against the previous snapshot — the patch algorithm only rewrites rows in
/// place or appends, so a top-level index identifies one row across
/// snapshots, and a layer whose addition changed the row (config
/// replacement, disable, group insert) is listed as having patched it.
/// Appended rows carry the appending layer as their origin.
///
/// Skipped-patch warnings are attributed to layers the same way: earlier
/// layers' patches see an identical preceding state in every snapshot that
/// includes them, so each snapshot's warning list extends the previous one
/// and the new tail belongs to the added layer. `warn` receives each tail
/// line prefixed with `[label]`.
///
/// Patches are never mutated (application clones into the result), so the
/// same layer slices are safely reused across snapshots.
pub fn compose_with_provenance(
    base: &[EntryOptions],
    base_label: &str,
    layers: &[DumpLayer<'_>],
    mut warn: impl FnMut(&str),
) -> (Vec<EntryOptions>, Vec<Provenance>) {
    let mut provenance = vec![
        Provenance {
            origin: base_label.to_owned(),
            patched_by: Vec::new(),
        };
        base.len()
    ];
    let mut previous = base.to_vec();
    let mut previous_warnings: Vec<String> = Vec::new();
    for (count, layer) in layers.iter().enumerate() {
        let flattened: Vec<PatchOptions> = layers[..=count]
            .iter()
            .flat_map(|layer| layer.patches.iter().cloned())
            .collect();
        let mut warnings: Vec<String> = Vec::new();
        let snapshot = apply_entry_patches(base, &flattened, |line| warnings.push(line.to_owned()));
        for line in warnings.iter().skip(previous_warnings.len()) {
            warn(&format!("[{}] {}", layer.label, line));
        }
        for (position, entry) in snapshot.iter().enumerate() {
            if position >= previous.len() {
                provenance.push(Provenance {
                    origin: layer.label.to_owned(),
                    patched_by: Vec::new(),
                });
            } else if entry != &previous[position] {
                provenance[position].patched_by.push(layer.label.to_owned());
            }
        }
        previous = snapshot;
        previous_warnings = warnings;
    }
    (previous, provenance)
}

/// Render composed rows as one loadable YAML document, grouped under a
/// `# == origin[, patched by …]` comment per contiguous run of rows with the
/// same source. `${{ env.NAME }}` templates print verbatim, unevaluated.
///
/// Entry fields serialize in the crate's stable order
/// (`id`, `name`, `disabled`, `inject`, `group`, `config`); config leaves
/// are delegated to the serde YAML emitter.
pub fn render_dump(composed: &[EntryOptions], provenance: &[Provenance]) -> Result<String> {
    let mut sections: Vec<String> = Vec::new();
    let mut group: Vec<&EntryOptions> = Vec::new();
    let mut current: Option<String> = None;
    for (entry, record) in composed.iter().zip(provenance) {
        let label = if record.patched_by.is_empty() {
            record.origin.clone()
        } else {
            format!(
                "{}, patched by {}",
                record.origin,
                record.patched_by.join(", ")
            )
        };
        if current.as_deref() != Some(label.as_str()) {
            flush_group(&mut sections, &group, current.take())?;
            current = Some(label);
            group.clear();
        }
        group.push(entry);
    }
    flush_group(&mut sections, &group, current)?;
    Ok(sections.join("\n") + "\n")
}

/// Serialize one contiguous group under its label comment.
fn flush_group(
    sections: &mut Vec<String>,
    group: &[&EntryOptions],
    label: Option<String>,
) -> Result<()> {
    if group.is_empty() {
        return Ok(());
    }
    let label = label.expect("a non-empty group always has a label");
    let rows: Vec<EntryOptions> = group.iter().map(|entry| (*entry).clone()).collect();
    let text = crate::yaml::emit_entry_list(&rows);
    sections.push(format!("# == {label}\n{}", text.trim_end()));
    Ok(())
}

/// Compose layers over `base` and render the dump in one step — the offline
/// twin of [`compose_with_provenance`] plus [`render_dump`]. See those for
/// the single-flatten, provenance, and warning-attribution contracts.
///
/// # Example
///
/// ```
/// use cordis_include::{render_config_dump, DumpLayer, EntryOptions, PatchOptions};
///
/// let base = [EntryOptions::new("./noop").with_id("shared")];
/// let overlay = [PatchOptions {
///     id: Some("shared".into()),
///     disabled: Some(true),
///     ..Default::default()
/// }];
/// let layers = [DumpLayer {
///     label: "overlay.yml",
///     patches: &overlay,
/// }];
/// let dump = render_config_dump(&base, "base.yml", &layers, |_| {}).unwrap();
/// assert!(dump.contains("# == base.yml, patched by overlay.yml"), "{dump}");
/// ```
pub fn render_config_dump(
    base: &[EntryOptions],
    base_label: &str,
    layers: &[DumpLayer<'_>],
    warn: impl FnMut(&str),
) -> Result<String> {
    let (composed, provenance) = compose_with_provenance(base, base_label, layers, warn);
    render_dump(&composed, &provenance)
}

/// Load an optional patch-list file: a top-level YAML array of patch rows.
/// A missing file means "no layer" (`Ok(None)`); any other read failure, a
/// parse failure, a non-array document, or a non-mapping entry is a hard
/// error — a present patch file that cannot apply is a misconfiguration and
/// must fail loud, never be silently skipped.
pub fn load_optional_patches(path: impl AsRef<Path>) -> Result<Option<Vec<PatchOptions>>> {
    let path = path.as_ref();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(IncludeError::Message {
                message: format!("failed to read patches {}: {error}", path.display()),
            });
        }
    };
    Ok(Some(parse_patch_list(path, &content, "patches")?))
}

/// Load a required overlay patch list — a bundle's `cordis.patch.yml` or a
/// `--patch` overlay. Same file format as [`load_optional_patches`], but a
/// missing file is a hard error: the caller *named* this file, so its
/// absence is a misconfiguration, not "no overlay".
pub fn load_overlay_patches(path: impl AsRef<Path>) -> Result<Vec<PatchOptions>> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|error| IncludeError::Message {
        message: format!("failed to read overlay {}: {error}", path.display()),
    })?;
    parse_patch_list(path, &content, "overlay")
}

/// Parse one patch list: a bare top-level YAML array of [`PatchOptions`]
/// rows, each a mapping.
fn parse_patch_list(path: &Path, content: &str, label: &str) -> Result<Vec<PatchOptions>> {
    let node = crate::yaml::parse_node(content).map_err(|error| IncludeError::Message {
        message: format!("failed to parse {label} {}: {error}", path.display()),
    })?;
    let Some(rows) = node.as_array() else {
        return Err(IncludeError::Message {
            message: format!(
                "{label} {} must be a top-level YAML array of loader patch entries",
                path.display()
            ),
        });
    };
    let mut patches = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        if row.as_object().is_none() {
            return Err(IncludeError::Message {
                message: format!(
                    "{label} entry {} in {} must be a mapping (a loader patch entry)",
                    index + 1,
                    path.display()
                ),
            });
        }
        let patch =
            crate::yaml::patch_from_node(row.clone()).map_err(|error| IncludeError::Message {
                message: format!(
                    "failed to parse {label} entry {} in {}: {error}",
                    index + 1,
                    path.display()
                ),
            })?;
        patches.push(patch);
    }
    Ok(patches)
}
