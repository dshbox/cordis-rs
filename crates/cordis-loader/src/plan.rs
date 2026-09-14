//! Immutable Loader plan construction and serialized source schema.
//!
//! A [`LoadPlanBuilder`] owns one plan lineage. Successful additions return
//! opaque [`EntryId`] correlations. [`LoadPlanBuilder::finish`] validates and
//! freezes the declarations into a cloneable [`LoadPlan`] without exposing
//! backing topology, navigation, or mutation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

struct Lineage;
struct Occurrence;

/// Opaque correlation identity for one declarative entry in one plan lineage.
///
/// Cloning preserves identity. IDs returned by one builder survive finish and
/// [`LoadPlan`] clones. Independently rebuilding equal source creates a fresh
/// lineage and unequal IDs. `EntryId` has no public constructor, number,
/// ordering, display, serialization, path, index, lookup, or control contract.
#[derive(Clone)]
pub struct EntryId {
    lineage: Arc<Lineage>,
    occurrence: Arc<Occurrence>,
}

impl EntryId {
    fn new(lineage: &Arc<Lineage>) -> Self {
        Self {
            lineage: Arc::clone(lineage),
            occurrence: Arc::new(Occurrence),
        }
    }

    fn belongs_to(&self, lineage: &Arc<Lineage>) -> bool {
        Arc::ptr_eq(&self.lineage, lineage)
    }
}

impl fmt::Debug for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EntryId(..)")
    }
}

impl PartialEq for EntryId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.lineage, &other.lineage)
            && Arc::ptr_eq(&self.occurrence, &other.occurrence)
    }
}
impl Eq for EntryId {}
impl Hash for EntryId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.lineage), state);
        std::ptr::hash(Arc::as_ptr(&self.occurrence), state);
    }
}

/// Mutable serialized source for one Plugin declaration.
///
/// `config` is required in the wire form. Unit configuration is JSON `null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    /// Explicit Plugin resolution key, preferred over [`PluginEntry::name`].
    pub key: Option<String>,
    /// Optional human-readable name and resolve-key fallback.
    pub name: Option<String>,
    /// Required JSON source configuration.
    pub config: Value,
    /// Whether this Plugin declaration disables its execution subtree.
    #[serde(default)]
    pub disabled: bool,
    /// Declarative Service dependencies.
    #[serde(default)]
    pub inject: Vec<InjectEntry>,
    /// Declarative Service placement policy.
    #[serde(default)]
    pub isolate: Vec<IsolateEntry>,
}

/// Mutable serialized source for one structural sequencing group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryGroup {
    /// Human-readable source-only group name.
    ///
    /// Groups are structural sequencing syntax. The name is accepted for
    /// configuration readability but is not retained in the frozen plan or
    /// surfaced in execution outcomes.
    pub name: String,
}

/// One declarative Service dependency in a [`PluginEntry`].
///
/// Wire syntax is explicitly tagged as `required` or `configured` and does not
/// depend on Rust enum layout.
#[derive(Debug, Clone)]
pub enum InjectEntry {
    /// Require a Service by semantic name.
    Required(String),
    /// Require a Service with JSON configuration for resolver preparation.
    Configured {
        /// Semantic Service name.
        service: String,
        /// JSON source configuration.
        config: Value,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum InjectEntryRef<'a> {
    Required { service: &'a str },
    Configured { service: &'a str, config: &'a Value },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum InjectEntryWire {
    Required { service: String },
    Configured { service: String, config: Value },
}

impl Serialize for InjectEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Required(service) => InjectEntryRef::Required { service }.serialize(serializer),
            Self::Configured { service, config } => {
                InjectEntryRef::Configured { service, config }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for InjectEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match InjectEntryWire::deserialize(deserializer)? {
            InjectEntryWire::Required { service } => Self::Required(service),
            InjectEntryWire::Configured { service, config } => Self::Configured { service, config },
        })
    }
}

impl InjectEntry {
    fn service(&self) -> &str {
        match self {
            Self::Required(service) | Self::Configured { service, .. } => service,
        }
    }
}

/// One declarative Service realm-selection row in a [`PluginEntry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolateEntry {
    /// Semantic Service name whose placement this row selects.
    pub service: String,
    /// Loader-local realm policy interpreted independently for each execution.
    pub policy: RealmPolicy,
}

/// Loader-local declarative Service realm policy.
///
/// Wire syntax is explicitly tagged; shared labels rendezvous only within one
/// future plan execution and are never core realm identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RealmPolicy {
    /// Select a fresh private realm when the plan is executed.
    Private,
    /// Rendezvous by label only within one future plan execution.
    Shared {
        /// Declarative per-execution rendezvous label.
        label: String,
    },
}

/// Validation failure while constructing an immutable [`LoadPlan`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlanError {
    /// A parent is not an already-admitted parent-capable entry in this lineage.
    #[error("parent {parent:?} is not an admitted entry in this plan lineage")]
    ForeignParent {
        /// Rejected parent correlation identity.
        parent: EntryId,
    },
    /// A Plugin declaration has neither a key nor a name fallback.
    #[error("entry {entry:?} has no resolve identity")]
    MissingResolveIdentity {
        /// Rejected declaration identity.
        entry: EntryId,
    },
    /// One Plugin declaration names the same injected Service more than once.
    #[error("entry {entry:?} declares injected service `{service}` more than once")]
    DuplicateInjectService {
        /// Rejected declaration identity.
        entry: EntryId,
        /// Duplicated semantic Service name.
        service: String,
    },
    /// One Plugin declaration names the same isolated Service more than once.
    #[error("entry {entry:?} declares isolated service `{service}` more than once")]
    DuplicateIsolateService {
        /// Rejected declaration identity.
        entry: EntryId,
        /// Duplicated semantic Service name.
        service: String,
    },
}

#[derive(Clone)]
enum PlanEntry {
    Plugin(PluginEntry),
    Group,
}
impl PlanEntry {
    fn is_parent_capable(&self) -> bool {
        matches!(self, Self::Plugin(_) | Self::Group)
    }
}

#[derive(Clone)]
struct PlanNode {
    id: EntryId,
    parent: Option<EntryId>,
    entry: PlanEntry,
}

/// Builder for one validated plan lineage.
///
/// Construction starts a fresh lineage. Every parent must identify an
/// already-admitted, parent-capable declaration from this exact builder. Each
/// add validates before admission and is failure-atomic.
pub struct LoadPlanBuilder {
    lineage: Arc<Lineage>,
    nodes: Vec<PlanNode>,
}

impl LoadPlanBuilder {
    /// Start a fresh empty plan lineage.
    ///
    /// `new` is intentionally the sole frozen constructor; `Default` is not
    /// part of the public plan contract.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            lineage: Arc::new(Lineage),
            nodes: Vec::new(),
        }
    }

    /// Validate and admit one Plugin declaration.
    pub fn add_plugin(
        &mut self,
        parent: Option<&EntryId>,
        entry: PluginEntry,
    ) -> Result<EntryId, PlanError> {
        self.validate_parent(parent)?;
        let id = EntryId::new(&self.lineage);
        validate_plugin(&id, &entry)?;
        self.nodes.push(PlanNode {
            id: id.clone(),
            parent: parent.cloned(),
            entry: PlanEntry::Plugin(entry),
        });
        Ok(id)
    }

    /// Validate and admit one structural sequencing group.
    pub fn add_group(
        &mut self,
        parent: Option<&EntryId>,
        group: EntryGroup,
    ) -> Result<EntryId, PlanError> {
        self.validate_parent(parent)?;
        // Group names are source syntax; frozen group semantics are structural only.
        let _ = group;
        let id = EntryId::new(&self.lineage);
        self.nodes.push(PlanNode {
            id: id.clone(),
            parent: parent.cloned(),
            entry: PlanEntry::Group,
        });
        Ok(id)
    }

    /// Validate the complete lineage and freeze it without renumbering IDs.
    pub fn finish(self) -> Result<LoadPlan, PlanError> {
        validate_complete(&self.lineage, &self.nodes)?;
        Ok(LoadPlan {
            inner: Arc::new(LoadPlanInner { nodes: self.nodes }),
        })
    }

    fn validate_parent(&self, parent: Option<&EntryId>) -> Result<(), PlanError> {
        let Some(parent) = parent else {
            return Ok(());
        };
        let admitted = parent.belongs_to(&self.lineage)
            && self
                .nodes
                .iter()
                .any(|node| node.id == *parent && node.entry.is_parent_capable());
        if admitted {
            Ok(())
        } else {
            Err(PlanError::ForeignParent {
                parent: parent.clone(),
            })
        }
    }
}

/// One immutable, validated declarative Loader capability.
///
/// Cloning shares the same frozen plan lineage and preserves every `EntryId`.
/// Repeated execution preserves those correlations and semantic ordering while
/// realm identities, Fiber identities, Service publication occurrences, outcome
/// values, and Fork ownership are created fresh for that execution. Structural
/// parentage is private sequencing input only: `LoadPlan` exposes no entry
/// iterator, children, arbitrary enablement query, path/index lookup, mutation,
/// patch, or removal API.
#[derive(Clone)]
pub struct LoadPlan {
    inner: Arc<LoadPlanInner>,
}

// Frozen backing topology remains private; execution consumes it without
// publishing navigation or storage representation.
struct LoadPlanInner {
    nodes: Vec<PlanNode>,
}

/// Ephemeral realm-label interpretation for one `LoadPlan::load` call.
///
/// Text labels terminate here in Loader. Every stored realm is allocated from
/// the execution's caller-supplied Context, so the table can rendezvous only
/// rows from this execution and can never become a cross-load/core interner.
struct RealmEnvironment {
    shared: HashMap<String, cordis_core::ServiceRealm>,
}

impl RealmEnvironment {
    fn new() -> Self {
        Self {
            shared: HashMap::new(),
        }
    }

    fn place(
        &mut self,
        ctx: &cordis_core::Context,
        declarations: &[IsolateEntry],
    ) -> cordis_core::Context {
        if declarations.is_empty() {
            return ctx.clone();
        }

        let mappings = declarations.iter().map(|declaration| {
            let realm = match &declaration.policy {
                RealmPolicy::Private => ctx.new_service_realm(),
                RealmPolicy::Shared { label } => self
                    .shared
                    .entry(label.clone())
                    .or_insert_with(|| ctx.new_service_realm())
                    .clone(),
            };
            (declaration.service.clone(), realm)
        });

        // Plan validation already rejects duplicate Service rows, and every realm
        // above was allocated from this exact Context's Runtime. Both public
        // RealmMappingError causes are therefore impossible at this private seam.
        ctx.with_service_realms(mappings).expect(
            "validated Loader isolate declarations use unique services and execution-local realms",
        )
    }
}

impl LoadPlan {
    /// Execute this frozen plan partially and return one semantic outcome per entry.
    ///
    /// Traversal is deterministic depth-first plan order: every parent precedes
    /// all descendants and siblings retain declaration order. Disabled Plugin
    /// ancestry is execution-local pruning only and never mutates the frozen plan;
    /// ordinary resolver or spawn failure affects only its own row.
    ///
    /// Each call creates one ephemeral Loader realm environment. `Private`
    /// declarations allocate fresh opaque core realms; equal `Shared` labels
    /// rendezvous only inside this call. Labels never enter core or survive into
    /// another execution, and structural parentage contributes no placement.
    /// Plan EntryIds/order remain stable while realms, Fibers, publications,
    /// outcomes, and Fork ownership are execution-specific. Core owns each
    /// in-progress spawn through its Fork handoff. After a successful spawn,
    /// Loader owns that Fork until the complete outcome is returned; abandoning
    /// this future transfers all already-obtained Forks to framework-owned,
    /// reverse-success-order attempt-all disposal. Ordinary entry failure remains
    /// partial and never triggers rollback. Once this method returns, ownership is
    /// the caller's and dropping the delivered outcome or its Forks is inert.
    pub async fn load<R>(
        &self,
        ctx: &cordis_core::Context,
        resolver: &R,
    ) -> crate::outcome::LoadOutcome
    where
        R: crate::resolver::PluginResolver + ?Sized,
    {
        use crate::handoff::ResultHandoff;
        use crate::outcome::{EntryOutcome, LoaderFailure};

        let mut handoff = ResultHandoff::with_capacity(self.inner.nodes.len());
        let mut disabled_by_entry = HashMap::<EntryId, EntryId>::new();
        let mut realms = RealmEnvironment::new();

        for index in self.execution_order() {
            let node = &self.inner.nodes[index];
            let inherited_disabled = node
                .parent
                .as_ref()
                .and_then(|parent| disabled_by_entry.get(parent))
                .cloned();

            let outcome = if let Some(disabled_ancestor) = inherited_disabled {
                disabled_by_entry.insert(node.id.clone(), disabled_ancestor.clone());
                EntryOutcome::Pruned {
                    id: node.id.clone(),
                    disabled_ancestor,
                }
            } else {
                match &node.entry {
                    PlanEntry::Group => EntryOutcome::Group {
                        id: node.id.clone(),
                    },
                    PlanEntry::Plugin(entry) if entry.disabled => {
                        disabled_by_entry.insert(node.id.clone(), node.id.clone());
                        EntryOutcome::Disabled {
                            id: node.id.clone(),
                        }
                    }
                    PlanEntry::Plugin(entry) => {
                        let resolve_key = entry
                            .key
                            .as_deref()
                            .or(entry.name.as_deref())
                            .expect("validated Plugin entries always carry a resolve identity")
                            .to_owned();
                        match crate::resolver::resolve_entry(entry, resolver) {
                            Err(failure) => EntryOutcome::Failed {
                                id: node.id.clone(),
                                resolve_key,
                                failure: LoaderFailure::Resolver(failure),
                            },
                            Ok(None) => EntryOutcome::Failed {
                                id: node.id.clone(),
                                resolve_key: resolve_key.clone(),
                                failure: LoaderFailure::UnresolvedKey { key: resolve_key },
                            },
                            Ok(Some(prepared)) => {
                                let placed = realms.place(ctx, &entry.isolate);
                                match placed.spawn(prepared).await {
                                    Ok(fork) => EntryOutcome::Spawned {
                                        id: node.id.clone(),
                                        resolve_key,
                                        fork,
                                    },
                                    Err(failure) => EntryOutcome::Failed {
                                        id: node.id.clone(),
                                        resolve_key,
                                        failure: LoaderFailure::Spawn(failure),
                                    },
                                }
                            }
                        }
                    }
                }
            };
            handoff.push(outcome);
        }

        debug_assert_eq!(handoff.len(), self.inner.nodes.len());
        handoff.finish()
    }

    /// Compute semantic execution order without exposing or mutating topology.
    ///
    /// Child vectors are populated by frozen declaration order. Traversal only
    /// reads those vectors, so HashMap bucket order can never influence outcomes.
    fn execution_order(&self) -> Vec<usize> {
        let nodes = &self.inner.nodes;
        let mut roots = Vec::new();
        let mut children = HashMap::<EntryId, Vec<usize>>::new();

        for (index, node) in nodes.iter().enumerate() {
            if let Some(parent) = &node.parent {
                children.entry(parent.clone()).or_default().push(index);
            } else {
                roots.push(index);
            }
        }

        let mut order = Vec::with_capacity(nodes.len());
        let mut stack = roots.into_iter().rev().collect::<Vec<_>>();
        while let Some(index) = stack.pop() {
            order.push(index);
            if let Some(descendants) = children.get(&nodes[index].id) {
                stack.extend(descendants.iter().rev().copied());
            }
        }
        debug_assert_eq!(order.len(), nodes.len());
        order
    }
}

fn validate_plugin(id: &EntryId, entry: &PluginEntry) -> Result<(), PlanError> {
    if entry.key.is_none() && entry.name.is_none() {
        return Err(PlanError::MissingResolveIdentity { entry: id.clone() });
    }

    let mut inject = HashSet::with_capacity(entry.inject.len());
    for declaration in &entry.inject {
        let service = declaration.service();
        if !inject.insert(service) {
            return Err(PlanError::DuplicateInjectService {
                entry: id.clone(),
                service: service.to_owned(),
            });
        }
    }

    let mut isolate = HashSet::with_capacity(entry.isolate.len());
    for declaration in &entry.isolate {
        if !isolate.insert(declaration.service.as_str()) {
            return Err(PlanError::DuplicateIsolateService {
                entry: id.clone(),
                service: declaration.service.clone(),
            });
        }
    }
    Ok(())
}

fn validate_complete(lineage: &Arc<Lineage>, nodes: &[PlanNode]) -> Result<(), PlanError> {
    let mut parent_capable = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if let Some(parent) = &node.parent
            && (!parent.belongs_to(lineage) || !parent_capable.contains(parent))
        {
            return Err(PlanError::ForeignParent {
                parent: parent.clone(),
            });
        }
        if let PlanEntry::Plugin(entry) = &node.entry {
            validate_plugin(&node.id, entry)?;
        }
        if node.entry.is_parent_capable() {
            parent_capable.insert(node.id.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(key: &str) -> PluginEntry {
        PluginEntry {
            key: Some(key.to_owned()),
            name: None,
            config: Value::Null,
            disabled: false,
            inject: Vec::new(),
            isolate: Vec::new(),
        }
    }

    #[test]
    fn finish_retains_the_exact_admitted_identities() {
        let mut builder = LoadPlanBuilder::new();
        let root = builder
            .add_group(
                None,
                EntryGroup {
                    name: "root".into(),
                },
            )
            .unwrap();
        let child = builder.add_plugin(Some(&root), plugin("child")).unwrap();

        let plan = builder.finish().unwrap();
        assert!(plan.inner.nodes.iter().any(|node| node.id == root));
        assert!(plan.inner.nodes.iter().any(|node| node.id == child));

        let cloned = plan.clone();
        assert!(cloned.inner.nodes.iter().any(|node| node.id == root));
        assert!(cloned.inner.nodes.iter().any(|node| node.id == child));
    }
}
