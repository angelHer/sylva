//! Commit graph layout: turning a linear list of commits into lanes and the
//! line segments that connect them.
//!
//! # The model
//!
//! A lane is a vertical track that is *waiting for* one specific commit. Walking
//! newest to oldest:
//!
//! - Reaching a commit that lanes are waiting for places it in the leftmost of
//!   those lanes; the others converge into it and are freed.
//! - A commit nothing waits for is a tip: it takes the leftmost free lane.
//! - The first parent continues in the commit's own lane, keeping each branch
//!   line straight. Additional parents (a merge) branch off into other lanes.
//!
//! # Why segments and not lane positions
//!
//! A lane can carry more than one incoming line at the same row: its own
//! continuation from above *plus* a merge diagonal arriving from another lane.
//! Storing "which lane is this commit in" cannot express that, which is why
//! every line crossing every row gap is recorded explicitly as a [`Segment`].
//!
//! # Cost
//!
//! One pass over the commits, O(lanes) work per commit. The result is indexed
//! in step with `snapshot.commits()`, so the renderer can slice the visible
//! viewport without touching the rest.

use std::collections::HashSet;

use super::oid::Oid;
use super::snapshot::RepositorySnapshot;

/// Number of distinct lane colours the layout cycles through. The actual
/// neon values live in the UI theme; the domain only decides *which* index a
/// lane gets, so the palette can be restyled without touching this algorithm.
pub const LANE_COLOR_COUNT: usize = 8;

/// One line crossing the gap between two adjacent rows.
///
/// `from_lane` is its horizontal position at the row above, `to_lane` at the
/// row below. Equal values draw a straight vertical line; different values
/// draw a diagonal or curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub from_lane: usize,
    pub to_lane: usize,
    pub color: usize,
}

impl Segment {
    pub const fn is_straight(&self) -> bool {
        self.from_lane == self.to_lane
    }

    /// True when the line moves left as it descends: a branch converging back
    /// into an older line.
    pub const fn is_converging(&self) -> bool {
        self.to_lane < self.from_lane
    }

    /// True when the line moves right as it descends: a branch splitting off.
    pub const fn is_diverging(&self) -> bool {
        self.to_lane > self.from_lane
    }
}

/// One commit's placement in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub commit: Oid,
    /// Horizontal track this commit's node sits on.
    pub lane: usize,
    /// Index into the lane palette.
    pub color: usize,
    pub is_merge: bool,
    /// A parent of this commit is missing from the snapshot because the walk
    /// was truncated. The renderer should fade the line out rather than
    /// pretend history ends here.
    pub has_dangling_parent: bool,
    /// Every line crossing the gap between the previous row and this one.
    /// Empty for the first row.
    pub incoming: Vec<Segment>,
}

/// The laid-out graph, in step with the snapshot's commit list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphLayout {
    rows: Vec<GraphRow>,
    lane_count: usize,
    trailing: Vec<Segment>,
}

/// A lane in flight, waiting for one commit.
#[derive(Debug, Clone, Copy)]
struct LaneSlot {
    awaiting: Oid,
    color: usize,
}

impl GraphLayout {
    /// Lays out the snapshot's commits.
    ///
    /// Assumes `snapshot.commits()` is newest-first and topologically
    /// consistent — every commit appears before its parents. That is what the
    /// `git2` adapter's revwalk produces.
    pub fn build(snapshot: &RepositorySnapshot) -> Self {
        let mut lanes: Vec<Option<LaneSlot>> = Vec::new();
        let mut rows: Vec<GraphRow> = Vec::with_capacity(snapshot.commit_count());
        let mut palette = ColorCycle::default();

        // Lines leaving the row just processed. They become the next row's
        // `incoming` once we know where that row's commit lands.
        let mut pending: Vec<Segment> = Vec::new();
        let mut lane_count = 0usize;

        for commit in snapshot.commits() {
            let claimed = lanes_awaiting(&lanes, &commit.id);

            // The leftmost waiting lane wins, so branch lines drift left and
            // the graph stays narrow.
            let (lane, color) = match claimed.first() {
                Some(&first) => (
                    first,
                    lanes[first].as_ref().expect("claimed lane is occupied").color,
                ),
                None => (allocate_lane(&mut lanes), palette.take()),
            };

            // Every line that was waiting for this commit now ends at its node.
            let mut incoming = std::mem::take(&mut pending);
            for segment in &mut incoming {
                if claimed.contains(&segment.to_lane) {
                    segment.to_lane = lane;
                }
            }

            for &index in &claimed {
                lanes[index] = None;
            }

            let mut has_dangling_parent = false;
            // Lanes this commit (re)assigned: their lines start at its node.
            let mut assigned: Vec<usize> = Vec::new();
            // Merge diagonals into lanes that already existed.
            let mut merge_links: Vec<Segment> = Vec::new();
            let mut seen_parents = HashSet::new();
            let mut continued = false;

            for parent in &commit.parents {
                // A commit can legally list the same parent twice; it is still
                // a single line.
                if !seen_parents.insert(*parent) {
                    continue;
                }

                // The parent was never loaded: the walk stopped short of it.
                // No lane is opened, so truncation cannot leak lanes that wait
                // forever.
                if !snapshot.contains(parent) {
                    has_dangling_parent = true;
                    continue;
                }

                if !continued {
                    // First parent keeps this commit's own lane and colour, so
                    // a branch reads as one continuous line.
                    lanes[lane] = Some(LaneSlot {
                        awaiting: *parent,
                        color,
                    });
                    assigned.push(lane);
                    continued = true;
                    continue;
                }

                match lane_awaiting(&lanes, parent) {
                    // Another line is already heading for this parent; join it
                    // rather than opening a duplicate lane.
                    Some(existing) => {
                        let existing_color =
                            lanes[existing].as_ref().expect("occupied lane").color;
                        merge_links.push(Segment {
                            from_lane: lane,
                            to_lane: existing,
                            color: existing_color,
                        });
                    }
                    None => {
                        let new_lane = allocate_lane(&mut lanes);
                        lanes[new_lane] = Some(LaneSlot {
                            awaiting: *parent,
                            color: palette.take(),
                        });
                        assigned.push(new_lane);
                    }
                }
            }

            pending = Vec::with_capacity(lanes.len() + merge_links.len());
            for (index, slot) in lanes.iter().enumerate() {
                let Some(slot) = slot else { continue };
                pending.push(Segment {
                    // A lane this commit assigned starts at the commit's node;
                    // any other lane simply passes through.
                    from_lane: if assigned.contains(&index) { lane } else { index },
                    to_lane: index,
                    color: slot.color,
                });
            }
            pending.extend(merge_links);

            let width = lanes.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
            lane_count = lane_count.max(width).max(lane + 1);

            rows.push(GraphRow {
                commit: commit.id,
                lane,
                color,
                is_merge: commit.is_merge(),
                has_dangling_parent,
                incoming,
            });
        }

        Self {
            rows,
            lane_count,
            trailing: pending,
        }
    }

    pub fn rows(&self) -> &[GraphRow] {
        &self.rows
    }

    pub fn row(&self, index: usize) -> Option<&GraphRow> {
        self.rows.get(index)
    }

    /// Rows for a viewport, clamped to what exists. This is the only slice the
    /// renderer touches per frame.
    pub fn rows_in_range(&self, start: usize, end: usize) -> &[GraphRow] {
        let start = start.min(self.rows.len());
        let end = end.clamp(start, self.rows.len());
        &self.rows[start..end]
    }

    /// Widest point of the graph, in lanes. Drives the horizontal space the
    /// graph column needs.
    pub const fn lane_count(&self) -> usize {
        self.lane_count
    }

    /// Lines still open below the last row, because history was truncated.
    /// The renderer fades these out at the bottom edge.
    pub fn trailing(&self) -> &[Segment] {
        &self.trailing
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Cycles through the lane palette. Colours repeat once the palette is
/// exhausted; with eight colours, two lanes sharing one are far enough apart
/// to stay readable.
#[derive(Debug, Default)]
struct ColorCycle {
    issued: usize,
}

impl ColorCycle {
    fn take(&mut self) -> usize {
        let color = self.issued % LANE_COLOR_COUNT;
        self.issued += 1;
        color
    }
}

fn lanes_awaiting(lanes: &[Option<LaneSlot>], oid: &Oid) -> Vec<usize> {
    lanes
        .iter()
        .enumerate()
        .filter(|(_, slot)| matches!(slot, Some(slot) if slot.awaiting == *oid))
        .map(|(index, _)| index)
        .collect()
}

fn lane_awaiting(lanes: &[Option<LaneSlot>], oid: &Oid) -> Option<usize> {
    lanes
        .iter()
        .position(|slot| matches!(slot, Some(slot) if slot.awaiting == *oid))
}

/// Leftmost free lane, reusing a slot freed by a finished branch before
/// widening the graph.
fn allocate_lane(lanes: &mut Vec<Option<LaneSlot>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(index) => index,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
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

    /// Builds a snapshot from commits given newest-first.
    fn layout_of(commits: Vec<Commit>) -> GraphLayout {
        let snapshot = RepositorySnapshot::new(
            PathBuf::from("/repo"),
            commits,
            vec![],
            vec![],
            false,
        );
        GraphLayout::build(&snapshot)
    }

    fn lanes(layout: &GraphLayout) -> Vec<usize> {
        layout.rows().iter().map(|row| row.lane).collect()
    }

    #[test]
    fn an_empty_history_lays_out_to_nothing() {
        let layout = layout_of(vec![]);
        assert!(layout.is_empty());
        assert_eq!(layout.lane_count(), 0);
        assert!(layout.trailing().is_empty());
    }

    #[test]
    fn a_single_root_commit_occupies_one_lane_with_no_lines() {
        let layout = layout_of(vec![commit(1, &[])]);

        assert_eq!(layout.len(), 1);
        assert_eq!(layout.lane_count(), 1);
        assert_eq!(layout.row(0).unwrap().lane, 0);
        assert!(layout.row(0).unwrap().incoming.is_empty());
        assert!(layout.trailing().is_empty());
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);

        assert_eq!(lanes(&layout), vec![0, 0, 0]);
        assert_eq!(layout.lane_count(), 1);
    }

    #[test]
    fn linear_history_draws_one_straight_line_between_each_row() {
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);

        assert!(layout.row(0).unwrap().incoming.is_empty());
        for index in 1..3 {
            let incoming = &layout.row(index).unwrap().incoming;
            assert_eq!(incoming.len(), 1, "row {index}");
            assert!(incoming[0].is_straight(), "row {index}");
        }
    }

    #[test]
    fn a_whole_linear_branch_shares_one_colour() {
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);

        let colors: HashSet<usize> = layout.rows().iter().map(|row| row.color).collect();
        assert_eq!(colors.len(), 1);
    }

    #[test]
    fn a_merge_opens_a_second_lane_and_both_sides_converge_on_the_shared_parent() {
        // D is a merge of B and C, which both descend from A.
        let layout = layout_of(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);

        assert_eq!(lanes(&layout), vec![0, 0, 1, 0]);
        assert_eq!(layout.lane_count(), 2);

        let merge = layout.row(0).unwrap();
        assert!(merge.is_merge);

        // The shared root receives the second branch converging back into it.
        let root = layout.row(3).unwrap();
        assert!(
            root.incoming.iter().any(|s| s.is_converging()),
            "expected a converging line into the shared parent, got {:?}",
            root.incoming
        );
    }

    #[test]
    fn a_merge_sends_a_diverging_line_into_its_second_parents_lane() {
        let layout = layout_of(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);

        // The gap below the merge carries the line that opens lane 1.
        let below_merge = &layout.row(1).unwrap().incoming;
        assert!(
            below_merge.iter().any(|s| s.is_diverging()),
            "expected a diverging line out of the merge, got {below_merge:?}"
        );
    }

    #[test]
    fn the_second_parent_of_a_merge_gets_its_own_colour() {
        let layout = layout_of(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);

        let merge_color = layout.row(0).unwrap().color;
        let side_color = layout.row(2).unwrap().color;
        assert_ne!(merge_color, side_color);
    }

    #[test]
    fn an_octopus_merge_opens_a_lane_per_parent() {
        let layout = layout_of(vec![
            commit(5, &[2, 3, 4]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(4, &[1]),
            commit(1, &[]),
        ]);

        assert_eq!(layout.lane_count(), 3);
        assert!(layout.row(0).unwrap().is_merge);
    }

    #[test]
    fn a_duplicated_parent_is_drawn_as_a_single_line() {
        // `git commit-tree -p X -p X` is legal and must not open two lanes.
        let layout = layout_of(vec![commit(2, &[1, 1]), commit(1, &[])]);

        assert_eq!(layout.lane_count(), 1);
        assert_eq!(layout.row(1).unwrap().incoming.len(), 1);
    }

    #[test]
    fn independent_roots_reuse_the_freed_lane() {
        // Two unrelated histories: the first ends before the second starts, so
        // the graph must not widen.
        let layout = layout_of(vec![commit(2, &[]), commit(1, &[])]);

        assert_eq!(lanes(&layout), vec![0, 0]);
        assert_eq!(layout.lane_count(), 1);
    }

    #[test]
    fn a_lane_freed_by_a_finished_branch_is_reused_by_a_later_tip() {
        // C sits on its own lane, ends, and D — an unrelated tip appearing
        // later — takes the lane back instead of opening a third.
        let layout = layout_of(vec![
            commit(4, &[2, 3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
            commit(5, &[]),
        ]);

        assert_eq!(layout.lane_count(), 2);
        assert_eq!(layout.row(4).unwrap().lane, 0);
    }

    #[test]
    fn a_parent_outside_the_snapshot_is_flagged_and_opens_no_lane() {
        // History truncated below commit 2: its parent was never loaded.
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1])]);

        let last = layout.row(1).unwrap();
        assert!(last.has_dangling_parent);
        assert!(
            layout.trailing().is_empty(),
            "a missing parent must not leave a lane waiting forever"
        );
    }

    #[test]
    fn a_present_parent_is_not_flagged_as_dangling() {
        let layout = layout_of(vec![commit(2, &[1]), commit(1, &[])]);

        assert!(layout.rows().iter().all(|row| !row.has_dangling_parent));
    }

    #[test]
    fn every_row_is_reachable_by_index_and_matches_its_commit() {
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);

        assert_eq!(layout.row(0).unwrap().commit, oid(3));
        assert_eq!(layout.row(2).unwrap().commit, oid(1));
        assert!(layout.row(3).is_none());
    }

    #[test]
    fn a_viewport_slice_is_clamped_to_the_rows_that_exist() {
        let layout = layout_of(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);

        assert_eq!(layout.rows_in_range(1, 2).len(), 1);
        assert_eq!(layout.rows_in_range(0, 99).len(), 3);
        assert_eq!(layout.rows_in_range(10, 20).len(), 0);
        // An inverted range yields nothing rather than panicking.
        assert_eq!(layout.rows_in_range(2, 1).len(), 0);
    }

    #[test]
    fn every_colour_stays_within_the_palette() {
        // More concurrent branches than the palette has colours.
        let mut commits = vec![commit(100, &(1..=12).collect::<Vec<u8>>())];
        for id in 1..=12u8 {
            commits.push(commit(id, &[]));
        }
        let layout = layout_of(commits);

        assert!(layout
            .rows()
            .iter()
            .all(|row| row.color < LANE_COLOR_COUNT));
        assert!(layout
            .rows()
            .iter()
            .flat_map(|row| &row.incoming)
            .all(|segment| segment.color < LANE_COLOR_COUNT));
    }

    #[test]
    fn no_segment_ever_points_outside_the_reported_width() {
        let layout = layout_of(vec![
            commit(6, &[4, 5]),
            commit(4, &[2, 3]),
            commit(5, &[3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);

        let width = layout.lane_count();
        for (index, row) in layout.rows().iter().enumerate() {
            assert!(row.lane < width, "row {index} lane {} >= {width}", row.lane);
            for segment in &row.incoming {
                assert!(segment.from_lane < width, "row {index}: {segment:?}");
                assert!(segment.to_lane < width, "row {index}: {segment:?}");
            }
        }
    }

    #[test]
    fn every_line_leaving_a_row_arrives_at_the_next_one() {
        // The gap between two rows must be described consistently: nothing is
        // drawn from nowhere, and nothing simply disappears.
        let layout = layout_of(vec![
            commit(6, &[4, 5]),
            commit(4, &[2, 3]),
            commit(5, &[3]),
            commit(2, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ]);

        for index in 1..layout.len() {
            let row = layout.row(index).unwrap();
            assert!(
                row.incoming.iter().any(|s| s.to_lane == row.lane),
                "row {index} has no line arriving at its own lane"
            );
        }
    }
}
