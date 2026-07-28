//! The screen shown when sylva starts without a repository.
//!
//! Launched from a desktop menu there is no repository to open: the working
//! directory is the user's home. Rather than an error, this offers the ones
//! opened before and a way to pick another.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};

use super::theme::Palette;

/// The widest the card gets, so the list does not stretch across a maximised
/// window and turn each row into a hard-to-hit ribbon.
const CARD_WIDTH: f32 = 460.0;

const ROW_HEIGHT: f32 = 44.0;

pub enum WelcomeAction {
    None,
    /// Open this repository.
    Open(PathBuf),
    /// Ask the user for a directory.
    Browse,
    /// Drop a repository from the list.
    Forget(PathBuf),
}

pub struct Welcome<'a> {
    /// Newest first.
    pub recent: &'a [PathBuf],
    /// Whether a directory dialog is already open, so a second one is not
    /// asked for.
    pub browsing: bool,
}

impl Welcome<'_> {
    pub fn show(self, ui: &mut egui::Ui) -> WelcomeAction {
        let mut action = WelcomeAction::None;

        let top = self.leading_space(ui.available_height());

        ui.vertical_centered(|ui| {
            ui.add_space(top);
            ui.set_max_width(CARD_WIDTH);

            ui.label(
                RichText::new("sylva")
                    .color(Palette::CYAN)
                    .size(34.0)
                    .strong(),
            );
            ui.label(
                RichText::new("many trees, one root")
                    .color(Palette::TEXT_FAINT)
                    .size(12.0)
                    .italics(),
            );

            ui.add_space(28.0);

            let button = egui::Button::new(
                RichText::new("Open a repository…")
                    .color(Palette::BACKDROP)
                    .size(13.0)
                    .strong(),
            )
            .fill(Palette::CYAN)
            .min_size(egui::vec2(CARD_WIDTH, 34.0));

            if ui.add_enabled(!self.browsing, button).clicked() {
                action = WelcomeAction::Browse;
            }
            if self.browsing {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Waiting for the folder dialog…")
                        .color(Palette::TEXT_DIM)
                        .size(11.0),
                );
            }

            ui.add_space(28.0);

            if self.recent.is_empty() {
                ui.label(
                    RichText::new("No repositories yet. The ones you open show up here.")
                        .color(Palette::TEXT_FAINT)
                        .size(11.5),
                );
                return;
            }

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("RECENT")
                        .color(Palette::TEXT_FAINT)
                        .size(10.0)
                        .strong(),
                );
            });
            ui.add_space(6.0);

            for path in self.recent {
                if let Some(chosen) = row(ui, path) {
                    action = chosen;
                }
            }
        });

        action
    }

    /// How far down to start, so the card sits centred whether the list is
    /// empty or full.
    ///
    /// The height is predicted rather than measured because centring on a
    /// measurement taken during layout costs a frame, and the card would visibly
    /// jump on the way in.
    fn leading_space(&self, available: f32) -> f32 {
        /// Title, tagline, button and the gaps around them.
        const HEADER: f32 = 170.0;
        /// The "RECENT" label and the space above it.
        const LIST_HEADER: f32 = 24.0;

        let list = if self.recent.is_empty() {
            20.0
        } else {
            LIST_HEADER + self.recent.len() as f32 * (ROW_HEIGHT + 2.0)
        };

        // Never so far down that a short window pushes the button off screen.
        ((available - HEADER - list) / 2.0).clamp(24.0, 160.0)
    }
}

/// One repository in the list: its name, its location, and a way to forget it.
fn row(ui: &mut egui::Ui, path: &Path) -> Option<WelcomeAction> {
    let mut action = None;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(4.0), Palette::HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repository");
    // The parent is what tells two checkouts of the same name apart.
    let location = path
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_default();

    let text_area = egui::UiBuilder::new()
        .max_rect(rect.shrink2(egui::vec2(10.0, 5.0)))
        .layout(egui::Layout::top_down(egui::Align::LEFT));
    ui.allocate_new_ui(text_area, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(RichText::new(name).color(Palette::TEXT).size(13.0));
        ui.label(
            RichText::new(location)
                .color(Palette::TEXT_FAINT)
                .size(10.5),
        );
    });

    if response.clicked() {
        action = Some(WelcomeAction::Open(path.to_path_buf()));
    }

    // Only offered while pointing at the row: a permanent column of crosses
    // next to every entry invites the accident it is meant to allow. Assigned
    // after the row's own click so pressing it removes rather than opens.
    if response.hovered() && forget_cross(ui, rect) {
        action = Some(WelcomeAction::Forget(path.to_path_buf()));
    }

    ui.add_space(2.0);
    action
}

/// The "forget this one" cross at the right of a row. Returns whether it was
/// clicked.
///
/// Drawn with the painter rather than set as text: the bundled font has no
/// glyph for a multiplication sign, and a missing glyph renders as an empty box
/// that looks like a bug.
fn forget_cross(ui: &mut egui::Ui, row: egui::Rect) -> bool {
    const ARM: f32 = 4.5;

    let hit = egui::Rect::from_center_size(
        egui::pos2(row.right() - 18.0, row.center().y),
        egui::vec2(20.0, 20.0),
    );
    let response = ui
        .interact(
            hit,
            ui.id().with(("forget", row.top() as i32)),
            egui::Sense::click(),
        )
        .on_hover_text("Remove from this list");

    let color = if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        Palette::DANGER
    } else {
        Palette::TEXT_FAINT
    };

    let centre = hit.center();
    let stroke = egui::Stroke::new(1.4_f32, color);
    let painter = ui.painter();
    painter.line_segment(
        [
            centre + egui::vec2(-ARM, -ARM),
            centre + egui::vec2(ARM, ARM),
        ],
        stroke,
    );
    painter.line_segment(
        [
            centre + egui::vec2(ARM, -ARM),
            centre + egui::vec2(-ARM, ARM),
        ],
        stroke,
    );

    response.clicked()
}
