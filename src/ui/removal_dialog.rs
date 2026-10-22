//! The window that asks before a worktree is deleted.
//!
//! Removal deletes a directory, so it is never one click. The window says
//! where, and when the worktree holds uncommitted work it says that the work
//! goes with it: confirming then forces the removal, which is the only way Git
//! will delete a dirty worktree at all.
//!
//! The count comes from Git, asked when the window opens, not from the
//! snapshot: the two can disagree, and a warning drawn from the snapshot once
//! offered a plain delete that Git then refused.

use eframe::egui::{self, RichText, Ui};

use super::sidebar::PanelAction;
use super::theme::Palette;
use crate::application::UncommittedEntry;
use crate::domain::Worktree;

/// Tall enough for a handful of entries; a longer list scrolls rather than
/// pushing the buttons off the screen.
const LIST_HEIGHT: f32 = 160.0;

/// `at_risk` is what Git counts as uncommitted, or `None` while it is still
/// being asked.
pub fn show(
    ctx: &egui::Context,
    worktree: &Worktree,
    at_risk: Option<&[UncommittedEntry]>,
) -> Option<PanelAction> {
    let mut action = None;
    let mut open = true;

    egui::Window::new("Remove worktree")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(Palette::PANEL)
                .stroke(egui::Stroke::new(1.0_f32, Palette::DANGER)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            action = contents(ui, worktree, at_risk);
        });

    // The window's own close button.
    if !open {
        return Some(PanelAction::CancelRemoval);
    }
    action
}

fn contents(
    ui: &mut Ui,
    worktree: &Worktree,
    at_risk: Option<&[UncommittedEntry]>,
) -> Option<PanelAction> {
    let mut action = None;
    let dirty = at_risk.map_or(0, <[UncommittedEntry]>::len);

    ui.label(
        RichText::new(format!("Delete {}?", worktree.path.display()))
            .color(Palette::TEXT)
            .size(11.5),
    );

    if at_risk.is_none() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new("checking for uncommitted work")
                    .color(Palette::TEXT_DIM)
                    .size(11.0),
            );
        });
    } else if dirty > 0 {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "{dirty} uncommitted file(s) would be lost for good"
            ))
            .color(Palette::DANGER)
            .size(11.0)
            .strong(),
        );
        ui.add_space(4.0);
        listing(ui, at_risk.unwrap_or_default());
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let label = if dirty > 0 {
            "Discard and delete"
        } else {
            "Delete"
        };
        // Not offered until Git has answered: until then nobody knows whether
        // confirming would need to force.
        if ui
            .add_enabled(
                at_risk.is_some(),
                egui::Button::new(RichText::new(label).color(Palette::DANGER).size(11.5)),
            )
            .clicked()
        {
            action = Some(PanelAction::Remove {
                dir_name: worktree.dir_name().to_string(),
                // Forcing is exactly what the user just agreed to, and only
                // that: a clean worktree is still removed without it.
                force: dirty > 0,
            });
        }
        if ui.button(RichText::new("Cancel").size(11.5)).clicked() {
            action = Some(PanelAction::CancelRemoval);
        }
    });

    action
}

fn listing(ui: &mut Ui, entries: &[UncommittedEntry]) {
    egui::Frame::none()
        .fill(Palette::SURFACE)
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(LIST_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("uncommitted")
                        .num_columns(2)
                        .spacing(egui::vec2(10.0, 3.0))
                        .show(ui, |ui| {
                            for entry in entries {
                                ui.label(
                                    RichText::new(entry.change.label())
                                        .color(Palette::DIRTY)
                                        .size(10.5),
                                );
                                ui.label(
                                    RichText::new(&entry.path)
                                        .color(Palette::TEXT)
                                        .size(10.5)
                                        .monospace(),
                                );
                                ui.end_row();
                            }
                        });
                });
        });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::application::ChangeKind;
    use crate::domain::{Oid, WorktreeHead, WorktreeStatus};
    use crate::ui::testing::{click, shows};

    fn entry(path: &str, change: ChangeKind) -> UncommittedEntry {
        UncommittedEntry {
            path: path.to_string(),
            change,
        }
    }

    fn modified(count: usize) -> Vec<UncommittedEntry> {
        (0..count)
            .map(|index| entry(&format!("file-{index}.txt"), ChangeKind::Modified))
            .collect()
    }

    fn worktree(status: WorktreeStatus) -> Worktree {
        Worktree {
            name: "feature-graph".into(),
            path: PathBuf::from("/home/dev/project-worktrees/feature-graph"),
            is_primary: false,
            head: WorktreeHead::Detached {
                target: Oid::from_bytes([2; 20]),
            },
            status,
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    fn dirty() -> WorktreeStatus {
        WorktreeStatus {
            unstaged: 1,
            untracked: 2,
            ..Default::default()
        }
    }

    #[test]
    fn a_clean_worktree_is_removed_without_force() {
        let worktree = worktree(WorktreeStatus::default());

        assert_eq!(
            click("Delete", |ctx| show(ctx, &worktree, Some(&[]))),
            Some(PanelAction::Remove {
                dir_name: "feature-graph".to_string(),
                force: false,
            })
        );
    }

    #[test]
    fn confirming_a_dirty_worktree_forces_its_removal() {
        let worktree = worktree(dirty());
        let at_risk = modified(3);

        assert_eq!(
            click("Discard and delete", |ctx| show(
                ctx,
                &worktree,
                Some(&at_risk)
            )),
            Some(PanelAction::Remove {
                dir_name: "feature-graph".to_string(),
                force: true,
            })
        );
    }

    #[test]
    fn cancelling_deletes_nothing() {
        let worktree = worktree(dirty());
        let at_risk = modified(3);

        assert_eq!(
            click("Cancel", |ctx| show(ctx, &worktree, Some(&at_risk))),
            Some(PanelAction::CancelRemoval)
        );
    }

    #[test]
    fn git_is_believed_over_a_snapshot_that_reads_clean() {
        // libgit2 called it clean; Git counts a symlinked node_modules.
        let worktree = worktree(WorktreeStatus::default());
        let at_risk = vec![entry("node_modules", ChangeKind::New)];

        assert_eq!(
            click("Discard and delete", |ctx| show(
                ctx,
                &worktree,
                Some(&at_risk)
            )),
            Some(PanelAction::Remove {
                dir_name: "feature-graph".to_string(),
                force: true,
            })
        );
    }

    #[test]
    fn every_file_that_would_be_lost_is_listed() {
        let worktree = worktree(WorktreeStatus::default());
        let at_risk = vec![
            entry("src/main.rs", ChangeKind::Modified),
            entry("node_modules", ChangeKind::New),
        ];

        let shown = shows(&["src/main.rs", "node_modules", "modified", "new"], |ctx| {
            show(ctx, &worktree, Some(&at_risk))
        });

        assert_eq!(shown, [true, true, true, true]);
    }

    #[test]
    fn nothing_is_deleted_before_git_has_answered() {
        let worktree = worktree(WorktreeStatus::default());

        assert_eq!(click("Delete", |ctx| show(ctx, &worktree, None)), None);
        assert_eq!(
            click("Cancel", |ctx| show(ctx, &worktree, None)),
            Some(PanelAction::CancelRemoval)
        );
    }
}
