//! The Cleanup Tray (PRD C1): a staging list of node ids.
//!
//! - Tied to one scan (`generation`); a new scan means a new tray.
//! - Every mutation bumps `version`, so a UI can ignore stale responses.
//! - Nested items are deduplicated: adding a folder drops its descendants
//!   already in the tray; adding something inside a tray folder is a no-op.
//! - Cheap safety checks run on add for early feedback. The full pipeline
//!   (including disk re-validation) still runs again at execution.

use super::{static_checks, CleanupContext, SkipReason};
use crate::size::SizeMode;
use crate::tree::{NodeId, Tree};

/// What happened to the ids passed to [`Tray::add`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AddOutcome {
    /// Newly added.
    pub added: Vec<NodeId>,
    /// Already in the tray, or inside a folder that is.
    pub covered: Vec<NodeId>,
    /// Previously in the tray, dropped because a folder containing them was added.
    pub replaced: Vec<NodeId>,
    /// Refused, with the reason.
    pub rejected: Vec<(NodeId, SkipReason)>,
}

/// The staging list.
#[derive(Debug, Clone)]
pub struct Tray {
    generation: u64,
    version: u64,
    items: Vec<NodeId>,
}

impl Tray {
    /// An empty tray for the scan identified by `generation`.
    pub fn new(generation: u64) -> Self {
        Tray {
            generation,
            version: 1,
            items: Vec::new(),
        }
    }

    /// The scan this tray belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Increases on every change.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Items in the order they were added.
    pub fn items(&self) -> &[NodeId] {
        &self.items
    }

    /// Adds items after cheap safety checks and deduplication.
    pub fn add(&mut self, tree: &Tree, ids: &[NodeId], ctx: &CleanupContext) -> AddOutcome {
        let mut out = AddOutcome::default();
        for &id in ids {
            if self.items.iter().any(|&t| tree.is_ancestor_or_self(t, id)) {
                out.covered.push(id);
                continue;
            }
            if let Err(reason) = static_checks(tree, id, ctx) {
                out.rejected.push((id, reason));
                continue;
            }
            let (inside, keep): (Vec<NodeId>, Vec<NodeId>) = self
                .items
                .iter()
                .partition(|&&t| tree.is_ancestor_or_self(id, t));
            out.replaced.extend(inside);
            self.items = keep;
            self.items.push(id);
            out.added.push(id);
        }
        if !out.added.is_empty() {
            self.version += 1;
        }
        out
    }

    /// Removes items (unknown ids are ignored).
    pub fn remove(&mut self, ids: &[NodeId]) {
        let before = self.items.len();
        self.items.retain(|i| !ids.contains(i));
        if self.items.len() != before {
            self.version += 1;
        }
    }

    /// Empties the tray.
    pub fn clear(&mut self) {
        self.items.clear();
        self.version += 1;
    }

    /// Drops items whose nodes were removed by a cleanup run.
    pub fn prune_removed(&mut self, tree: &Tree) {
        let before = self.items.len();
        self.items.retain(|&i| {
            tree.get(i)
                .is_some_and(|n| !n.flags.contains(crate::tree::NodeFlags::REMOVED))
        });
        if self.items.len() != before {
            self.version += 1;
        }
    }

    /// Estimated total of all items in `mode` (hardlink duplicates count 0).
    pub fn total(&self, tree: &Tree, mode: SizeMode) -> u64 {
        self.items
            .iter()
            .filter_map(|&i| tree.get(i))
            .map(|n| n.size(mode))
            .sum()
    }

    /// Total number of files the items contain.
    pub fn file_count(&self, tree: &Tree) -> u64 {
        self.items
            .iter()
            .filter_map(|&i| tree.get(i))
            .map(|n| u64::from(n.file_count.max(1)))
            .sum()
    }
}
