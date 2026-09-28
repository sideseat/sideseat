use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::types::BranchId;

/// Lifecycle hook interface for pluggable context extensions.
///
/// Stateless extensions (Canvas, Kanban, Artifact) hold no internal state and
/// receive storage via method parameters. Stateful extensions (VFS, CRDT) hold
/// state internally behind interior mutability and use lifecycle hooks to stay
/// in sync with the active branch.
pub trait ContextExtension: Send + Sync + 'static {
    /// Unique string identifier for this extension (e.g. `"crdt"`, `"vfs"`, `"canvas"`).
    fn id(&self) -> &str;
    /// Called after a branch is forked; stateful extensions (VFS, CRDT) use this to
    /// copy their branch-scoped state to the new child branch.
    fn on_branch_forked(&self, _parent: &BranchId, _child: &BranchId) {}
    /// Called after `checkout()`; stateful extensions use this to switch their
    /// internal view to the new active branch.
    fn on_branch_checked_out(&self, _branch: &BranchId) {}
}

// ---------------------------------------------------------------------------
// ExtensionRegistry — type-erased extension container
// ---------------------------------------------------------------------------

struct ExtensionRegistryInner {
    /// Hook list for lifecycle callbacks — same order as insertion.
    hooks: Vec<Arc<dyn ContextExtension>>,
    /// Type-erased store for typed lookups by concrete type or string ID.
    store: HashMap<String, Arc<dyn Any + Send + Sync>>,
}

/// Type-erased registry for [`ContextExtension`] instances.
///
/// Supports typed lookup by concrete type (`extension::<T>()`) or string ID
/// (`extension_by_id()`), and broadcasts lifecycle hooks to all registered
/// extensions in insertion order.
pub struct ExtensionRegistry {
    inner: RwLock<ExtensionRegistryInner>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(ExtensionRegistryInner {
                hooks: Vec::new(),
                store: HashMap::new(),
            }),
        }
    }

    /// Register an extension. Both the type-erased store entry and the hook
    /// list entry are inserted under the same write lock, eliminating any
    /// window where `extension<T>()` and `fire_branch_forked()` could observe
    /// an inconsistent view of the registry.
    pub(crate) fn register<T: ContextExtension>(&self, ext: Arc<T>) {
        let id = ext.id().to_string();
        let mut guard = self.inner.write();
        guard
            .store
            .insert(id, ext.clone() as Arc<dyn Any + Send + Sync>);
        guard.hooks.push(ext as Arc<dyn ContextExtension>);
    }

    /// Return the registered extension whose concrete type is `T`, if any.
    pub fn extension<T: ContextExtension>(&self) -> Option<Arc<T>> {
        self.inner
            .read()
            .store
            .values()
            .find_map(|arc| arc.clone().downcast::<T>().ok())
    }

    /// Return a registered extension by its string ID and cast to `T`.
    pub fn extension_by_id<T: ContextExtension>(&self, id: &str) -> Option<Arc<T>> {
        self.inner
            .read()
            .store
            .get(id)
            .and_then(|arc| arc.clone().downcast::<T>().ok())
    }

    pub(crate) fn fire_branch_forked(&self, parent: &BranchId, child: &BranchId) {
        for ext in self.inner.read().hooks.iter() {
            ext.on_branch_forked(parent, child);
        }
    }

    pub(crate) fn fire_branch_checked_out(&self, branch: &BranchId) {
        for ext in self.inner.read().hooks.iter() {
            ext.on_branch_checked_out(branch);
        }
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
