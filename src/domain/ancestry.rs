//! Which commits belong to one worktree's line of work.
//!
//! Walking back from a HEAD through every parent answers "what can this
//! worktree see?", but that is rarely the question being asked: a branch cut
//! from `dev` can see the whole of `dev`, so highlighting all of it says
//! nothing about the branch. The line of work is the stretch between the HEAD
//! and the commit the branch was cut from, and that is what gets highlighted.
//!
//! The cut is the fork *commit*, not the parent branch's tip, so the highlight
//! does not grow when that branch moves on. What ends a line is another
//! worktree — a line somebody has checked out — and never a branch left behind
//! on the trunk years ago.

use super::oid::Oid;
use super::snapshot::RepositorySnapshot;
use super::worktree::Worktree;

/// One line of work, as a mask over a snapshot's rows.
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
    /// Walks back from `tip` and stops where this branch joined the one it was
    /// cut from.
    ///
    /// Everything the fork commit can see was written by that other line, so
    /// it is dropped; the fork commit itself stays lit, anchoring the branch to
    /// the trunk it grew out of. Work merged *into* this branch since the fork
    /// is not below it and stays lit too.
    pub fn of(snapshot: &RepositorySnapshot, tip: Oid) -> Self {
        let start = snapshot.position_of(&tip);
        let mut reachable = walk_back(snapshot, start.as_slice());

        if let Some(fork) = start.and_then(|start| fork_point(snapshot, &tip, start)) {
            let older = walk_back(snapshot, &[fork]);
            for (position, lit) in reachable.iter_mut().enumerate() {
                if position != fork && older[position] {
                    *lit = false;
                }
            }
        }

        let count = reachable.iter().filter(|lit| **lit).count();

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

/// Every commit reachable from the given rows, as one mask.
///
/// One forward pass, no queue: commits are ordered with parents after
/// children, so by the time a row is reached, every child that could have
/// marked it has already been visited. Seeding several rows at once therefore
/// costs the same as seeding one.
///
/// A parent that is missing — truncated history — or that sits earlier in the
/// list is not followed, exactly as the graph layout does not draw a line to
/// it. The highlight and the drawn lines therefore always agree.
fn walk_back(snapshot: &RepositorySnapshot, seeds: &[usize]) -> Vec<bool> {
    let mut reachable = vec![false; snapshot.commit_count()];
    let Some(first) = seeds.iter().copied().min() else {
        return reachable;
    };
    for seed in seeds {
        reachable[*seed] = true;
    }

    let commits = snapshot.commits();
    for position in first..commits.len() {
        if !reachable[position] {
            continue;
        }

        for parent in &commits[position].parents {
            let Some(parent_position) = snapshot.position_of(parent) else {
                continue;
            };
            if parent_position > position {
                reachable[parent_position] = true;
            }
        }
    }

    reachable
}

/// Whether `from` is this line carried further: its first parents lead back to
/// `start` without ever leaving its own spine.
///
/// Descending from `start` is not enough. A trunk that merged this branch in
/// descends from it too, and that trunk is exactly the line the branch was cut
/// from — but the merge joined as a second parent, off the spine.
fn stands_on(snapshot: &RepositorySnapshot, from: usize, start: usize) -> bool {
    let commits = snapshot.commits();
    let mut position = from;

    while position < start {
        let Some(parent) = commits[position].parents.first() else {
            return false;
        };
        let Some(next) = snapshot.position_of(parent) else {
            return false;
        };
        if next <= position {
            return false;
        }
        position = next;
    }

    position == start
}

/// The row where this line of work stopped being its own.
///
/// The other lines are the other worktrees, not every branch in the
/// repository. A repository accumulates branches left pointing at old trunk
/// commits, and treating one of those as a fork would cut the trunk short at a
/// commit nobody is working on. A worktree is a line somebody actually has
/// checked out, which is the only claim strong enough to end another one.
///
/// Searched along first parents only, because that is this line's own spine: a
/// branch merged *into* it arrives as a second parent, and taking one of those
/// for the fork would cut the trunk short at its latest merge.
///
/// A spine row is the fork as soon as another line already holds it, as its
/// HEAD or anywhere in its history. Holding it in *history* is what keeps the
/// cut still when the parent branch moves on: `dev` racing ahead does not
/// change which commit this branch was cut from.
fn fork_point(snapshot: &RepositorySnapshot, tip: &Oid, start: usize) -> Option<usize> {
    // The primary checkout is what every other line was cut from; there is
    // nothing above it to stop at.
    if snapshot.primary_worktree().and_then(Worktree::target) == Some(*tip) {
        return None;
    }

    // A line carrying this one further was cut *from* it, and says nothing
    // about where it began.
    let elsewhere: Vec<usize> = snapshot
        .worktrees()
        .iter()
        .filter_map(Worktree::target)
        .filter(|target| target != tip)
        .filter_map(|target| snapshot.position_of(&target))
        .filter(|position| !stands_on(snapshot, *position, start))
        .collect();
    let held_elsewhere = walk_back(snapshot, &elsewhere);

    let commits = snapshot.commits();
    let mut position = start;
    loop {
        if held_elsewhere[position] {
            return Some(position);
        }

        let next = snapshot.position_of(commits[position].parents.first()?)?;
        if next <= position {
            return None;
        }
        position = next;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::branch::{Branch, BranchKind};
    use crate::domain::commit::{Commit, Signature, Timestamp};
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

    fn snapshot(commits: Vec<Commit>) -> RepositorySnapshot {
        RepositorySnapshot::new(PathBuf::from("/repo"), commits, vec![], vec![], false)
    }

    fn snapshot_with(commits: Vec<Commit>, worktrees: Vec<Worktree>) -> RepositorySnapshot {
        RepositorySnapshot::new(PathBuf::from("/repo"), commits, vec![], worktrees, false)
    }

    fn checkout(name: &str, target: u8, is_primary: bool) -> Worktree {
        Worktree {
            name: name.into(),
            path: PathBuf::from(format!("/repo/{name}")),
            is_primary,
            head: WorktreeHead::Branch {
                name: name.into(),
                target: oid(target),
            },
            status: WorktreeStatus::default(),
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    fn primary(name: &str, target: u8) -> Worktree {
        checkout(name, target, true)
    }

    fn worktree(name: &str, target: u8) -> Worktree {
        checkout(name, target, false)
    }

    fn branch(name: &str, kind: BranchKind, target: u8) -> Branch {
        Branch {
            name: name.into(),
            kind,
            target: oid(target),
            upstream: None,
            divergence: None,
            is_head: false,
        }
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

    /// 1 root, 2 the commit `dev` was at when the branch was cut, 3 a later
    /// `dev` commit, 4 and 5 the branch's own work.
    fn forked_history() -> Vec<Commit> {
        vec![
            commit(5, &[4]),
            commit(4, &[2]),
            commit(3, &[2]),
            commit(2, &[1]),
            commit(1, &[]),
        ]
    }

    /// The merge history the trunk tests share: 5 merges the branch at 3 into
    /// `dev`, and 1 is where `dev` itself was cut from `master`.
    fn merged_history() -> Vec<Commit> {
        vec![
            commit(5, &[4, 3]),
            commit(4, &[2]),
            commit(3, &[2]),
            commit(2, &[1]),
            commit(1, &[0]),
            commit(0, &[]),
        ]
    }

    #[test]
    fn a_branch_stops_at_the_commit_it_was_cut_from() {
        let snap = snapshot_with(
            forked_history(),
            vec![primary("dev", 2), worktree("feature", 5)],
        );
        let ancestry = Ancestry::of(&snap, oid(5));

        // 2 anchors the highlight to the trunk; 1 is history the branch did
        // not write.
        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 2]);
        assert_eq!(ancestry.count(), 3);
    }

    #[test]
    fn the_parent_branch_moving_on_does_not_move_the_cut() {
        // `dev` is now at 3, but the branch was cut at 2 and that is still
        // where its work begins.
        let snap = snapshot_with(
            forked_history(),
            vec![primary("dev", 3), worktree("feature", 5)],
        );
        let ancestry = Ancestry::of(&snap, oid(5));

        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 2]);
    }

    #[test]
    fn a_branch_left_behind_on_the_line_does_not_cut_it() {
        // `origin/feature` two commits back, and a branch nobody checked out
        // sitting on the trunk. Neither is a line of work, so neither ends
        // one.
        let snap = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            forked_history(),
            vec![
                branch("feature", BranchKind::Local, 5),
                branch("origin/feature", BranchKind::Remote, 4),
                branch("abandoned", BranchKind::Local, 1),
            ],
            vec![primary("dev", 3), worktree("feature", 5)],
            false,
        );
        let ancestry = Ancestry::of(&snap, oid(5));

        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 2]);
    }

    #[test]
    fn a_line_cut_from_ours_does_not_cut_ours() {
        // 5 was branched off 4, which is our tip. It descends from us, so it
        // says nothing about where our work started.
        let snap = snapshot_with(
            forked_history(),
            vec![
                primary("dev", 2),
                worktree("feature", 4),
                worktree("child", 5),
            ],
        );
        let ancestry = Ancestry::of(&snap, oid(4));

        assert_eq!(covered(&snap, &ancestry), vec![4, 2]);
    }

    #[test]
    fn the_primary_checkout_keeps_its_whole_history() {
        // Every other line was cut from this one, so there is nothing above it
        // to stop at.
        let snap = snapshot_with(
            merged_history(),
            vec![primary("dev", 5), worktree("feature", 3)],
        );
        let ancestry = Ancestry::of(&snap, oid(5));

        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn work_merged_into_a_line_stays_lit() {
        // `dev` is checked out away from the primary here. 3 arrived through
        // the merge at 5, so it is part of this line's work even though the
        // branch it came from is a line of its own.
        let snap = snapshot_with(
            merged_history(),
            vec![
                primary("master", 1),
                worktree("dev", 5),
                worktree("feature", 3),
            ],
        );
        let ancestry = Ancestry::of(&snap, oid(5));

        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 3, 2]);
    }

    /// `dev` at 3 merged the branch at 2, which was cut from `dev` at 1.
    fn merged_back_history() -> Vec<Commit> {
        vec![
            commit(3, &[1, 2]),
            commit(2, &[1]),
            commit(1, &[0]),
            commit(0, &[]),
        ]
    }

    #[test]
    fn a_branch_already_merged_into_the_trunk_lights_only_its_head() {
        // The trunk descends from this branch now, but it is still the line
        // the branch was cut from — and it already holds everything the branch
        // wrote.
        let snap = snapshot_with(
            merged_back_history(),
            vec![primary("dev", 3), worktree("feature", 2)],
        );
        let ancestry = Ancestry::of(&snap, oid(2));

        assert_eq!(covered(&snap, &ancestry), vec![2]);
    }

    #[test]
    fn a_branch_that_carried_on_after_being_merged_lights_what_came_after() {
        let mut commits = vec![commit(4, &[2])];
        commits.extend(merged_back_history());
        let snap = snapshot_with(commits, vec![primary("dev", 3), worktree("feature", 4)]);
        let ancestry = Ancestry::of(&snap, oid(4));

        assert_eq!(covered(&snap, &ancestry), vec![4, 2]);
    }

    #[test]
    fn a_tip_no_other_line_touches_still_sees_its_whole_history() {
        let snap = snapshot_with(forked_history(), vec![worktree("feature", 5)]);
        let ancestry = Ancestry::of(&snap, oid(5));

        assert_eq!(covered(&snap, &ancestry), vec![5, 4, 2, 1]);
    }

    #[test]
    fn a_worktree_still_standing_where_the_primary_stands_shows_that_history() {
        // Freshly created, nothing committed on it yet: it has no line of its
        // own to show, and one commit alone would read as an error.
        let snap = snapshot_with(
            forked_history(),
            vec![primary("dev", 2), worktree("fresh", 2)],
        );
        let ancestry = Ancestry::of(&snap, oid(2));

        assert_eq!(covered(&snap, &ancestry), vec![2, 1]);
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
