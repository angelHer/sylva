//! The commit graph, drawn.
//!
//! Only the rows inside the viewport are painted. On a 50,000-commit
//! repository that is about thirty rows per frame regardless of how far down
//! the user has scrolled, which is what keeps this at sixty frames per second.

use eframe::egui::{
    self, Color32, FontId, Painter, Pos2, Rect, Rounding, Sense, Stroke, Ui, Vec2,
};
use eframe::epaint::CubicBezierShape;

use super::theme::{focus_aware, lane_color, lane_glow, Palette};
use crate::domain::{
    Ancestry, Branch, GraphLayout, GraphRow, Oid, RepositorySnapshot, Segment, Worktree,
};

/// Height of one commit row. Everything vertical is derived from this.
pub const ROW_HEIGHT: f32 = 26.0;

const LANE_WIDTH: f32 = 15.0;
const GRAPH_PADDING: f32 = 14.0;
const NODE_RADIUS: f32 = 4.5;
const LINE_WIDTH: f32 = 1.7;
const CHIP_HEIGHT: f32 = 15.0;
const CHIP_GAP: f32 = 5.0;
/// The share of a row that chips may take before the rest is summarised.
///
/// A commit can carry a dozen refs, and without a limit they squeeze the
/// message down to an ellipsis — hiding the one thing every row must show.
const CHIP_BUDGET: f32 = 0.5;
/// Horizontal room a chip's mark needs.
const MARK_SPACE: f32 = 11.0;
/// Padding either side of the rule dividing a chip's name from its remote.
const DIVIDER_PAD: f32 = 6.0;
/// How far the dividing rule stops short of the chip's edges, so it reads as a
/// separator inside the chip rather than as a second border.
const DIVIDER_INSET: f32 = 3.0;
/// The box a chip's mark is drawn inside.
const MARK_SIZE: f32 = 8.0;
/// The narrowest a chip may be drawn: a dot, a glimpse of the name, and the
/// ellipsis that admits the rest was cut.
const MIN_CHIP_WIDTH: f32 = 46.0;

/// Horizontal space the graph column needs for a given width in lanes.
pub fn graph_column_width(lane_count: usize) -> f32 {
    GRAPH_PADDING + lane_count.max(1) as f32 * LANE_WIDTH + GRAPH_PADDING
}

pub struct GraphView<'a> {
    pub snapshot: &'a RepositorySnapshot,
    pub layout: &'a GraphLayout,
    /// When set, only this history is drawn at full strength.
    pub focus: Option<&'a Ancestry>,
    /// A row to bring into view on this frame, taken once and then cleared.
    pub scroll_to: Option<usize>,
}

impl GraphView<'_> {
    pub fn show(&self, ui: &mut Ui, selected: &mut Option<Oid>) {
        if self.layout.is_empty() {
            self.show_empty(ui);
            return;
        }

        let branch_tips = self.snapshot.branch_tips();
        let worktree_anchors = self.snapshot.worktree_anchors();
        let graph_width = graph_column_width(self.layout.lane_count());
        let total_rows = self.layout.len();

        // `show_rows` derives everything from `row_height + item_spacing.y`:
        // how many rows fit, which ones are visible, and how tall the scroll
        // content is. The theme's vertical spacing would therefore make egui
        // reserve more per row than this view paints, cutting rows off the
        // bottom and desynchronising the scrollbar. Rows carry their own
        // padding, so the gap between them is zero.
        ui.spacing_mut().item_spacing.y = 0.0;

        let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false, false]);

        // Centre the requested row rather than putting it at the top edge, so
        // the commits around it stay visible and the jump keeps its context.
        if let Some(row) = self.scroll_to {
            let centred = row as f32 * ROW_HEIGHT - ui.available_height() / 2.0;
            scroll_area = scroll_area.vertical_scroll_offset(centred.max(0.0));
        }

        scroll_area
            .show_rows(ui, ROW_HEIGHT, total_rows, |ui, range| {
                let width = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(width, range.len() as f32 * ROW_HEIGHT),
                    Sense::hover(),
                );
                let painter = ui.painter().clone();

                let lane_x = |lane: usize| {
                    rect.left() + GRAPH_PADDING + lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0
                };

                // A line belongs to the focused history when the commit that
                // opened it does. Judging it by where it is heading would light
                // up every branch in the repository, since they all converge on
                // a shared ancestor the focused worktree can see.
                let segment_in_focus = |segment: &Segment| {
                    self.focus.is_none_or(|ancestry| {
                        self.snapshot
                            .position_of(&segment.source)
                            .is_some_and(|position| ancestry.includes_position(position))
                    })
                };

                for (offset, index) in range.clone().enumerate() {
                    let row = self.layout.row(index).expect("row inside range");
                    let top = rect.top() + offset as f32 * ROW_HEIGHT;
                    let center_y = top + ROW_HEIGHT / 2.0;
                    let row_rect =
                        Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(width, ROW_HEIGHT));

                    let response = ui.interact(row_rect, ui.id().with(index), Sense::click());
                    if response.clicked() {
                        *selected = Some(row.commit);
                    }

                    if selected.as_ref() == Some(&row.commit) {
                        painter.rect_filled(row_rect, Rounding::ZERO, Palette::SELECTED);
                    } else if response.hovered() {
                        painter.rect_filled(row_rect, Rounding::ZERO, Palette::HOVER);
                    }

                    // A row is in focus when no worktree is focused at all, or
                    // when the focused worktree can see this commit.
                    let in_focus = self
                        .focus
                        .is_none_or(|ancestry| ancestry.includes_position(index));

                    // Lines arriving from the row above.
                    for segment in &row.incoming {
                        draw_segment(
                            &painter,
                            segment,
                            &lane_x,
                            center_y - ROW_HEIGHT,
                            center_y,
                            segment_in_focus(segment),
                        );
                    }

                    draw_node(&painter, row, lane_x(row.lane), center_y, in_focus);

                    let mut x = rect.left() + graph_width;
                    x += draw_hash(&painter, row, x, center_y, in_focus);

                    let chip_limit = x + (rect.right() - x) * CHIP_BUDGET;
                    let mut hidden = 0usize;

                    // Worktree markers come first: they are the point of this
                    // client, and a branch chip must never push one out. They
                    // are shortened rather than dropped when the budget runs
                    // out, so a repository whose worktree names are long still
                    // leaves room for the summary.
                    if let Some(worktrees) = worktree_anchors.get(&row.commit) {
                        for worktree in worktrees {
                            let color = focus_aware(worktree_color(worktree), in_focus);
                            x += chip(
                                &painter,
                                x,
                                center_y,
                                ChipContent {
                                    text: worktree.dir_name(),
                                    // The mark is what separates a worktree
                                    // from a branch at a glance — the two often
                                    // carry nearly the same name on the same
                                    // row — and its colour still carries the
                                    // state.
                                    mark: Some(ChipMark::Directory(color)),
                                    remote: None,
                                },
                                color,
                                worktree_chip_width(x, chip_limit),
                            );
                        }
                    }

                    if let Some(branches) = branch_tips.get(&row.commit) {
                        for branch in branch_chips(branches) {
                            // A branch that does not fit whole is worth more as
                            // part of the "+N" count than as a stub: the name is
                            // the only thing it carries.
                            let color = focus_aware(Palette::CYAN, in_focus);
                            let remote = branch.remote.as_deref();
                            let width = chip_width(&painter, &branch.name, remote, true);
                            if !branch_chip_fits(x, width, chip_limit) {
                                hidden += 1;
                                continue;
                            }
                            x += chip(
                                &painter,
                                x,
                                center_y,
                                ChipContent {
                                    text: &branch.name,
                                    mark: Some(ChipMark::Branch(color)),
                                    remote,
                                },
                                color,
                                width,
                            );
                        }
                    }

                    if hidden > 0 {
                        let label = format!("+{hidden}");
                        let width = chip_width(&painter, &label, None, false);
                        x += chip(
                            &painter,
                            x,
                            center_y,
                            ChipContent {
                                text: &label,
                                mark: None,
                                remote: None,
                            },
                            focus_aware(Palette::TEXT_DIM, in_focus),
                            width,
                        );
                    }

                    draw_summary(
                        &painter,
                        self.snapshot,
                        row,
                        x,
                        center_y,
                        rect.right(),
                        in_focus,
                    );
                }

                // The row just below the viewport is not painted, so its
                // incoming lines would leave a gap at the bottom edge while
                // scrolling. Draw them here.
                if let Some(next) = self.layout.row(range.end) {
                    let bottom = rect.bottom();
                    for segment in &next.incoming {
                        draw_segment(
                            &painter,
                            segment,
                            &lane_x,
                            bottom - ROW_HEIGHT / 2.0,
                            bottom + ROW_HEIGHT / 2.0,
                            segment_in_focus(segment),
                        );
                    }
                }
            });
    }

    fn show_empty(&self, ui: &mut Ui) {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("No commits yet")
                    .color(Palette::TEXT_FAINT)
                    .size(14.0),
            );
        });
    }
}

fn draw_segment(
    painter: &Painter,
    segment: &Segment,
    lane_x: &impl Fn(usize) -> f32,
    top_y: f32,
    bottom_y: f32,
    in_focus: bool,
) {
    let from = Pos2::new(lane_x(segment.from_lane), top_y);
    let to = Pos2::new(lane_x(segment.to_lane), bottom_y);
    let stroke = Stroke::new(LINE_WIDTH, focus_aware(lane_color(segment.color), in_focus));

    if segment.is_straight() {
        painter.line_segment([from, to], stroke);
        return;
    }

    // Control points pulled vertically, never horizontally: the curve leaves
    // and enters each lane straight down, so a line reads as belonging to its
    // lane rather than drifting between two.
    let pull = (bottom_y - top_y) * 0.5;
    painter.add(CubicBezierShape::from_points_stroke(
        [
            from,
            Pos2::new(from.x, from.y + pull),
            Pos2::new(to.x, to.y - pull),
            to,
        ],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn draw_node(painter: &Painter, row: &GraphRow, x: f32, y: f32, in_focus: bool) {
    let center = Pos2::new(x, y);
    let color = focus_aware(lane_color(row.color), in_focus);

    // A soft halo behind the node is what sells the neon look; it also
    // separates the node from any line passing behind it. Out of focus there
    // is no halo at all, which is most of what makes the focused history pop.
    if in_focus {
        painter.circle_filled(center, NODE_RADIUS * 1.7, lane_glow(row.color));
    }

    if row.is_merge {
        // Hollow, so merges are findable at a glance while scrolling.
        painter.circle_filled(center, NODE_RADIUS, Palette::BACKDROP);
        painter.circle_stroke(center, NODE_RADIUS, Stroke::new(2.0_f32, color));
    } else {
        painter.circle_filled(center, NODE_RADIUS, color);
    }
}

fn draw_hash(painter: &Painter, row: &GraphRow, x: f32, center_y: f32, in_focus: bool) -> f32 {
    let color = focus_aware(Palette::TEXT_FAINT, in_focus);
    let galley = painter.layout_no_wrap(row.commit.to_short_hex(8), FontId::monospace(11.0), color);
    let size = galley.size();
    painter.galley(Pos2::new(x, center_y - size.y / 2.0), galley, color);
    size.x + 10.0
}

/// The colour a worktree marker is drawn in: stale first, then dirty, then
/// clean.
fn worktree_color(worktree: &Worktree) -> Color32 {
    if worktree.is_prunable {
        Palette::DANGER
    } else if worktree.status.is_dirty() {
        Palette::DIRTY
    } else {
        Palette::OK
    }
}

/// The mark drawn at the head of a chip, saying what kind of thing it names.
///
/// Shapes rather than characters, for the reason the plain status dot always
/// was one: the bundled font has no glyph for a folder or a branch, and a
/// missing glyph renders as an empty box.
#[derive(Clone, Copy)]
enum ChipMark {
    /// A worktree: somewhere on disk. Its colour carries the state of that
    /// directory, which is the job the bare dot used to do alone.
    Directory(Color32),
    /// A branch: a name for a commit, which is not a place.
    Branch(Color32),
}

/// Body and tab of the folder mark.
fn folder_parts(center: Pos2, size: f32) -> (Rect, Rect) {
    let half = size / 2.0;
    let tab_height = size * 0.25;
    let body = Rect::from_min_max(
        Pos2::new(center.x - half, center.y - half + tab_height),
        Pos2::new(center.x + half, center.y + half),
    );
    let tab = Rect::from_min_max(
        Pos2::new(center.x - half, center.y - half),
        Pos2::new(center.x - half + size * 0.45, center.y - half + tab_height),
    );
    (body, tab)
}

/// Root, tip and fork of the branch mark: a trunk with one branch leaving it.
fn branch_parts(center: Pos2, size: f32) -> (Pos2, Pos2, Pos2) {
    let half = size / 2.0;
    let trunk_x = center.x - half + 1.5;
    (
        Pos2::new(trunk_x, center.y + half),
        Pos2::new(trunk_x, center.y - half),
        Pos2::new(center.x + half, center.y - half + 1.5),
    )
}

fn draw_mark(painter: &Painter, mark: ChipMark, center: Pos2) {
    match mark {
        ChipMark::Directory(color) => {
            let (body, tab) = folder_parts(center, MARK_SIZE);
            painter.rect_filled(body, Rounding::same(1.0_f32), color);
            painter.rect_filled(tab, Rounding::same(1.0_f32), color);
        }
        ChipMark::Branch(color) => {
            let (root, tip, fork) = branch_parts(center, MARK_SIZE);
            let stroke = Stroke::new(1.2_f32, color);
            painter.line_segment([root, tip], stroke);

            // The branch leaves the trunk on a curve, the same way a lane does
            // in the graph beside it. Both control points sit at the corner, so
            // it turns once rather than bulging back and closing into a loop.
            let corner = Pos2::new(root.x, fork.y);
            painter.add(CubicBezierShape::from_points_stroke(
                [Pos2::new(root.x, center.y), corner, corner, fork],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));

            // Three nodes, as the branch glyph is drawn everywhere else. With
            // only two it reads as a letter rather than a branch.
            for node in [root, tip, fork] {
                painter.circle_filled(node, 1.3, color);
            }
        }
    }
}

/// One branch chip: a name, and the remotes that agree with it on this row.
///
/// A local branch and its remote counterpart sitting on the same commit are one
/// fact, not two, so they share a chip. When they part company each lands on its
/// own row, and naming the remote there is what tells the reader which side of
/// the split they are looking at without hunting for the other row.
#[derive(Debug, PartialEq, Eq)]
struct BranchChip {
    name: String,
    /// The remotes pointing here, joined for display; `None` when only a local
    /// branch does.
    remote: Option<String>,
}

/// Groups one row's branches so a name shared by a local branch and its remotes
/// collapses into a single chip.
fn branch_chips(branches: &[&Branch]) -> Vec<BranchChip> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();

    for branch in branches {
        // A remote ref carrying no remote prefix cannot be attributed to one, so
        // it gets a group of its own: nothing says it belongs with a local
        // branch whose name it only appears to share.
        let Some(remote) = branch.remote_name() else {
            groups.push((branch.name.clone(), Vec::new()));
            continue;
        };

        let name = branch.short_name();
        let index = match groups.iter().position(|(grouped, _)| grouped == name) {
            Some(index) => index,
            None => {
                groups.push((name.to_owned(), Vec::new()));
                groups.len() - 1
            }
        };
        groups[index].1.push(remote.to_owned());
    }

    groups
        .into_iter()
        .map(|(name, remotes)| BranchChip {
            name,
            remote: (!remotes.is_empty()).then(|| remotes.join(", ")),
        })
        .collect()
}

/// Room the remote half of a chip needs: its rule, the padding either side, and
/// the remote's own name.
fn remote_half_width(painter: &Painter, remote: &str) -> f32 {
    let galley =
        painter.layout_no_wrap(remote.to_owned(), FontId::proportional(10.5), Palette::TEXT);
    DIVIDER_PAD * 2.0 + 1.0 + galley.size().x
}

/// The width a chip wants if nothing constrains it.
fn chip_width(painter: &Painter, text: &str, remote: Option<&str>, has_mark: bool) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(10.5), Palette::TEXT);
    galley.size().x
        + 10.0
        + if has_mark { MARK_SPACE } else { 0.0 }
        + remote.map_or(0.0, |remote| remote_half_width(painter, remote))
}

/// Everything a chip says, as opposed to where and how big it is drawn.
struct ChipContent<'a> {
    text: &'a str,
    mark: Option<ChipMark>,
    /// The remote sharing this commit, drawn in a second half behind a rule.
    remote: Option<&'a str>,
}

/// Draws a rounded label and returns the horizontal space it consumed.
///
/// The label is ellipsised to `max_width` rather than overflowing it, which is
/// what keeps a caller's width budget honest.
fn chip(
    painter: &Painter,
    x: f32,
    center_y: f32,
    content: ChipContent<'_>,
    color: Color32,
    max_width: f32,
) -> f32 {
    let ChipContent { text, mark, remote } = content;
    let dot_space = if mark.is_some() { MARK_SPACE } else { 0.0 };
    // The remote half keeps its full width when space runs short: it is one
    // short word, and an ellipsised "or…" would say nothing at all.
    let remote_space = remote.map_or(0.0, |remote| remote_half_width(painter, remote));

    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        FontId::proportional(10.5),
        color,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (max_width - 10.0 - dot_space - remote_space).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };

    let galley = painter.layout_job(job);
    let name_width = galley.size().x;
    let width = name_width + 10.0 + dot_space + remote_space;
    let rect = Rect::from_min_size(
        Pos2::new(x, center_y - CHIP_HEIGHT / 2.0),
        Vec2::new(width, CHIP_HEIGHT),
    );

    painter.rect_filled(rect, Rounding::same(3.0_f32), Palette::SURFACE);
    painter.rect_stroke(
        rect,
        Rounding::same(3.0_f32),
        Stroke::new(1.0_f32, tint(color, 0.45)),
    );

    if let Some(mark) = mark {
        draw_mark(
            painter,
            mark,
            Pos2::new(x + 5.0 + MARK_SPACE / 2.0, center_y),
        );
    }

    painter.galley(
        Pos2::new(x + 5.0 + dot_space, center_y - galley.size().y / 2.0),
        galley,
        color,
    );

    if let Some(remote) = remote {
        let rule_x = x + 5.0 + dot_space + name_width + DIVIDER_PAD;
        painter.line_segment(
            [
                Pos2::new(rule_x, rect.top() + DIVIDER_INSET),
                Pos2::new(rule_x, rect.bottom() - DIVIDER_INSET),
            ],
            Stroke::new(1.0_f32, tint(color, 0.45)),
        );

        // Dimmer than the branch name: the remote answers "where else is this?",
        // which is the lesser half of what the chip says.
        let remote_color = tint(color, 0.75);
        let galley =
            painter.layout_no_wrap(remote.to_owned(), FontId::proportional(10.5), remote_color);
        painter.galley(
            Pos2::new(rule_x + DIVIDER_PAD + 1.0, center_y - galley.size().y / 2.0),
            galley,
            remote_color,
        );
    }

    width + CHIP_GAP
}

fn draw_summary(
    painter: &Painter,
    snapshot: &RepositorySnapshot,
    row: &GraphRow,
    x: f32,
    center_y: f32,
    right: f32,
    in_focus: bool,
) {
    let Some(commit) = snapshot.commit(&row.commit) else {
        return;
    };

    let available = right - x - 12.0;
    if available < 30.0 {
        return;
    }

    let color = focus_aware(
        if row.has_dangling_parent {
            Palette::TEXT_DIM
        } else {
            Palette::TEXT
        },
        in_focus,
    );

    // One line, ellipsised. A wrapped commit summary would break the fixed row
    // height the virtualization depends on.
    let mut job = egui::text::LayoutJob::simple_singleline(
        commit.summary.clone(),
        FontId::proportional(12.5),
        color,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: available,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };

    let galley = painter.layout_job(job);
    painter.galley(Pos2::new(x, center_y - galley.size().y / 2.0), galley, color);
}

/// How wide a worktree marker starting at `x` may be drawn.
///
/// Worktree markers are never dropped, so once the budget is spent they fall
/// back to a stub rather than disappearing. Before this they ignored the budget
/// altogether, and a repository with long worktree names left the commit
/// summary as a bare ellipsis — the one thing the budget exists to prevent.
fn worktree_chip_width(x: f32, chip_limit: f32) -> f32 {
    (chip_limit - x).max(MIN_CHIP_WIDTH)
}

/// Whether a branch chip of `width` still ends inside the budget.
///
/// Measured against where the chip *ends*: testing its start let the one chip
/// straddling the limit through at full width.
fn branch_chip_fits(x: f32, width: f32, chip_limit: f32) -> bool {
    x + width <= chip_limit
}

/// Blends a colour towards the panel background, for strokes that should read
/// as related to a lane without competing with it.
fn tint(color: Color32, factor: f32) -> Color32 {
    let mix = |channel: u8| (channel as f32 * factor) as u8;
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BranchKind;

    #[test]
    fn the_graph_column_grows_with_the_number_of_lanes() {
        assert!(graph_column_width(4) > graph_column_width(1));
    }

    #[test]
    fn an_empty_graph_still_reserves_a_single_lane_of_space() {
        // Prevents the commit text from sitting flush against the window edge
        // on a repository with no branches yet.
        assert_eq!(graph_column_width(0), graph_column_width(1));
    }

    #[test]
    fn each_extra_lane_costs_a_fixed_width() {
        let step = graph_column_width(3) - graph_column_width(2);
        assert_eq!(step, graph_column_width(9) - graph_column_width(8));
    }

    #[test]
    fn tinting_darkens_every_channel_without_changing_the_hue_order() {
        let base = Color32::from_rgb(200, 100, 50);
        let tinted = tint(base, 0.5);
        assert_eq!((tinted.r(), tinted.g(), tinted.b()), (100, 50, 25));
    }

    #[test]
    fn tinting_by_one_leaves_a_colour_untouched() {
        let base = Color32::from_rgb(0, 229, 255);
        let tinted = tint(base, 1.0);
        assert_eq!((tinted.r(), tinted.g(), tinted.b()), (0, 229, 255));
    }

    #[test]
    fn a_worktree_marker_may_use_what_the_budget_still_allows() {
        assert_eq!(worktree_chip_width(500.0, 800.0), 300.0);
    }

    #[test]
    fn a_worktree_marker_is_shortened_rather_than_dropped_once_the_budget_is_spent() {
        // The marker still has to appear — this client is about worktrees — but
        // a stub is what keeps it from eating the commit summary.
        assert_eq!(worktree_chip_width(900.0, 800.0), MIN_CHIP_WIDTH);
    }

    #[test]
    fn a_worktree_marker_never_shrinks_below_the_stub_width() {
        assert_eq!(worktree_chip_width(790.0, 800.0), MIN_CHIP_WIDTH);
    }

    #[test]
    fn a_branch_chip_fits_when_it_ends_inside_the_budget() {
        assert!(branch_chip_fits(700.0, 100.0, 800.0));
    }

    fn branch(name: &str, kind: BranchKind) -> Branch {
        Branch {
            name: name.into(),
            kind,
            target: Oid::zero(),
            upstream: None,
            divergence: None,
            is_head: false,
        }
    }

    fn chips(branches: &[Branch]) -> Vec<BranchChip> {
        branch_chips(&branches.iter().collect::<Vec<_>>())
    }

    #[test]
    fn a_local_branch_on_its_own_gets_a_chip_with_no_remote_side() {
        assert_eq!(
            chips(&[branch("main", BranchKind::Local)]),
            vec![BranchChip {
                name: "main".into(),
                remote: None,
            }]
        );
    }

    #[test]
    fn a_local_branch_and_its_remote_on_one_commit_share_a_chip() {
        let chips = chips(&[
            branch("main", BranchKind::Local),
            branch("origin/main", BranchKind::Remote),
        ]);
        assert_eq!(
            chips,
            vec![BranchChip {
                name: "main".into(),
                remote: Some("origin".into()),
            }]
        );
    }

    #[test]
    fn a_remote_branch_left_behind_still_names_its_remote() {
        // Its local counterpart is on another row, so this chip is all the
        // reader has to tell them which side of the split they are looking at.
        assert_eq!(
            chips(&[branch("origin/main", BranchKind::Remote)]),
            vec![BranchChip {
                name: "main".into(),
                remote: Some("origin".into()),
            }]
        );
    }

    #[test]
    fn every_remote_agreeing_on_one_commit_is_named() {
        let chips = chips(&[
            branch("main", BranchKind::Local),
            branch("origin/main", BranchKind::Remote),
            branch("upstream/main", BranchKind::Remote),
        ]);
        assert_eq!(
            chips,
            vec![BranchChip {
                name: "main".into(),
                remote: Some("origin, upstream".into()),
            }]
        );
    }

    #[test]
    fn branches_of_different_names_keep_their_own_chips() {
        let chips = chips(&[
            branch("main", BranchKind::Local),
            branch("origin/feature", BranchKind::Remote),
        ]);
        assert_eq!(
            chips,
            vec![
                BranchChip {
                    name: "main".into(),
                    remote: None,
                },
                BranchChip {
                    name: "feature".into(),
                    remote: Some("origin".into()),
                },
            ]
        );
    }

    #[test]
    fn chips_keep_the_order_their_branches_arrived_in() {
        let chips = chips(&[
            branch("origin/feature", BranchKind::Remote),
            branch("main", BranchKind::Local),
        ]);
        assert_eq!(chips[0].name, "feature");
        assert_eq!(chips[1].name, "main");
    }

    #[test]
    fn a_remote_branch_carrying_no_remote_prefix_stands_on_its_own() {
        // `short_name()` hands back the whole name for such a ref, which would
        // merge it into an unrelated local branch that happens to share it.
        let chips = chips(&[
            branch("weird", BranchKind::Local),
            branch("weird", BranchKind::Remote),
        ]);
        assert_eq!(chips.len(), 2);
        assert!(chips
            .iter()
            .all(|chip| chip.name == "weird" && chip.remote.is_none()));
    }

    #[test]
    fn a_folder_mark_wears_its_tab_on_top_of_its_body() {
        let (body, tab) = folder_parts(Pos2::new(50.0, 50.0), 8.0);
        assert_eq!(tab.max.y, body.min.y);
        assert!(tab.width() < body.width());
    }

    #[test]
    fn a_folder_mark_stays_inside_the_space_it_was_given() {
        let (body, tab) = folder_parts(Pos2::new(50.0, 50.0), 8.0);
        let bounds = body.union(tab);
        assert_eq!(bounds.width(), 8.0);
        assert_eq!(bounds.height(), 8.0);
    }

    #[test]
    fn a_branch_mark_forks_upwards_and_to_the_right() {
        let (root, tip, fork) = branch_parts(Pos2::new(50.0, 50.0), 8.0);
        // Screen coordinates grow downwards, so the tip is the smaller y.
        assert!(tip.y < root.y);
        assert_eq!(tip.x, root.x);
        assert!(fork.x > root.x);
        assert!(fork.y < root.y);
    }

    #[test]
    fn a_branch_mark_stays_inside_the_space_it_was_given() {
        let center = Pos2::new(50.0, 50.0);
        let (root, tip, fork) = branch_parts(center, 8.0);
        for point in [root, tip, fork] {
            assert!((point.x - center.x).abs() <= 4.0);
            assert!((point.y - center.y).abs() <= 4.0);
        }
    }

    #[test]
    fn a_branch_chip_that_would_cross_the_budget_is_collapsed() {
        // The old rule tested where the chip *started*, so the one chip that
        // straddled the limit was still drawn at full width.
        assert!(!branch_chip_fits(700.0, 200.0, 800.0));
    }
}
