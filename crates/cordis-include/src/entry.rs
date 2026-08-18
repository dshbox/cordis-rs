//! In-memory entry nodes: identity, runtime state, and ancestor walks.

use crate::error::{IncludeError, Result};
use crate::expr::evaluate_node;
use crate::interpolate::interpolate_node;
use crate::lock;
use crate::options::{Disabled, EntryOptions};
use cordis::Fiber;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// A live entry in an [`crate::EntryTree`].
///
/// `Entry` is a cheap handle (interior `Arc`); two handles refer to the same
/// entry exactly when [`Entry::ptr_eq`] says so. The loader attaches the
/// entry's [`Fiber`] here and uses [`Entry::suspend`] to suppress write-back
/// while applying changes that originate from the config file itself.
#[derive(Clone)]
pub struct Entry {
    inner: Arc<EntryInner>,
}

/// Immutable identity plus guarded runtime state of one entry.
struct EntryInner {
    id: String,
    state: Mutex<EntryState>,
}

/// Fields that mutate over the entry's lifetime.
pub(crate) struct EntryState {
    options: EntryOptions,
    parent: Option<Weak<EntryInner>>,
    children: Vec<Entry>,
    fiber: Option<Fiber>,
    suspend: usize,
}

impl Entry {
    /// Create a detached leaf entry; the tree wires parent and children.
    pub(crate) fn new(id: String, options: EntryOptions) -> Self {
        debug_assert_eq!(options.id.as_deref(), Some(id.as_str()));
        Self {
            inner: Arc::new(EntryInner {
                id,
                state: Mutex::new(EntryState {
                    options,
                    parent: None,
                    children: Vec::new(),
                    fiber: None,
                    suspend: 0,
                }),
            }),
        }
    }

    /// The synthetic tree root, addressed by the empty id.
    pub(crate) fn new_root() -> Self {
        Self::new(
            String::new(),
            EntryOptions {
                id: Some(String::new()),
                ..EntryOptions::default()
            },
        )
    }

    /// Stable identity of this entry; never empty except for the root.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// Whether both handles refer to the same entry.
    pub fn ptr_eq(left: &Entry, right: &Entry) -> bool {
        Arc::ptr_eq(&left.inner, &right.inner)
    }

    /// Borrow the guarded runtime state.
    pub(crate) fn state(&self) -> MutexGuard<'_, EntryState> {
        lock(&self.inner.state)
    }

    /// The plugin name from the entry options.
    pub fn name(&self) -> String {
        self.state().options.name.clone()
    }

    /// A snapshot of the raw entry options (config templates unexpanded,
    /// `group` always empty — children are live tree state).
    pub fn options(&self) -> EntryOptions {
        let mut options = self.state().options.clone();
        options.group = Vec::new();
        options
    }

    /// Replace the entry options. The id is identity and never changes.
    pub(crate) fn set_options(&self, options: EntryOptions) {
        let mut state = self.state();
        state.options = options;
        state.options.id = Some(self.inner.id.clone());
        state.options.group = Vec::new();
    }

    /// The raw config with `${{ ... }}` templates still intact.
    pub fn config(&self) -> Option<crate::node::Node> {
        self.state().options.config.clone()
    }

    /// The config with `${{ env.NAME }}` templates expanded and every
    /// `!!js` expression node evaluated, ready to be handed to a plugin
    /// as `cordis_rs::Value::new(node)`. An out-of-subset expression
    /// (for example one referencing `ctx.*`) fails here.
    pub fn resolved_config(&self) -> Result<Option<crate::node::Node>> {
        match self.config() {
            Some(node) => evaluate_node(&interpolate_node(&node)?).map(Some),
            None => Ok(None),
        }
    }

    /// The parent entry, or `None` for the tree root and detached entries.
    pub fn parent(&self) -> Option<Entry> {
        let parent = self.state().parent.clone();
        parent
            .and_then(|weak| weak.upgrade())
            .map(|inner| Entry { inner })
    }

    /// Child entries in file order. Empty for leaf entries.
    pub fn children(&self) -> Vec<Entry> {
        self.state().children.clone()
    }

    /// Whether this entry currently has children (i.e. acts as a group).
    pub fn is_group(&self) -> bool {
        !self.state().children.is_empty()
    }

    /// Replace the child list, re-pointing moved-in children and clearing
    /// the parent link of children that were dropped. Detached children
    /// keep their own subtrees intact for teardown.
    pub(crate) fn set_children(&self, new_children: Vec<Entry>) {
        let old_children = {
            let mut state = self.state();
            std::mem::replace(&mut state.children, new_children.clone())
        };
        for child in &new_children {
            child.state().parent = Some(Arc::downgrade(&self.inner));
        }
        for child in &old_children {
            let still_present = new_children.iter().any(|kept| Entry::ptr_eq(kept, child));
            if !still_present {
                child.state().parent = None;
            }
        }
    }

    /// Composite id from the tree root down to this entry, e.g. `a1:b2`.
    pub fn path(&self) -> String {
        let mut parts = vec![self.inner.id.clone()];
        let mut current = self.parent();
        while let Some(parent) = current {
            if parent.inner.id.is_empty() {
                break;
            }
            parts.push(parent.inner.id.clone());
            current = parent.parent();
        }
        parts.reverse();
        parts.join(":")
    }

    /// The entry's own disable slot, statically viewed: [`Disabled::Flag`]
    /// yields the flag, an unevaluated expression does not disable. The
    /// options snapshot ([`Entry::options`]) carries the raw slot for
    /// write-back.
    pub fn disabled(&self) -> bool {
        self.state().options.disabled.is_disabled()
    }

    /// The entry's own disable state with its `!!js` expression evaluated:
    /// the expression must evaluate to a boolean, or the error propagates.
    pub fn resolved_disabled(&self) -> Result<bool> {
        let disabled = self.state().options.disabled.clone();
        match disabled {
            Disabled::Flag(flag) => Ok(flag),
            Disabled::Expr(source) => match crate::expr::evaluate(&source)? {
                crate::node::Node::Bool(flag) => Ok(flag),
                other => Err(IncludeError::JsExpression {
                    message: format!(
                        "the disabled expression must evaluate to a boolean, found {}",
                        crate::yaml::node_kind(&other)
                    ),
                    expression: source,
                }),
            },
        }
    }

    /// Whether this entry and every ancestor are statically enabled. A
    /// disabled group cascades to its whole subtree. `!!js` expressions
    /// are *not* evaluated here — an unevaluated expression does not
    /// disable; use [`Entry::resolved_enabled`] for the evaluated
    /// decision.
    pub fn enabled(&self) -> bool {
        if self.is_root() {
            return true;
        }
        !self.disabled() && self.parent().is_none_or(|parent| parent.enabled())
    }

    /// Whether this entry and every ancestor are enabled once their
    /// `!!js` expressions are evaluated (a disabled group cascades to
    /// its whole subtree). The first evaluation failure propagates.
    pub fn resolved_enabled(&self) -> Result<bool> {
        if self.is_root() {
            return Ok(true);
        }
        if self.resolved_disabled()? {
            return Ok(false);
        }
        match self.parent() {
            Some(parent) => parent.resolved_enabled(),
            None => Ok(true),
        }
    }

    /// The fiber started for this entry, if any (set by the loader layer).
    pub fn fiber(&self) -> Option<Fiber> {
        self.state().fiber.clone()
    }

    /// Attach or detach the entry's fiber.
    pub fn set_fiber(&self, fiber: Option<Fiber>) {
        self.state().fiber = fiber;
    }

    /// Increment the suspend counter, returning a guard. While suspended,
    /// the loader suppresses config write-back for this entry.
    pub fn suspend(&self) -> EntrySuspendGuard {
        {
            let mut state = self.state();
            state.suspend += 1;
        }
        EntrySuspendGuard {
            entry: self.clone(),
        }
    }

    /// Whether any suspend guard is currently held for this entry.
    pub fn is_suspended(&self) -> bool {
        self.state().suspend > 0
    }

    pub(crate) fn is_root(&self) -> bool {
        self.inner.id.is_empty()
    }

    /// Whether `other` is this entry or one of its descendants.
    pub(crate) fn contains(&self, other: &Entry) -> bool {
        if Entry::ptr_eq(self, other) {
            return true;
        }
        other.parent().is_some_and(|parent| self.contains(&parent))
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.inner.id)
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// RAII guard for the entry-level suspend counter.
///
/// Dropping the guard decrements the counter; write-back resumes once all
/// guards for the entry are gone.
#[derive(Debug)]
pub struct EntrySuspendGuard {
    entry: Entry,
}

impl Drop for EntrySuspendGuard {
    fn drop(&mut self) {
        let mut state = self.entry.state();
        state.suspend = state.suspend.saturating_sub(1);
    }
}
