//! The window that asks before a worktree is deleted.
//!
//! Removal deletes a directory, so it is never one click. The window says
//! where, and when the worktree holds uncommitted work it says that the work
//! goes with it: confirming then forces the removal, which is the only way Git
//! will delete a dirty worktree at all.

use eframe::egui::{self, RichText, Ui};

use super::sidebar::PanelAction;
use super::theme::Palette;
use crate::domain::Worktree;

pub fn show(ctx: &egui::Context, worktree: &Worktree) -> Option<PanelAction> {
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
            action = contents(ui, worktree);
        });

    // The window's own close button.
    if !open {
        return Some(PanelAction::CancelRemoval);
    }
    action
}

fn contents(ui: &mut Ui, worktree: &Worktree) -> Option<PanelAction> {
    let mut action = None;
    let dirty = worktree.status.total_changed();

    ui.label(
        RichText::new(format!("Delete {}?", worktree.path.display()))
            .color(Palette::TEXT)
            .size(11.5),
    );

    if dirty > 0 {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "{dirty} uncommitted file(s) would be lost for good"
            ))
            .color(Palette::DANGER)
            .size(11.0)
            .strong(),
        );
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let label = if dirty > 0 {
            "Discard and delete"
        } else {
            "Delete"
        };
        if ui
            .button(RichText::new(label).color(Palette::DANGER).size(11.5))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::{Oid, WorktreeHead, WorktreeStatus};
    use crate::ui::testing::click;

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
            click("Delete", |ctx| show(ctx, &worktree)),
            Some(PanelAction::Remove {
                dir_name: "feature-graph".to_string(),
                force: false,
            })
        );
    }

    #[test]
    fn confirming_a_dirty_worktree_forces_its_removal() {
        let worktree = worktree(dirty());

        assert_eq!(
            click("Discard and delete", |ctx| show(ctx, &worktree)),
            Some(PanelAction::Remove {
                dir_name: "feature-graph".to_string(),
                force: true,
            })
        );
    }

    #[test]
    fn cancelling_deletes_nothing() {
        let worktree = worktree(dirty());

        assert_eq!(
            click("Cancel", |ctx| show(ctx, &worktree)),
            Some(PanelAction::CancelRemoval)
        );
    }
}
