//! The application shell: window layout and the load lifecycle.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::graph_view::GraphView;
use super::loader::{self, LoadMessage, PendingLoad};
use super::sidebar::WorktreePanel;
use super::theme::{self, Palette};
use crate::application::HistoryQuery;
use crate::domain::{GraphLayout, Oid, RepositorySnapshot};
use crate::infrastructure::Git2Backend;

const SIDEBAR_WIDTH: f32 = 250.0;

enum State {
    Loading,
    Ready {
        snapshot: Box<RepositorySnapshot>,
        layout: Box<GraphLayout>,
    },
    Failed(String),
}

/// A pending `--screenshot` request: render, capture, write, quit.
///
/// This exists so the rendering can be checked without a human looking at a
/// screen — on a build machine, or in a test.
struct Capture {
    path: PathBuf,
    /// Frames to let the window settle after loading. Capturing immediately
    /// tends to catch a half-laid-out first frame.
    settle: u32,
    requested: bool,
}

pub struct GitGuiApp {
    repo_path: PathBuf,
    state: State,
    pending: Option<PendingLoad>,
    selected: Option<Oid>,
    capture: Option<Capture>,
}

impl GitGuiApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        repo_path: PathBuf,
        query: HistoryQuery,
        screenshot: Option<PathBuf>,
    ) -> Self {
        theme::apply(&cc.egui_ctx);

        // Opening the repository is disk work, so even that happens on the
        // worker: the first frame must paint immediately.
        let path = repo_path.clone();
        let pending = loader::spawn(
            move || Git2Backend::discover(&path),
            query,
            cc.egui_ctx.clone(),
        );

        Self {
            repo_path,
            state: State::Loading,
            pending: Some(pending),
            selected: None,
            capture: screenshot.map(|path| Capture {
                path,
                settle: 3,
                requested: false,
            }),
        }
    }

    /// Drives the `--screenshot` sequence, if one was asked for.
    fn drive_capture(&mut self, ctx: &egui::Context) {
        let Some(capture) = &mut self.capture else {
            return;
        };

        // The reply to a request made on an earlier frame.
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        if let Some(color_image) = captured {
            if let Err(error) = write_png(&capture.path, &color_image) {
                eprintln!("gitgui: could not write {}: {error}", capture.path.display());
            } else {
                println!(
                    "gitgui: wrote {} ({}x{})",
                    capture.path.display(),
                    color_image.size[0],
                    color_image.size[1]
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Keep the frames coming: nothing is moving on screen, so without this
        // the window would idle and the capture would never happen.
        ctx.request_repaint();

        // Capture the loaded window, not the spinner.
        if matches!(self.state, State::Loading) {
            return;
        }

        if capture.settle > 0 {
            capture.settle -= 1;
        } else if !capture.requested {
            capture.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }

    fn poll_load(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        let Some(message) = pending.poll() else {
            return;
        };

        self.state = match message {
            LoadMessage::Loaded { snapshot, layout } => {
                // Start on the primary checkout, so the window opens showing
                // where the user actually is.
                self.selected = snapshot.primary_worktree().and_then(|wt| wt.target());
                State::Ready {
                    snapshot: Box::new(snapshot),
                    layout: Box::new(layout),
                }
            }
            LoadMessage::Failed(error) => State::Failed(error),
        };
        self.pending = None;
    }

    fn title_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let name = self
                .repo_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repository");

            ui.label(RichText::new(name).color(Palette::CYAN).size(13.5).strong());

            if let State::Ready { snapshot, layout } = &self.state {
                ui.label(
                    RichText::new(format!("{} commits", snapshot.commit_count()))
                        .color(Palette::TEXT_DIM)
                        .size(11.0),
                );
                ui.label(
                    RichText::new(format!("{} lanes", layout.lane_count()))
                        .color(Palette::TEXT_DIM)
                        .size(11.0),
                );
                ui.label(
                    RichText::new(format!("{} branches", snapshot.branches().len()))
                        .color(Palette::TEXT_DIM)
                        .size(11.0),
                );
                if snapshot.is_truncated() {
                    ui.label(
                        RichText::new("history truncated")
                            .color(Palette::DIRTY)
                            .size(11.0),
                    );
                }
            }
        });
    }
}

impl eframe::App for GitGuiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Palette::BACKDROP.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_load();

        egui::TopBottomPanel::top("title")
            .frame(
                egui::Frame::none()
                    .fill(Palette::PANEL)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| self.title_bar(ui));

        if let State::Ready { snapshot, .. } = &self.state {
            egui::SidePanel::left("worktrees")
                .resizable(true)
                .default_width(SIDEBAR_WIDTH)
                .frame(
                    egui::Frame::none()
                        .fill(Palette::PANEL)
                        .inner_margin(egui::Margin::same(10.0)),
                )
                .show(ctx, |ui| {
                    WorktreePanel { snapshot }.show(ui, &mut self.selected);
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Palette::BACKDROP))
            .show(ctx, |ui| match &self.state {
                State::Loading => {
                    ui.centered_and_justified(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new("Reading repository…")
                                    .color(Palette::TEXT_DIM)
                                    .size(13.0),
                            );
                        });
                    });
                }
                State::Failed(error) => {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new(error).color(Palette::DANGER).size(13.0));
                    });
                }
                State::Ready { snapshot, layout } => {
                    GraphView { snapshot, layout }.show(ui, &mut self.selected);
                }
            });

        self.drive_capture(ctx);
    }
}

/// Writes a captured framebuffer out as a PNG.
fn write_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let [width, height] = image.size;

    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }

    ::image::save_buffer(
        path,
        &rgba,
        width as u32,
        height as u32,
        ::image::ExtendedColorType::Rgba8,
    )
    .map_err(|error| error.to_string())
}
