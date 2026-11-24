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

pub const LEASE_BLOCK: u64 = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdBlock {
    pub start: u64,
    pub end: u64,
}

impl IdBlock {
    pub fn contains(self, id: u64) -> bool {
        self.start <= id && id < self.end
    }

    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Allocator {
    next: u64,
    end: Option<u64>,
    queued: std::collections::VecDeque<IdBlock>,
    overdrawn: bool,
}

impl Allocator {
    fn allocate(&mut self) -> u64 {
        if let Some(end) = self.end
            && self.next >= end
        {
            match self.queued.pop_front() {
                Some(block) => {
                    self.next = block.start;
                    self.end = Some(block.end);
                }
                None => self.overdrawn = true,
            }
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        id
    }
}

#[derive(Clone)]
pub struct IdSource {
    inner: std::sync::Arc<std::sync::Mutex<Allocator>>,
}

impl IdSource {
    pub fn starting_at(next: u64) -> Self {
        Self::from_allocator(Allocator {
            next: next.max(1),
            end: None,
            queued: std::collections::VecDeque::new(),
            overdrawn: false,
        })
    }

    pub fn leased(blocks: &[IdBlock], at: u64) -> Self {
        let mut queued: std::collections::VecDeque<IdBlock> = blocks
            .iter()
            .copied()
            .filter(|block| !block.is_empty())
            .collect();
        let mut allocator = Allocator {
            next: at,
            end: Some(at),
            queued: std::collections::VecDeque::new(),
            overdrawn: false,
        };
        while let Some(block) = queued.pop_front() {
            if block.start <= at && at <= block.end {
                allocator.end = Some(block.end);
                allocator.queued = queued;
                return Self::from_allocator(allocator);
            }
        }
        allocator.overdrawn = true;
        Self::from_allocator(allocator)
    }

    fn from_allocator(allocator: Allocator) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(allocator)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Allocator> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn next_value(&self) -> u64 {
        self.lock().next
    }

    pub fn allocate(&self) -> u64 {
        self.lock().allocate()
    }

    pub fn claim(&self, id: u64) {
        let mut allocator = self.lock();
        allocator.next = allocator.next.max(id.saturating_add(1));
    }

    pub fn shares_with(&self, other: &IdSource) -> bool {
        std::sync::Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn fork(&self) -> Self {
        Self::from_allocator(self.lock().clone())
    }

    pub fn adopt(&self, other: &IdSource) {
        if self.shares_with(other) {
            return;
        }
        let taken = other.lock().clone();
        *self.lock() = taken;
    }

    pub fn grant(&self, block: IdBlock) {
        if block.is_empty() {
            return;
        }
        let mut allocator = self.lock();
        let exhausted = allocator.end.is_none_or(|end| allocator.next >= end);
        if exhausted && allocator.queued.is_empty() {
            allocator.next = block.start;
            allocator.end = Some(block.end);
            allocator.overdrawn = false;
        } else {
            allocator.queued.push_back(block);
        }
    }

    pub fn is_leased(&self) -> bool {
        self.lock().end.is_some()
    }

    pub fn is_overdrawn(&self) -> bool {
        self.lock().overdrawn
    }

    pub fn remaining(&self) -> Option<u64> {
        let allocator = self.lock();
        let end = allocator.end?;
        let current = if allocator.overdrawn {
            0
        } else {
            end.saturating_sub(allocator.next)
        };
        Some(
            current
                + allocator
                    .queued
                    .iter()
                    .map(|block| block.len())
                    .sum::<u64>(),
        )
    }

    pub fn await_lease(&self) {
        let mut allocator = self.lock();
        allocator.end = Some(allocator.next);
        allocator.queued.clear();
        allocator.overdrawn = false;
    }

    pub fn release_lease(&self) {
        let mut allocator = self.lock();
        allocator.end = None;
        allocator.queued.clear();
        allocator.overdrawn = false;
    }
}

impl Default for IdSource {
    fn default() -> Self {
        Self::starting_at(1)
    }
}

impl std::fmt::Debug for IdSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let allocator = self.lock();
        f.debug_struct("IdSource")
            .field("next", &allocator.next)
            .field("end", &allocator.end)
            .field("queued", &allocator.queued)
            .finish()
    }
}

impl PartialEq for IdSource {
    fn eq(&self, other: &Self) -> bool {
        self.shares_with(other) || self.next_value() == other.next_value()
    }
}

impl Eq for IdSource {}
