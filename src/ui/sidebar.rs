//! The worktree panel: every checkout of the repository, its state, and the
//! actions that can be taken on it. This is the view the client exists for.

use eframe::egui::{self, Color32, RichText, Ui};

use super::theme::Palette;
use crate::domain::{Divergence, Oid, RepositorySnapshot, Worktree, WorktreeStatus};

/// What the user asked for by clicking in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    /// Highlight only what this commit can see.
    Focus(Oid),
    /// Go back to showing the whole history at full strength.
    ClearFocus,
    /// Open the dialog for creating a worktree.
    NewWorktree,
    /// Drop the records of worktrees whose directories are gone.
    Prune,
    /// Ask before removing. Nothing is deleted yet.
    AskToRemove(String),
    /// Confirmed. `force` means the user accepted losing uncommitted work.
    Remove { dir_name: String, force: bool },
    /// Back out of a removal.
    CancelRemoval,
    /// Open a terminal at this worktree's directory.
    OpenTerminal(String),
}

pub struct WorktreePanel<'a> {
    pub snapshot: &'a RepositorySnapshot,
    /// The tip whose history is currently focused, if any.
    pub focused: Option<Oid>,
    /// The worktree awaiting a removal confirmation, if any.
    pub confirming: Option<&'a str>,
    /// True while an operation is running, so nothing can be started twice.
    pub busy: bool,
}

impl WorktreePanel<'_> {
    pub fn show(&self, ui: &mut Ui, selected: &mut Option<Oid>) -> Option<PanelAction> {
        let mut action = None;
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
        ui.add_enabled_ui(!self.busy, |ui| {
            ui.horizontal(|ui| {
                if ui.button(RichText::new("new").size(10.5)).clicked() {
                    action = Some(PanelAction::NewWorktree);
                }

                // Pruning only drops records for directories that are already
                // gone, so it destroys nothing and needs no confirmation.
                let stale = worktrees.iter().filter(|wt| wt.is_prunable).count();
                if ui
                    .add_enabled(
                        stale > 0,
                        egui::Button::new(RichText::new("prune").size(10.5)),
                    )
                    .on_hover_text(format!("{stale} stale record(s)"))
                    .clicked()
                {
                    action = Some(PanelAction::Prune);
                }
            });
        });

        if self.focused.is_some() {
            ui.add_space(2.0);
            if ui
                .button(
                    RichText::new("show all history")
                        .color(Palette::CYAN)
                        .size(10.5),
                )
                .clicked()
            {
                action = Some(PanelAction::ClearFocus);
            }
        }

        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for worktree in worktrees {
                    if let Some(requested) = self.show_worktree(ui, worktree, selected) {
                        action = Some(requested);
                    }
                    ui.add_space(4.0);
                }
            });

        action
    }

    fn show_worktree(
        &self,
        ui: &mut Ui,
        worktree: &Worktree,
        selected: &mut Option<Oid>,
    ) -> Option<PanelAction> {
        let target = worktree.target();
        let is_selected = target.is_some() && target == *selected;
        let is_focused = target.is_some() && target == self.focused;
        let is_confirming = self.confirming == Some(worktree.dir_name());

        let frame = egui::Frame::none()
            .fill(if is_selected || is_focused {
                Palette::SELECTED
            } else {
                Palette::SURFACE
            })
            .rounding(egui::Rounding::same(5.0))
            .inner_margin(egui::Margin::symmetric(8.0, 7.0))
            .stroke(egui::Stroke::new(
                1.0_f32,
                if is_confirming {
                    // One click from deleting a directory: say so in the
                    // colour that means danger everywhere else in this window.
                    Palette::DANGER
                } else if is_focused {
                    // Only focus gets the outline. Several worktrees commonly
                    // sit on the same commit, so outlining every card that
                    // matches the selection would mark most of the panel.
                    Palette::CYAN
                } else {
                    Palette::BORDER
                },
            ));

        let mut action = None;

        let response = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    // The primary checkout is the one the repository itself
                    // sits in; everything else is a linked worktree.
                    if worktree.is_primary {
                        // A drawn marker, not a character: no glyph risk.
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, Palette::CYAN);
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
                    if is_focused {
                        ui.label(RichText::new("focused").color(Palette::CYAN).size(10.5));
                    }
                });

                action = self.actions(ui, worktree);
            })
            .response
            .interact(egui::Sense::click());

        // A button inside the card wins over the card's own click.
        if action.is_some() {
            return action;
        }

        // Clicking a worktree jumps to the commit it sits on and focuses its
        // history. Clicking the focused one again releases the focus, so the
        // same gesture goes both ways.
        if response.clicked() {
            *selected = target;
            return match target {
                Some(_) if is_focused => Some(PanelAction::ClearFocus),
                Some(oid) => Some(PanelAction::Focus(oid)),
                // An unborn worktree sits on no commit; there is nothing to
                // focus and nothing to jump to.
                None => None,
            };
        }

        None
    }

    /// The row of buttons at the foot of a card: removal (confirmed in two
    /// steps) and a terminal, offered on every card including the primary
    /// one.
    fn actions(&self, ui: &mut Ui, worktree: &Worktree) -> Option<PanelAction> {
        if self.confirming == Some(worktree.dir_name()) {
            return confirmation(ui, worktree);
        }

        let mut action = None;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if !worktree.is_primary {
                ui.add_enabled_ui(!self.busy, |ui| {
                    if ui.button(RichText::new("remove").size(10.5)).clicked() {
                        action = Some(PanelAction::AskToRemove(worktree.dir_name().to_string()));
                    }
                });
            }

            // Deliberately outside the `!self.busy` guard above: opening a
            // terminal is not a Git operation, so a worktree operation
            // running in the background must not block it.
            if ui
                .add_enabled(
                    !worktree.is_prunable,
                    egui::Button::new(RichText::new("terminal").size(10.5)),
                )
                .on_disabled_hover_text("gone; prune it instead")
                .clicked()
            {
                action = Some(PanelAction::OpenTerminal(worktree.dir_name().to_string()));
            }
        });

        action
    }
}

/// The second step of a removal, which says plainly what is about to be lost.
fn confirmation(ui: &mut Ui, worktree: &Worktree) -> Option<PanelAction> {
    let mut action = None;
    let dirty = worktree.status.total_changed();

    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Delete {}?", worktree.path.display()))
            .color(Palette::TEXT)
            .size(10.5),
    );

    if dirty > 0 {
        ui.label(
            RichText::new(format!(
                "{dirty} uncommitted file(s) would be lost for good"
            ))
            .color(Palette::DANGER)
            .size(10.5)
            .strong(),
        );
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let label = if dirty > 0 {
            "discard and delete"
        } else {
            "delete"
        };
        if ui
            .button(RichText::new(label).color(Palette::DANGER).size(10.5))
            .clicked()
        {
            action = Some(PanelAction::Remove {
                dir_name: worktree.dir_name().to_string(),
                // Forcing is exactly what the user just agreed to, and only
                // that: a clean worktree is still removed without it.
                force: dirty > 0,
            });
        }
        if ui.button(RichText::new("cancel").size(10.5)).clicked() {
            action = Some(PanelAction::CancelRemoval);
        }
    });

    action
}

/// One-line summary of a working directory's state, with the colour it should
/// be shown in. Conflicts outrank everything else.
fn status_badge(status: &WorktreeStatus) -> (String, Color32) {
    if status.has_conflicts() {
        return (format!("{} conflicted", status.conflicted), Palette::DANGER);
    }

    if status.is_clean() {
        return ("clean".to_string(), Palette::OK);
    }

    // Spelled out rather than written in Git's +/~/? shorthand, for the same
    // reason divergence_label spells out "ahead": the sidebar has to be
    // readable by someone who does not already know the notation.
    let mut parts = Vec::new();
    if status.staged > 0 {
        parts.push(format!("{} staged", status.staged));
    }
    if status.unstaged > 0 {
        parts.push(format!("{} modified", status.unstaged));
    }
    if status.untracked > 0 {
        parts.push(format!("{} new", status.untracked));
    }

    (parts.join(", "), Palette::DIRTY)
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
        assert_eq!(text, "1 staged, 2 modified, 3 new");
        assert_eq!(color, Palette::DIRTY);
    }

    #[test]
    fn categories_with_no_changes_are_left_out() {
        let status = WorktreeStatus {
            untracked: 4,
            ..Default::default()
        };
        assert_eq!(status_badge(&status).0, "4 new");
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
