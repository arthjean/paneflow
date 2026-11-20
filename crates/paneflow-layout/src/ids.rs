use serde::{Deserialize, Serialize};

macro_rules! layout_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub fn get(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

layout_id!(WorkspaceId);
layout_id!(TabId);
layout_id!(PaneId);
layout_id!(SurfaceId);
layout_id!(SplitId);

#[derive(Clone)]
pub struct IdSource {
    next: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl IdSource {
    pub fn starting_at(next: u64) -> Self {
        Self {
            next: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(next.max(1))),
        }
    }

    pub fn next_value(&self) -> u64 {
        self.next.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn allocate(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    pub fn claim(&self, id: u64) {
        self.next
            .fetch_max(id.saturating_add(1), std::sync::atomic::Ordering::SeqCst);
    }

    pub fn shares_with(&self, other: &IdSource) -> bool {
        std::sync::Arc::ptr_eq(&self.next, &other.next)
    }
}

impl Default for IdSource {
    fn default() -> Self {
        Self::starting_at(1)
    }
}

impl std::fmt::Debug for IdSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdSource")
            .field("next", &self.next_value())
            .finish()
    }
}

impl PartialEq for IdSource {
    fn eq(&self, other: &Self) -> bool {
        self.next_value() == other.next_value()
    }
}

impl Eq for IdSource {}
