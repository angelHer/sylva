//! The worktree panel: every checkout of the repository and its state, at a
//! glance. This is the view the client exists for.

use eframe::egui::{self, Color32, RichText, Ui};

use super::theme::Palette;
use crate::domain::{Divergence, Oid, RepositorySnapshot, Worktree, WorktreeStatus};

pub struct WorktreePanel<'a> {
    pub snapshot: &'a RepositorySnapshot,
}

impl WorktreePanel<'_> {
    pub fn show(&self, ui: &mut Ui, selected: &mut Option<Oid>) {
        let worktrees = self.snapshot.worktrees();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("WORKTREES")
                    .color(Palette::TEXT_DIM)
                    .size(10.5)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("{}", worktrees.len()))
                    .color(Palette::CYAN)
                    .size(10.5),
            );
        });

        let needing_attention = self.snapshot.worktrees_needing_attention().len();
        if needing_attention > 0 {
            ui.label(
                RichText::new(format!("{needing_attention} need attention"))
                    .color(Palette::DIRTY)
                    .size(10.5),
            );
        }

        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for worktree in worktrees {
                    self.show_worktree(ui, worktree, selected);
                    ui.add_space(4.0);
                }
            });
    }

    fn show_worktree(&self, ui: &mut Ui, worktree: &Worktree, selected: &mut Option<Oid>) {
        let is_selected = worktree.target().is_some() && worktree.target() == *selected;

        let frame = egui::Frame::none()
            .fill(if is_selected {
                Palette::SELECTED
            } else {
                Palette::SURFACE
            })
            .rounding(egui::Rounding::same(5.0))
            .inner_margin(egui::Margin::symmetric(8.0, 7.0))
            .stroke(egui::Stroke::new(
                1.0_f32,
                if is_selected {
                    Palette::CYAN
                } else {
                    Palette::BORDER
                },
            ));

        let response = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    // The primary checkout is the one the repository itself
                    // sits in; everything else is a linked worktree.
                    if worktree.is_primary {
                        // A drawn marker, not a character: no glyph risk.
                        let (dot, _) = ui.allocate_exact_size(
                            egui::vec2(9.0, 9.0),
                            egui::Sense::hover(),
                        );
                        ui.painter()
                            .circle_filled(dot.center(), 3.5, Palette::CYAN);
                    }
                    ui.label(
                        RichText::new(worktree.dir_name())
                            .color(Palette::TEXT)
                            .size(12.5)
                            .strong(),
                    );
                });

                ui.label(
                    RichText::new(worktree.head.label())
                        .color(if worktree.head.is_detached() {
                            Palette::DIRTY
                        } else {
                            Palette::TEXT_DIM
                        })
                        .size(11.0)
                        .monospace(),
                );

                ui.horizontal_wrapped(|ui| {
                    let (text, color) = status_badge(&worktree.status);
                    ui.label(RichText::new(text).color(color).size(10.5));

                    if let Some(label) = divergence_label(worktree.divergence) {
                        ui.label(RichText::new(label).color(Palette::TEXT_DIM).size(10.5));
                    }

                    if worktree.is_locked {
                        ui.label(RichText::new("locked").color(Palette::TEXT_DIM).size(10.5));
                    }
                    if worktree.is_prunable {
                        ui.label(RichText::new("prunable").color(Palette::DANGER).size(10.5));
                    }
                });
            })
            .response
            .interact(egui::Sense::click());

        // Clicking a worktree selects the commit it sits on, which highlights
        // its position in the shared graph.
        if response.clicked() {
            *selected = worktree.target();
        }
    }
}

/// One-line summary of a working directory's state, with the colour it should
/// be shown in. Conflicts outrank everything else.
fn status_badge(status: &WorktreeStatus) -> (String, Color32) {
    if status.has_conflicts() {
        return (
            format!("{} conflicted", status.conflicted),
            Palette::DANGER,
        );
    }

    if status.is_clean() {
        return ("clean".to_string(), Palette::OK);
    }

    let mut parts = Vec::new();
    if status.staged > 0 {
        parts.push(format!("+{}", status.staged));
    }
    if status.unstaged > 0 {
        parts.push(format!("~{}", status.unstaged));
    }
    if status.untracked > 0 {
        parts.push(format!("?{}", status.untracked));
    }

    (parts.join(" "), Palette::DIRTY)
}

/// Ahead/behind against the upstream, or nothing when there is no upstream or
/// the branch is level with it.
fn divergence_label(divergence: Option<Divergence>) -> Option<String> {
    let divergence = divergence?;
    if divergence.is_in_sync() {
        return None;
    }

    // Spelled out rather than drawn with arrows: the bundled font has no
    // glyph for them, and a missing glyph renders as an empty box.
    let mut parts = Vec::new();
    if divergence.ahead > 0 {
        parts.push(format!("{} ahead", divergence.ahead));
    }
    if divergence.behind > 0 {
        parts.push(format!("{} behind", divergence.behind));
    }

    Some(parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_worktree_reads_as_clean() {
        let (text, color) = status_badge(&WorktreeStatus::default());
        assert_eq!(text, "clean");
        assert_eq!(color, Palette::OK);
    }

    #[test]
    fn a_dirty_worktree_lists_each_kind_of_change() {
        let status = WorktreeStatus {
            staged: 1,
            unstaged: 2,
            untracked: 3,
            conflicted: 0,
        };
        let (text, color) = status_badge(&status);
        assert_eq!(text, "+1 ~2 ?3");
        assert_eq!(color, Palette::DIRTY);
    }

    #[test]
    fn categories_with_no_changes_are_left_out() {
        let status = WorktreeStatus {
            untracked: 4,
            ..Default::default()
        };
        assert_eq!(status_badge(&status).0, "?4");
    }

    #[test]
    fn conflicts_outrank_every_other_change() {
        let status = WorktreeStatus {
            staged: 5,
            unstaged: 5,
            untracked: 5,
            conflicted: 2,
        };
        let (text, color) = status_badge(&status);
        assert_eq!(text, "2 conflicted");
        assert_eq!(color, Palette::DANGER);
    }

    #[test]
    fn a_branch_level_with_its_upstream_shows_nothing() {
        assert_eq!(divergence_label(Some(Divergence::default())), None);
    }

    #[test]
    fn a_branch_without_an_upstream_shows_nothing() {
        assert_eq!(divergence_label(None), None);
    }

    #[test]
    fn being_ahead_and_behind_shows_both_directions() {
        assert_eq!(
            divergence_label(Some(Divergence::new(3, 4))),
            Some("3 ahead, 4 behind".to_string())
        );
    }

    #[test]
    fn being_only_behind_shows_one_direction() {
        assert_eq!(
            divergence_label(Some(Divergence::new(0, 7))),
            Some("7 behind".to_string())
        );
    }
}
