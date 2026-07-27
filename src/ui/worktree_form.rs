//! The dialog for creating a worktree.
//!
//! It refuses to submit anything the domain would reject, and shows why. The
//! validation itself is not duplicated here — the name goes through
//! [`BranchName`] exactly as it would anywhere else.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};

use super::theme::Palette;
use crate::application::AddWorktreeRequest;
use crate::domain::{default_worktree_path, BranchName};

pub enum FormOutcome {
    /// Still open.
    Open,
    Cancelled,
    Submit(Box<AddWorktreeRequest>),
}

pub struct WorktreeForm {
    branch: String,
    path: String,
    create_branch: bool,
    /// Set once the user types in the path field, so the suggestion stops
    /// overwriting what they wrote.
    path_is_manual: bool,
    error: Option<String>,
}

impl Default for WorktreeForm {
    fn default() -> Self {
        Self {
            branch: String::new(),
            path: String::new(),
            create_branch: true,
            path_is_manual: false,
            error: None,
        }
    }
}

impl WorktreeForm {
    /// Keeps the suggested path in step with the branch name until the user
    /// takes the path over.
    fn refresh_suggestion(&mut self, repository_root: &Path) {
        if self.path_is_manual {
            return;
        }

        self.path = match BranchName::parse(&self.branch) {
            Ok(branch) => default_worktree_path(repository_root, &branch)
                .to_string_lossy()
                .into_owned(),
            // Nothing sensible to suggest for a name that is not a branch yet.
            Err(_) => String::new(),
        };
    }

    fn build_request(&self) -> Result<AddWorktreeRequest, String> {
        let branch = BranchName::parse(&self.branch).map_err(|error| error.to_string())?;

        let path = self.path.trim();
        if path.is_empty() {
            return Err("choose where the worktree should live".to_string());
        }

        Ok(AddWorktreeRequest {
            branch,
            path: PathBuf::from(path),
            create_branch: self.create_branch,
        })
    }

    pub fn show(&mut self, ctx: &egui::Context, repository_root: &Path) -> FormOutcome {
        let mut outcome = FormOutcome::Open;
        let mut open = true;

        egui::Window::new("New worktree")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(Palette::PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, Palette::BORDER)),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0);
                outcome = self.contents(ui, repository_root);
            });

        // The window's own close button.
        if !open {
            return FormOutcome::Cancelled;
        }
        outcome
    }

    fn contents(&mut self, ui: &mut Ui, repository_root: &Path) -> FormOutcome {
        let mut outcome = FormOutcome::Open;

        ui.label(RichText::new("branch").color(Palette::TEXT_DIM).size(10.5));
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.branch)
                .hint_text("feature/graph-lanes")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.error = None;
            self.refresh_suggestion(repository_root);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .radio(self.create_branch, RichText::new("create it").size(11.5))
                .clicked()
            {
                self.create_branch = true;
                self.error = None;
            }
            if ui
                .radio(!self.create_branch, RichText::new("check out an existing branch").size(11.5))
                .clicked()
            {
                self.create_branch = false;
                self.error = None;
            }
        });

        ui.add_space(8.0);
        ui.label(RichText::new("directory").color(Palette::TEXT_DIM).size(10.5));
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.path)
                .hint_text("a sibling of the repository")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.path_is_manual = true;
            self.error = None;
        }

        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.label(RichText::new(error).color(Palette::DANGER).size(11.0));
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Create").size(11.5)).clicked() {
                match self.build_request() {
                    Ok(request) => outcome = FormOutcome::Submit(Box::new(request)),
                    Err(message) => self.error = Some(message),
                }
            }
            if ui.button(RichText::new("Cancel").size(11.5)).clicked() {
                outcome = FormOutcome::Cancelled;
            }
        });

        outcome
    }

    /// Reports a failure that only became known once git ran.
    pub fn show_error(&mut self, message: String) {
        self.error = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(branch: &str) -> WorktreeForm {
        let mut form = WorktreeForm {
            branch: branch.to_string(),
            ..Default::default()
        };
        form.refresh_suggestion(Path::new("/home/dev/project"));
        form
    }

    #[test]
    fn the_suggested_directory_follows_the_branch_name() {
        let form = form("feature/graph");
        assert_eq!(form.path, "/home/dev/project-worktrees/feature-graph");
    }

    #[test]
    fn a_name_that_is_not_yet_a_valid_branch_suggests_nothing() {
        assert!(form("feature/").path.is_empty());
        assert!(form("").path.is_empty());
    }

    #[test]
    fn a_directory_the_user_typed_is_not_overwritten_by_the_suggestion() {
        let mut form = form("feature/graph");
        form.path = "/somewhere/else".to_string();
        form.path_is_manual = true;

        form.branch = "feature/other".to_string();
        form.refresh_suggestion(Path::new("/home/dev/project"));

        assert_eq!(form.path, "/somewhere/else");
    }

    #[test]
    fn a_valid_form_produces_the_request_the_use_case_expects() {
        let request = form("feature/graph").build_request().expect("valid");

        assert_eq!(request.branch.as_str(), "feature/graph");
        assert_eq!(
            request.path,
            PathBuf::from("/home/dev/project-worktrees/feature-graph")
        );
        assert!(request.create_branch);
    }

    #[test]
    fn an_invalid_branch_name_is_reported_in_the_words_the_domain_used() {
        let error = form("my branch").build_request().unwrap_err();
        assert!(error.contains("cannot contain"), "got {error:?}");
    }

    #[test]
    fn an_empty_directory_is_refused() {
        let mut form = form("feature/graph");
        form.path = "   ".to_string();

        assert!(form.build_request().unwrap_err().contains("where"));
    }

    #[test]
    fn checking_out_an_existing_branch_is_carried_through_to_the_request() {
        let mut form = form("release");
        form.create_branch = false;

        assert!(!form.build_request().unwrap().create_branch);
    }
}
