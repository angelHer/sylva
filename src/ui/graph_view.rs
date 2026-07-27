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
    Ancestry, GraphLayout, GraphRow, Oid, RepositorySnapshot, Segment, Worktree,
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
/// Horizontal room a chip's status dot needs.
const DOT_SPACE: f32 = 11.0;

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
                    // client, and a branch chip must never push one out.
                    if let Some(worktrees) = worktree_anchors.get(&row.commit) {
                        for worktree in worktrees {
                            let color = focus_aware(worktree_color(worktree), in_focus);
                            x += chip(
                                &painter,
                                x,
                                center_y,
                                worktree.dir_name(),
                                color,
                                // The dot is what separates a worktree marker
                                // from a branch chip at a glance, and it
                                // carries the state in its colour.
                                Some(color),
                            );
                        }
                    }

                    if let Some(branches) = branch_tips.get(&row.commit) {
                        for branch in branches {
                            if x > chip_limit {
                                hidden += 1;
                                continue;
                            }
                            x += chip(
                                &painter,
                                x,
                                center_y,
                                &branch.name,
                                focus_aware(Palette::CYAN, in_focus),
                                None,
                            );
                        }
                    }

                    if hidden > 0 {
                        x += chip(
                            &painter,
                            x,
                            center_y,
                            &format!("+{hidden}"),
                            focus_aware(Palette::TEXT_DIM, in_focus),
                            None,
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
        painter.circle_filled(center, NODE_RADIUS * 2.4, lane_glow(row.color));
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

/// Draws a rounded label and returns the horizontal space it consumed.
///
/// `dot` draws a filled circle before the text. It is a shape rather than a
/// character on purpose: the bundled font has no glyph for the symbols this
/// would otherwise want, and a missing glyph renders as an empty box.
fn chip(
    painter: &Painter,
    x: f32,
    center_y: f32,
    text: &str,
    color: Color32,
    dot: Option<Color32>,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(10.5), color);
    let dot_space = if dot.is_some() { DOT_SPACE } else { 0.0 };
    let width = galley.size().x + 10.0 + dot_space;
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

    if let Some(dot_color) = dot {
        painter.circle_filled(Pos2::new(x + 8.0, center_y), 3.0, dot_color);
    }

    painter.galley(
        Pos2::new(x + 5.0 + dot_space, center_y - galley.size().y / 2.0),
        galley,
        color,
    );

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

/// Blends a colour towards the panel background, for strokes that should read
/// as related to a lane without competing with it.
fn tint(color: Color32, factor: f32) -> Color32 {
    let mix = |channel: u8| (channel as f32 * factor) as u8;
    Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
