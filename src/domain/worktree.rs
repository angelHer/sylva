use std::path::{Path, PathBuf};

use super::branch::Divergence;
use super::oid::Oid;

/// What a worktree currently has checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeHead {
    /// On a branch. The oid is the branch tip.
    Branch { name: String, target: Oid },
    /// Detached HEAD, sitting directly on a commit.
    Detached { target: Oid },
    /// An unborn branch: the worktree exists but has no commit yet.
    Unborn { name: String },
}

impl WorktreeHead {
    /// The commit this worktree points at, if there is one. This is the anchor
    /// the unified graph uses to place the worktree marker.
    pub fn target(&self) -> Option<Oid> {
        match self {
            Self::Branch { target, .. } | Self::Detached { target } => Some(*target),
            Self::Unborn { .. } => None,
        }
    }

    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Self::Branch { name, .. } | Self::Unborn { name } => Some(name),
            Self::Detached { .. } => None,
        }
    }

    pub fn is_detached(&self) -> bool {
        matches!(self, Self::Detached { .. })
    }

    /// Label for the worktree badge drawn on the graph.
    pub fn label(&self) -> String {
        match self {
            Self::Branch { name, .. } | Self::Unborn { name } => name.clone(),
            Self::Detached { target } => format!("({})", target.to_short_hex(7)),
        }
    }
}

/// Working-directory state, counted per file (not per hunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorktreeStatus {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

impl WorktreeStatus {
    pub const fn is_clean(&self) -> bool {
        self.staged == 0 && self.unstaged == 0 && self.untracked == 0 && self.conflicted == 0
    }

    pub const fn is_dirty(&self) -> bool {
        !self.is_clean()
    }

    pub const fn has_conflicts(&self) -> bool {
        self.conflicted > 0
    }

    /// Total files needing attention. Drives the number on the dirty badge.
    pub const fn total_changed(&self) -> usize {
        self.staged + self.unstaged + self.untracked + self.conflicted
    }
}

/// A checkout of the repository: either the primary working directory or one
/// linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Git's worktree name. The primary worktree has none of its own, so it is
    /// named after its directory.
    pub name: String,
    pub path: PathBuf,
    pub is_primary: bool,
    pub head: WorktreeHead,
    pub status: WorktreeStatus,
    /// Divergence of this worktree's branch from its upstream.
    pub divergence: Option<Divergence>,
    pub is_locked: bool,
    /// Git considers the worktree removable: its directory is gone or stale.
    pub is_prunable: bool,
}

impl Worktree {
    /// The commit this worktree sits on, for placement in the unified graph.
    pub fn target(&self) -> Option<Oid> {
        self.head.target()
    }

    /// A worktree needs the user's attention when it has uncommitted work,
    /// conflicts, or has gone stale.
    pub fn needs_attention(&self) -> bool {
        self.status.is_dirty() || self.is_prunable
    }

    /// Directory name, which is what the sidebar shows.
    pub fn dir_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.name)
    }

    pub fn is_at(&self, path: &Path) -> bool {
        self.path == path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    fn worktree(head: WorktreeHead, status: WorktreeStatus) -> Worktree {
        Worktree {
            name: "feature-graph".into(),
            path: PathBuf::from("/home/dev/project-worktrees/feature-graph"),
            is_primary: false,
            head,
            status,
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    #[test]
    fn a_worktree_on_a_branch_anchors_to_the_branch_tip() {
        let head = WorktreeHead::Branch {
            name: "feature/graph".into(),
            target: oid(0xab),
        };
        assert_eq!(head.target(), Some(oid(0xab)));
        assert_eq!(head.branch_name(), Some("feature/graph"));
        assert!(!head.is_detached());
        assert_eq!(head.label(), "feature/graph");
    }

    #[test]
    fn a_detached_worktree_anchors_to_its_commit_and_is_labelled_by_hash() {
        let head = WorktreeHead::Detached { target: oid(0xab) };
        assert_eq!(head.target(), Some(oid(0xab)));
        assert_eq!(head.branch_name(), None);
        assert!(head.is_detached());
        assert_eq!(head.label(), "(abababa)");
    }

    #[test]
    fn an_unborn_worktree_has_no_anchor_in_the_graph() {
        let head = WorktreeHead::Unborn {
            name: "main".into(),
        };
        assert_eq!(head.target(), None);
        assert_eq!(head.branch_name(), Some("main"));
        assert_eq!(head.label(), "main");
    }

    #[test]
    fn a_status_with_no_changes_is_clean() {
        let status = WorktreeStatus::default();
        assert!(status.is_clean());
        assert!(!status.is_dirty());
        assert_eq!(status.total_changed(), 0);
    }

    #[test]
    fn untracked_files_alone_make_a_worktree_dirty() {
        let status = WorktreeStatus {
            untracked: 2,
            ..Default::default()
        };
        assert!(status.is_dirty());
        assert_eq!(status.total_changed(), 2);
    }

    #[test]
    fn total_changed_sums_every_category() {
        let status = WorktreeStatus {
            staged: 1,
            unstaged: 2,
            untracked: 3,
            conflicted: 4,
        };
        assert_eq!(status.total_changed(), 10);
        assert!(status.has_conflicts());
    }

    #[test]
    fn a_clean_worktree_needs_no_attention() {
        let wt = worktree(
            WorktreeHead::Detached { target: oid(1) },
            WorktreeStatus::default(),
        );
        assert!(!wt.needs_attention());
    }

    #[test]
    fn a_dirty_worktree_needs_attention() {
        let wt = worktree(
            WorktreeHead::Detached { target: oid(1) },
            WorktreeStatus {
                unstaged: 1,
                ..Default::default()
            },
        );
        assert!(wt.needs_attention());
    }

    #[test]
    fn a_clean_but_prunable_worktree_still_needs_attention() {
        let mut wt = worktree(
            WorktreeHead::Detached { target: oid(1) },
            WorktreeStatus::default(),
        );
        wt.is_prunable = true;
        assert!(wt.needs_attention());
    }

    #[test]
    fn the_sidebar_label_is_the_directory_name() {
        let wt = worktree(
            WorktreeHead::Detached { target: oid(1) },
            WorktreeStatus::default(),
        );
        assert_eq!(wt.dir_name(), "feature-graph");
    }
}
