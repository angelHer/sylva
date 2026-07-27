//! Which commits one worktree can actually see.
//!
//! Answering "what has this worktree done?" means walking back from its HEAD
//! through every parent. That set is what the graph highlights when a worktree
//! is focused, and what makes a single line of work readable inside a history
//! that hundreds of branches share.

use super::oid::Oid;
use super::snapshot::RepositorySnapshot;

/// The commits reachable from one tip, as a mask over a snapshot's rows.
///
/// A `Vec<bool>` indexed by row rather than a set of ids: the renderer asks
/// this question once per visible row per frame, and an index is free where a
/// hash lookup is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestry {
    tip: Oid,
    reachable: Vec<bool>,
    count: usize,
}

impl Ancestry {
    /// Walks back from `tip` through every parent present in the snapshot.
    ///
    /// One forward pass, no queue: commits are ordered with parents after
    /// children, so by the time a row is reached, every child that could have
    /// marked it has already been visited.
    ///
    /// A parent that is missing — truncated history — or that sits earlier in
    /// the list is not followed, exactly as the graph layout does not draw a
    /// line to it. The highlight and the drawn lines therefore always agree.
    pub fn of(snapshot: &RepositorySnapshot, tip: Oid) -> Self {
        let mut reachable = vec![false; snapshot.commit_count()];
        let mut count = 0;

        if let Some(start) = snapshot.position_of(&tip) {
            reachable[start] = true;
            count = 1;

            let commits = snapshot.commits();
            for position in start..commits.len() {
                if !reachable[position] {
                    continue;
                }

                for parent in &commits[position].parents {
                    let Some(parent_position) = snapshot.position_of(parent) else {
                        continue;
                    };
                    if parent_position > position && !reachable[parent_position] {
                        reachable[parent_position] = true;
                        count += 1;
                    }
                }
            }
        }

        Self {
            tip,
            reachable,
            count,
        }
    }

    pub fn tip(&self) -> Oid {
        self.tip
    }

    /// How many commits this tip can see inside the loaded snapshot.
    pub fn count(&self) -> usize {
        self.count
    }

    /// True when the tip is not in the snapshot at all, so nothing should be
    /// highlighted and nothing dimmed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn includes_position(&self, position: usize) -> bool {
        self.reachable.get(position).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::commit::{Commit, Signature, Timestamp};

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

    fn snapshot(commits: Vec<Commit>) -> RepositorySnapshot {
        RepositorySnapshot::new(PathBuf::from("/repo"), commits, vec![], vec![], false)
    }

    /// Ids of the commits an ancestry covers, for readable assertions.
    fn covered(snapshot: &RepositorySnapshot, ancestry: &Ancestry) -> Vec<u8> {
        snapshot
            .commits()
            .iter()
            .enumerate()
            .filter(|(position, _)| ancestry.includes_position(*position))
            .map(|(_, commit)| commit.id.as_bytes()[0])
            .collect()
    }

    #[test]
    fn a_tip_sees_its_whole_linear_history() {
        let snap = snapshot(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);
        let ancestry = Ancestry::of(&snap, oid(3));

        assert_eq!(covered(&snap, &ancestry), vec![3, 2, 1]);
        assert_eq!(ancestry.count(), 3);
        assert_eq!(ancestry.tip(), oid(3));
    }

    #[test]
    fn a_tip_does_not_see_commits_made_after_it() {
        let snap = snapshot(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);
        let ancestry = Ancestry::of(&snap, oid(2));

        assert_eq!(covered(&snap, &ancestry), vec![2, 1]);
        assert!(!ancestry.includes_position(0));
    }

    #[test]
    fn two_branches_see_their_shared_base_but_not_each_other() {
        // 2 and 3 both descend from 1; neither can see the other.
        let snap = snapshot(vec![commit(3, &[1]), commit(2, &[1]), commit(1, &[])]);

        let left = Ancestry::of(&snap, oid(3));
        let right = Ancestry::of(&snap, oid(2));

        assert_eq!(covered(&snap, &left), vec![3, 1]);
        assert_eq!(covered(&snap, &right), vec![2, 1]);
    }

    #[test]
    fn a_merge_sees_both_sides_of_the_history_it_joined() {
        let snap = snapshot(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);
        let ancestry = Ancestry::of(&snap, oid(4));

        assert_eq!(covered(&snap, &ancestry), vec![4, 2, 3, 1]);
        assert_eq!(ancestry.count(), 4);
    }

    #[test]
    fn a_shared_commit_is_counted_once_however_many_paths_reach_it() {
        let snap = snapshot(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);
        assert_eq!(Ancestry::of(&snap, oid(4)).count(), 4);
    }

    #[test]
    fn a_tip_outside_the_snapshot_covers_nothing() {
        let snap = snapshot(vec![commit(2, &[1]), commit(1, &[])]);
        let ancestry = Ancestry::of(&snap, oid(99));

        assert!(ancestry.is_empty());
        assert_eq!(ancestry.count(), 0);
        assert!(!ancestry.includes_position(0));
    }

    #[test]
    fn an_empty_snapshot_yields_an_empty_ancestry() {
        let snap = snapshot(vec![]);
        assert!(Ancestry::of(&snap, oid(1)).is_empty());
    }

    #[test]
    fn a_parent_lost_to_truncation_simply_ends_the_walk() {
        // Commit 1 was never loaded.
        let snap = snapshot(vec![commit(3, &[2]), commit(2, &[1])]);
        let ancestry = Ancestry::of(&snap, oid(3));

        assert_eq!(covered(&snap, &ancestry), vec![3, 2]);
    }

    #[test]
    fn asking_beyond_the_last_row_is_false_rather_than_a_panic() {
        let snap = snapshot(vec![commit(1, &[])]);
        let ancestry = Ancestry::of(&snap, oid(1));

        assert!(ancestry.includes_position(0));
        assert!(!ancestry.includes_position(9_999));
    }

    #[test]
    fn a_parent_listed_above_its_child_is_not_followed() {
        // Same rule the graph layout applies, so the highlight can never
        // disagree with the lines that are drawn.
        let snap = snapshot(vec![commit(1, &[]), commit(2, &[1])]);
        let ancestry = Ancestry::of(&snap, oid(2));

        assert_eq!(covered(&snap, &ancestry), vec![2]);
    }
}
