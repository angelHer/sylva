use std::collections::HashMap;
use std::path::PathBuf;

use super::branch::Branch;
use super::commit::Commit;
use super::oid::Oid;
use super::worktree::Worktree;

/// An immutable view of a repository at one instant.
///
/// The UI thread never touches Git; it renders a snapshot that a worker thread
/// built and handed over. Rebuilding produces a whole new snapshot rather than
/// mutating this one, so rendering can never observe a half-updated repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySnapshot {
    /// Path to the primary working directory.
    root: PathBuf,
    /// Commits in graph order: newest first, topologically consistent.
    commits: Vec<Commit>,
    /// Commit id -> position in `commits`. Built once so the graph renderer
    /// can resolve parent edges in O(1) instead of scanning.
    positions: HashMap<Oid, usize>,
    branches: Vec<Branch>,
    worktrees: Vec<Worktree>,
    /// True when the walk stopped at a limit and older history was not loaded.
    truncated: bool,
}

impl RepositorySnapshot {
    pub fn new(
        root: PathBuf,
        commits: Vec<Commit>,
        branches: Vec<Branch>,
        worktrees: Vec<Worktree>,
        truncated: bool,
    ) -> Self {
        let positions = commits
            .iter()
            .enumerate()
            .map(|(index, commit)| (commit.id, index))
            .collect();

        Self {
            root,
            commits,
            positions,
            branches,
            worktrees,
            truncated,
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    pub fn branches(&self) -> &[Branch] {
        &self.branches
    }

    pub fn worktrees(&self) -> &[Worktree] {
        &self.worktrees
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn commit_count(&self) -> usize {
        self.commits.len()
    }

    pub fn position_of(&self, id: &Oid) -> Option<usize> {
        self.positions.get(id).copied()
    }

    pub fn commit(&self, id: &Oid) -> Option<&Commit> {
        self.position_of(id).map(|index| &self.commits[index])
    }

    /// Whether the commit is present in this snapshot. A parent can be absent
    /// when the walk was truncated, and the renderer must draw those edges as
    /// dangling rather than following them.
    pub fn contains(&self, id: &Oid) -> bool {
        self.positions.contains_key(id)
    }

    /// Every worktree anchored to a given commit. This is what makes the
    /// unified graph work: one commit can carry several worktree markers.
    pub fn worktrees_at(&self, id: &Oid) -> Vec<&Worktree> {
        self.worktrees
            .iter()
            .filter(|wt| wt.target().as_ref() == Some(id))
            .collect()
    }

    /// Commit -> worktrees anchored there, precomputed for one render pass.
    pub fn worktree_anchors(&self) -> HashMap<Oid, Vec<&Worktree>> {
        let mut anchors: HashMap<Oid, Vec<&Worktree>> = HashMap::new();
        for worktree in &self.worktrees {
            if let Some(target) = worktree.target() {
                anchors.entry(target).or_default().push(worktree);
            }
        }
        anchors
    }

    /// Commit -> branches pointing at it, for the ref chips on each row.
    pub fn branch_tips(&self) -> HashMap<Oid, Vec<&Branch>> {
        let mut tips: HashMap<Oid, Vec<&Branch>> = HashMap::new();
        for branch in &self.branches {
            tips.entry(branch.target).or_default().push(branch);
        }
        tips
    }

    /// The same repository without its history: root, branches and worktrees
    /// only.
    ///
    /// Guards on worktree operations look at refs and checkouts and never at
    /// commits, so this is what gets handed to a worker thread. Cloning the
    /// full snapshot to check whether a branch is already checked out would
    /// copy tens of thousands of commits for a question about a dozen
    /// directories.
    pub fn metadata_only(&self) -> Self {
        Self::new(
            self.root.clone(),
            Vec::new(),
            self.branches.clone(),
            self.worktrees.clone(),
            self.truncated,
        )
    }

    pub fn primary_worktree(&self) -> Option<&Worktree> {
        self.worktrees.iter().find(|wt| wt.is_primary)
    }

    /// Worktrees with uncommitted work or gone stale, newest concern first.
    pub fn worktrees_needing_attention(&self) -> Vec<&Worktree> {
        self.worktrees
            .iter()
            .filter(|wt| wt.needs_attention())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::branch::{Branch, BranchKind};
    use crate::domain::commit::{Signature, Timestamp};
    use crate::domain::worktree::{WorktreeHead, WorktreeStatus};

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    fn commit(id: u8, parents: &[u8]) -> Commit {
        let sig = Signature::new("Dev", "dev@example.com", Timestamp::from_utc(id as i64));
        Commit {
            id: oid(id),
            parents: parents.iter().copied().map(oid).collect(),
            summary: format!("commit {id}"),
            body: String::new(),
            author: sig.clone(),
            committer: sig,
        }
    }

    fn branch(name: &str, target: u8) -> Branch {
        Branch {
            name: name.into(),
            kind: BranchKind::Local,
            target: oid(target),
            upstream: None,
            divergence: None,
            is_head: false,
        }
    }

    fn worktree(name: &str, primary: bool, target: Option<u8>) -> Worktree {
        let head = match target {
            Some(t) => WorktreeHead::Branch {
                name: name.into(),
                target: oid(t),
            },
            None => WorktreeHead::Unborn { name: name.into() },
        };
        Worktree {
            name: name.into(),
            path: PathBuf::from(format!("/repo/{name}")),
            is_primary: primary,
            head,
            status: WorktreeStatus::default(),
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    fn snapshot() -> RepositorySnapshot {
        RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])],
            vec![branch("main", 3), branch("feature", 2)],
            vec![
                worktree("main", true, Some(3)),
                worktree("feature", false, Some(2)),
            ],
            false,
        )
    }

    #[test]
    fn commits_are_indexed_by_id_in_walk_order() {
        let snap = snapshot();
        assert_eq!(snap.commit_count(), 3);
        assert_eq!(snap.position_of(&oid(3)), Some(0));
        assert_eq!(snap.position_of(&oid(1)), Some(2));
        assert_eq!(snap.commit(&oid(2)).unwrap().summary, "commit 2");
    }

    #[test]
    fn a_commit_outside_the_snapshot_is_not_found() {
        let snap = snapshot();
        assert_eq!(snap.position_of(&oid(99)), None);
        assert!(snap.commit(&oid(99)).is_none());
        assert!(!snap.contains(&oid(99)));
    }

    #[test]
    fn worktrees_are_located_by_the_commit_they_sit_on() {
        let snap = snapshot();
        let at_tip = snap.worktrees_at(&oid(3));
        assert_eq!(at_tip.len(), 1);
        assert_eq!(at_tip[0].name, "main");
    }

    #[test]
    fn several_worktrees_on_the_same_commit_all_anchor_there() {
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![commit(1, &[])],
            vec![],
            vec![
                worktree("a", true, Some(1)),
                worktree("b", false, Some(1)),
                worktree("c", false, Some(1)),
            ],
            false,
        );
        let anchors = snap.worktree_anchors();
        assert_eq!(anchors.get(&oid(1)).map(Vec::len), Some(3));
    }

    #[test]
    fn an_unborn_worktree_anchors_nowhere() {
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![],
            vec![],
            vec![worktree("main", true, None)],
            false,
        );
        assert!(snap.worktree_anchors().is_empty());
        assert_eq!(snap.worktrees().len(), 1);
    }

    #[test]
    fn branches_pointing_at_one_commit_are_grouped_together() {
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![commit(1, &[])],
            vec![branch("main", 1), branch("release", 1)],
            vec![],
            false,
        );
        let tips = snap.branch_tips();
        assert_eq!(tips.get(&oid(1)).map(Vec::len), Some(2));
    }

    #[test]
    fn the_primary_worktree_is_identified() {
        let snap = snapshot();
        assert_eq!(snap.primary_worktree().map(|wt| wt.name.as_str()), Some("main"));
    }

    #[test]
    fn a_snapshot_of_clean_worktrees_reports_nothing_needing_attention() {
        assert!(snapshot().worktrees_needing_attention().is_empty());
    }

    #[test]
    fn a_dirty_worktree_is_reported_as_needing_attention() {
        let mut dirty = worktree("feature", false, Some(2));
        dirty.status.unstaged = 3;
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![commit(2, &[])],
            vec![],
            vec![worktree("main", true, Some(2)), dirty],
            false,
        );
        let flagged = snap.worktrees_needing_attention();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].name, "feature");
    }

    #[test]
    fn a_metadata_only_copy_keeps_the_refs_and_checkouts_but_drops_the_history() {
        let snap = snapshot();
        let meta = snap.metadata_only();

        assert_eq!(meta.commit_count(), 0);
        assert_eq!(meta.root(), snap.root());
        assert_eq!(meta.branches().len(), snap.branches().len());
        assert_eq!(meta.worktrees().len(), snap.worktrees().len());
    }

    #[test]
    fn a_metadata_only_copy_still_answers_which_worktree_is_primary() {
        // This is what the worktree guards actually ask it.
        let meta = snapshot().metadata_only();
        assert_eq!(meta.primary_worktree().map(|wt| wt.name.as_str()), Some("main"));
    }

    #[test]
    fn a_truncated_walk_is_reported_so_the_renderer_can_dangle_edges() {
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            vec![commit(2, &[1])],
            vec![],
            vec![],
            true,
        );
        assert!(snap.is_truncated());
        assert!(!snap.contains(&oid(1)));
    }
}
