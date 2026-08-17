//! The entry tree: id scheme, structural edits, and whole-tree diffs.

use crate::entry::Entry;
use crate::error::{IncludeError, Result};
use crate::options::EntryOptions;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// What changed in one [`EntryTree::update`] pass.
///
/// The loader consumes this to start/stop/patch fibers without touching
/// entries whose id, options, and position are unchanged.
#[derive(Debug, Default, Clone)]
pub struct TreeDiff {
    /// Entries that appeared in the new data (subtree roots first).
    pub created: Vec<Entry>,
    /// Entries still present whose options changed.
    pub updated: Vec<Entry>,
    /// Entries that moved to a different parent.
    pub moved: Vec<Entry>,
    /// Entries whose ids vanished from the new data (whole subtrees).
    pub removed: Vec<Entry>,
}

impl TreeDiff {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.updated.is_empty()
            && self.moved.is_empty()
            && self.removed.is_empty()
    }
}

/// An in-memory tree of [`Entry`]s mirroring one config file.
///
/// Entries are addressed by id; nested entries use composite ids
/// (`outer:inner`, see [`Entry::path`]). Entries without an explicit id get
/// a random 6-character base36 id that is persisted on the next write-back.
///
/// Structural edits are serialized through an internal lock; individual
/// reads (children, parent walks) take per-entry locks. `EntryTree` is not
/// bound to any file — pairing it with a [`crate::LoaderFile`] is the
/// caller's (usually the loader's) job.
pub struct EntryTree {
    root: Entry,
    mutation: std::sync::Mutex<()>,
}

impl Default for EntryTree {
    fn default() -> Self {
        Self::new()
    }
}

impl EntryTree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self {
            root: Entry::new_root(),
            mutation: std::sync::Mutex::new(()),
        }
    }

    /// The synthetic root holding the top-level entries. It is addressed by
    /// the empty id and never returned by [`EntryTree::resolve`].
    pub fn root(&self) -> &Entry {
        &self.root
    }

    /// Top-level entries in file order.
    pub fn top_level(&self) -> Vec<Entry> {
        self.root.children()
    }

    /// All entries in depth-first order, parents before children.
    pub fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        fn walk(entry: &Entry, out: &mut Vec<Entry>) {
            for child in entry.children() {
                out.push(child.clone());
                walk(&child, out);
            }
        }
        walk(&self.root, &mut out);
        out
    }

    /// Look an entry up by (possibly composite) id, e.g. `group1:child2`.
    pub fn resolve(&self, id: &str) -> Option<Entry> {
        let mut current = self.root.clone();
        for part in id.split(':') {
            current = current
                .children()
                .into_iter()
                .find(|child| child.id() == part)?;
        }
        Some(current)
    }

    /// Serialize the whole tree back to options, generated ids included.
    pub fn serialize(&self) -> Vec<EntryOptions> {
        fn to_options(entry: &Entry) -> EntryOptions {
            let mut options = entry.options();
            options.group = entry.children().iter().map(to_options).collect();
            options
        }
        self.root.children().iter().map(to_options).collect()
    }

    /// Create an entry below `parent` (the tree root when `None`) at
    /// `position` (appended when `None`). `options.group` seeds the child
    /// list for group entries. Returns the created entry.
    pub fn create(
        &self,
        options: EntryOptions,
        parent: Option<&Entry>,
        position: Option<usize>,
    ) -> Result<Entry> {
        let _guard = crate::lock(&self.mutation);
        let parent = parent.unwrap_or(&self.root);
        self.assert_owned(parent)?;
        if options.name.is_empty() {
            return Err(IncludeError::InvalidName);
        }
        let reserved: HashSet<String> = self.ids();
        validate_subtree(
            std::slice::from_ref(&options),
            &reserved,
            &mut HashSet::new(),
        )?;

        let mut options = options;
        let id = match options.id.take() {
            Some(id) => id,
            None => generate_id(&reserved, &HashMap::new()),
        };
        let group = std::mem::take(&mut options.group);
        options.id = Some(id.clone());
        let entry = Entry::new(id, options);
        let children = sync_children(
            &entry,
            group,
            &mut HashMap::new(),
            &reserved,
            &mut TreeDiff::default(),
        );
        entry.set_children(children);
        insert_child(parent, entry.clone(), position);
        Ok(entry)
    }

    /// Detach the entry with the given id and return it with its subtree
    /// intact, so the loader can still stop the fibers inside it.
    pub fn remove(&self, id: &str) -> Result<Entry> {
        let _guard = crate::lock(&self.mutation);
        let entry = self
            .resolve(id)
            .ok_or_else(|| IncludeError::EntryNotFound { id: id.to_owned() })?;
        let parent = entry
            .parent()
            .ok_or_else(|| IncludeError::EntryNotFound { id: id.to_owned() })?;
        detach_child(&parent, &entry);
        Ok(entry)
    }

    /// Update one entry's options and optionally move it below
    /// `new_parent` (appended unless `position` is given). The entry id is
    /// identity and survives the update; `options.id` is ignored.
    /// `options.group` re-syncs the entry's children, reusing existing
    /// subtree entries whose ids match.
    pub fn update_entry(
        &self,
        id: &str,
        options: EntryOptions,
        new_parent: Option<&Entry>,
        position: Option<usize>,
    ) -> Result<Entry> {
        let _guard = crate::lock(&self.mutation);
        let entry = self
            .resolve(id)
            .ok_or_else(|| IncludeError::EntryNotFound { id: id.to_owned() })?;
        let old_parent = entry
            .parent()
            .ok_or_else(|| IncludeError::EntryNotFound { id: id.to_owned() })?;
        let parent = match new_parent {
            Some(parent) => {
                self.assert_owned(parent)?;
                if entry.contains(parent) {
                    return Err(IncludeError::Cycle);
                }
                parent.clone()
            }
            None => old_parent.clone(),
        };
        if options.name.is_empty() {
            return Err(IncludeError::InvalidName);
        }

        // Existing subtree entries stay reusable by id; everything else in
        // the tree is reserved.
        let subtree = descendants(&entry);
        let mut pool: HashMap<String, Entry> = subtree
            .iter()
            .map(|child| (child.id().to_string(), child.clone()))
            .collect();
        let mut reserved = self.ids();
        for key in pool.keys() {
            reserved.remove(key);
        }
        validate_subtree(
            std::slice::from_ref(&options),
            &reserved,
            &mut HashSet::new(),
        )?;

        let mut options = options;
        options.id = Some(entry.id().to_owned());
        let group = std::mem::take(&mut options.group);
        entry.set_options(options);

        // Detach first so a cross-group move never leaves the entry in two
        // sibling lists at once.
        detach_child(&old_parent, &entry);
        let children = sync_children(
            &entry,
            group,
            &mut pool,
            &reserved,
            &mut TreeDiff::default(),
        );
        entry.set_children(children);
        insert_child(&parent, entry.clone(), position);
        Ok(entry)
    }

    /// Reload the whole tree from new options, reusing existing entries
    /// wherever ids match — including entries that moved between groups —
    /// and returning what changed.
    ///
    /// Entries in the new data without an id cannot be matched and are
    /// always created fresh; persist generated ids by writing
    /// [`EntryTree::serialize`] back to the file.
    pub fn update(&self, entries: Vec<EntryOptions>) -> Result<TreeDiff> {
        let _guard = crate::lock(&self.mutation);
        // Existing ids are reusable here, so nothing is reserved; only
        // duplicates within the incoming data are rejected.
        validate_subtree(&entries, &HashSet::new(), &mut HashSet::new())?;

        // Index every existing entry by id so matches work across groups.
        let mut pool: HashMap<String, Entry> = self
            .entries()
            .into_iter()
            .map(|entry| (entry.id().to_string(), entry))
            .collect();
        let reserved = HashSet::new();
        let mut diff = TreeDiff::default();
        let children = sync_children(&self.root, entries, &mut pool, &reserved, &mut diff);
        self.root.set_children(children);
        diff.removed = pool.into_values().collect();
        Ok(diff)
    }

    /// All entry ids currently in the tree.
    fn ids(&self) -> HashSet<String> {
        self.entries().iter().map(|e| e.id().to_string()).collect()
    }

    /// Fail unless `candidate` is the root of, or lives inside, this tree.
    fn assert_owned(&self, candidate: &Entry) -> Result<()> {
        let mut current = candidate.clone();
        loop {
            if Entry::ptr_eq(&current, &self.root) {
                return Ok(());
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => return Err(IncludeError::NotInTree),
            }
        }
    }
}

/// Build the new child list for `parent` from `options`, taking reusable
/// entries out of `pool` (leftovers become `removed`) and reporting
/// mutations through `diff`. Infallible: callers pre-validate ids.
fn sync_children(
    parent: &Entry,
    options: Vec<EntryOptions>,
    pool: &mut HashMap<String, Entry>,
    reserved: &HashSet<String>,
    diff: &mut TreeDiff,
) -> Vec<Entry> {
    let mut result = Vec::with_capacity(options.len());
    for options in options {
        let mut options = options;
        let id = match options.id.take() {
            Some(id) => id,
            None => generate_id(reserved, pool),
        };
        options.id = Some(id.clone());
        let group = std::mem::take(&mut options.group);
        let entry = match pool.remove(&id) {
            Some(existing) => {
                if existing.options() != options {
                    existing.set_options(options);
                    diff.updated.push(existing.clone());
                }
                let moved = existing
                    .parent()
                    .is_none_or(|old| !Entry::ptr_eq(&old, parent));
                if moved {
                    diff.moved.push(existing.clone());
                }
                existing
            }
            None => {
                let created = Entry::new(id, options);
                diff.created.push(created.clone());
                created
            }
        };
        let children = sync_children(&entry, group, pool, reserved, diff);
        entry.set_children(children);
        result.push(entry);
    }
    result
}

/// All entries strictly below `entry`, depth-first.
fn descendants(entry: &Entry) -> Vec<Entry> {
    let mut out = Vec::new();
    fn walk(entry: &Entry, out: &mut Vec<Entry>) {
        for child in entry.children() {
            out.push(child.clone());
            walk(&child, out);
        }
    }
    walk(entry, &mut out);
    out
}

/// Insert `child` into `parent`'s child list at `position` (append when
/// `None`, clamped to the ends).
fn insert_child(parent: &Entry, child: Entry, position: Option<usize>) {
    let mut siblings = parent.children();
    let index = position.unwrap_or(siblings.len()).min(siblings.len());
    siblings.insert(index, child);
    parent.set_children(siblings);
}

/// Remove `child` from `parent`'s child list, leaving the child's own
/// subtree intact and clearing its parent link.
fn detach_child(parent: &Entry, child: &Entry) {
    let kept: Vec<Entry> = parent
        .children()
        .into_iter()
        .filter(|kept| !Entry::ptr_eq(kept, child))
        .collect();
    parent.set_children(kept);
}

/// Reject empty ids and the `:` path separator.
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains(':') {
        return Err(IncludeError::InvalidId { id: id.to_owned() });
    }
    Ok(())
}

/// Pre-validate an incoming options tree before any mutation: non-empty
/// names, well-formed ids, no duplicate explicit ids within the data, and
/// no collision with `reserved` (ids outside the reusable pool).
fn validate_subtree(
    entries: &[EntryOptions],
    reserved: &HashSet<String>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    for options in entries {
        if options.name.is_empty() {
            return Err(IncludeError::InvalidName);
        }
        if let Some(id) = options.id.as_deref() {
            validate_id(id)?;
            if reserved.contains(id) {
                return Err(IncludeError::DuplicateId { id: id.to_owned() });
            }
            if !seen.insert(id.to_owned()) {
                return Err(IncludeError::DuplicateId { id: id.to_owned() });
            }
        }
        validate_subtree(&options.group, reserved, seen)?;
    }
    Ok(())
}

/// Generate a random 6-character base36 id avoiding `reserved` ids and
/// anything still pooled.
fn generate_id(reserved: &HashSet<String>, pool: &HashMap<String, Entry>) -> String {
    loop {
        let candidate = random_base36_6();
        if !reserved.contains(&candidate) && !pool.contains_key(&candidate) {
            return candidate;
        }
    }
}

/// Six base36 characters from a time/counter-seeded splitmix64 stream.
/// Uniqueness matters, unpredictability does not.
fn random_base36_6() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut z = nanos ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;

    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut value = z % 2_176_782_336; // 36^6
    let mut out = [0u8; 6];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_six_base36_chars() {
        for _ in 0..100 {
            let id = random_base36_6();
            assert_eq!(id.len(), 6, "{id}");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
            );
        }
    }

    #[test]
    fn generated_ids_avoid_collisions() {
        let first = random_base36_6();
        let reserved: HashSet<String> = [first.clone()].into_iter().collect();
        assert_ne!(generate_id(&reserved, &HashMap::new()), first);
    }
}
