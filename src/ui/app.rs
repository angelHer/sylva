//! The application shell: window layout, the load lifecycle, and the worktree
//! operations the user can start from it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, RichText};

use super::background::{self, Poll, Task};
use super::detail::CommitDetail;
use super::graph_view::GraphView;
use super::loader::{self, LoadMessage, PendingLoad};
use super::sidebar::{PanelAction, WorktreePanel};
use super::theme::{self, Palette};
use super::welcome::{Welcome, WelcomeAction};
use super::worktree_form::{FormOutcome, WorktreeForm};
use crate::application::{
    AddWorktree, AddWorktreeRequest, HistoryQuery, OpenTerminal, PruneWorktrees, RemoveWorktree,
    TerminalLauncher, WorktreeOperations,
};
use crate::domain::{Ancestry, GraphLayout, Oid, RepositorySnapshot};
use crate::infrastructure::{
    Git2Backend, GitCli, RecentRepositories, RepositoryWatcher, SystemTerminal,
};

const SIDEBAR_WIDTH: f32 = 250.0;
const DETAIL_WIDTH: f32 = 320.0;

/// How often to look for filesystem changes while nothing else is happening.
///
/// The watcher runs on its own thread, but a change only reaches the user when
/// a frame is drawn, and an idle window draws none.
const IDLE_POLL: Duration = Duration::from_millis(500);

enum State {
    /// No repository yet: offer the recent ones and a way to pick another.
    Welcome,
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

/// Something that changes the repository.
enum Operation {
    Add(Box<AddWorktreeRequest>),
    Remove { dir_name: String, force: bool },
    Prune,
}

/// The outcome of an operation, in words meant for the status bar.
type OperationResult = Result<String, String>;

pub struct SylvaApp {
    /// `None` until a repository is chosen, which is how the window opens when
    /// started from a desktop launcher.
    repo_path: Option<PathBuf>,
    state: State,
    pending: Option<PendingLoad>,
    selected: Option<Oid>,
    /// The history currently highlighted, computed once when focus changes
    /// rather than per frame.
    focus: Option<Ancestry>,
    /// A row to bring into view on the next frame.
    scroll_to: Option<usize>,
    /// A worktree name from `--focus`, consumed once the repository loads.
    initial_focus: Option<String>,
    /// The worktree awaiting a removal confirmation.
    confirming: Option<String>,
    form: Option<WorktreeForm>,
    /// Bound to the open repository, so it exists only once there is one.
    operations: Option<Arc<GitCli>>,
    /// The repositories offered on the welcome screen.
    recent: RecentRepositories,
    /// A directory dialog waiting on the user. It runs on a worker thread
    /// because the portal call blocks until they answer, which would otherwise
    /// freeze the window behind it.
    browsing: Option<Task<Option<PathBuf>>>,
    running: Option<Task<OperationResult>>,
    /// Machine-wide rather than repository-bound, so it is built once in
    /// [`Self::new`] and never rebuilt or cleared by [`Self::close_repository`].
    terminal: Arc<SystemTerminal>,
    /// Kept separate from `running`: a terminal launch is not a Git
    /// operation, and must not make [`Self::is_busy`] report one, or block a
    /// reload.
    opening: Option<Task<Result<(), String>>>,
    status: Option<(String, bool)>,
    watcher: Option<RepositoryWatcher>,
    /// Whether the window had keyboard focus last frame, to notice it coming
    /// back.
    was_focused: bool,
    capture: Option<Capture>,
}

impl SylvaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        repo_path: Option<PathBuf>,
        query: HistoryQuery,
        screenshot: Option<PathBuf>,
        initial_focus: Option<String>,
    ) -> Self {
        theme::apply(&cc.egui_ctx);

        let mut app = Self {
            repo_path: None,
            operations: None,
            recent: RecentRepositories::load(),
            browsing: None,
            pending: None,
            state: State::Welcome,
            selected: None,
            focus: None,
            scroll_to: None,
            initial_focus,
            confirming: None,
            form: None,
            running: None,
            terminal: Arc::new(SystemTerminal::new()),
            opening: None,
            status: None,
            watcher: None,
            was_focused: true,
            capture: screenshot.map(|path| Capture {
                path,
                settle: 3,
                requested: false,
            }),
        };

        if let Some(path) = repo_path {
            app.open(path, query, &cc.egui_ctx);
        }
        app
    }

    /// Points the window at a repository and starts reading it.
    ///
    /// Everything derived from the previous repository is dropped here rather
    /// than left to be noticed later: a stale watcher would report changes for
    /// a directory nobody is looking at, and a stale selection would highlight
    /// a commit that is not in the new history.
    fn open(&mut self, path: PathBuf, query: HistoryQuery, ctx: &egui::Context) {
        self.operations = Some(Arc::new(GitCli::at(&path)));
        self.pending = Some(Self::start_load(&path, query, ctx));
        self.repo_path = Some(path.clone());
        self.state = State::Loading;
        self.selected = None;
        self.focus = None;
        self.scroll_to = None;
        self.confirming = None;
        self.form = None;
        self.status = None;
        self.watcher = None;

        // Remembered on open rather than on a successful load: a path that
        // fails today may be a drive that is not mounted yet, and forgetting it
        // would be the wrong lesson. Paths that stop being repositories are
        // dropped the next time the list is read.
        self.recent.record(&path);
        if let Err(error) = self.recent.save() {
            eprintln!("sylva: could not save the recent repository list: {error}");
        }
    }

    /// Returns to the welcome screen, releasing the repository.
    fn close_repository(&mut self) {
        self.repo_path = None;
        self.operations = None;
        self.pending = None;
        self.watcher = None;
        self.selected = None;
        self.focus = None;
        self.confirming = None;
        self.form = None;
        self.status = None;
        self.state = State::Welcome;
        // Reread so a repository that has since been deleted stops being
        // offered.
        self.recent = RecentRepositories::load();
    }

    /// Asks the user for a directory, on a worker thread.
    fn browse(&mut self, ctx: &egui::Context) {
        if self.browsing.is_some() {
            return;
        }

        // Start the dialog where the user most likely keeps their projects.
        let start = self
            .repo_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));

        self.browsing = Some(background::spawn(
            move || {
                let mut dialog = rfd::FileDialog::new().set_title("Open a repository");
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                dialog.pick_folder()
            },
            ctx.clone(),
        ));
    }

    fn poll_browse(&mut self, ctx: &egui::Context) {
        let Some(browsing) = &mut self.browsing else {
            return;
        };

        let chosen = match browsing.poll() {
            Poll::Pending => return,
            Poll::Ready(chosen) => chosen,
            // The dialog could not be shown at all — no portal, no session bus.
            // Say so, because silently doing nothing looks like a dead button.
            Poll::Lost => {
                self.browsing = None;
                self.status = Some((
                    "could not open the folder dialog; pass the repository path on the command line"
                        .to_string(),
                    true,
                ));
                return;
            }
        };
        self.browsing = None;

        // Cancelled: leave everything as it was.
        if let Some(path) = chosen {
            self.open(path, HistoryQuery::first_page(), ctx);
        }
    }

    /// Opening the repository is disk work, so even that happens on the
    /// worker: the first frame must paint immediately.
    fn start_load(root: &Path, query: HistoryQuery, ctx: &egui::Context) -> PendingLoad {
        let path = root.to_path_buf();
        loader::spawn(move || Git2Backend::discover(&path), query, ctx.clone())
    }

    /// Reads the repository again, keeping what the user was looking at.
    fn reload(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return; // Already on its way.
        }
        let Some(path) = &self.repo_path else {
            return; // Nothing open to reload.
        };
        self.pending = Some(Self::start_load(path, HistoryQuery::first_page(), ctx));
    }

    fn poll_load(&mut self, ctx: &egui::Context) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        let Some(message) = pending.poll() else {
            return;
        };
        self.pending = None;

        self.state = match message {
            LoadMessage::Loaded { snapshot, layout } => {
                if self.selected.is_none() {
                    // Start on the primary checkout, so the window opens
                    // showing where the user actually is.
                    self.selected = snapshot.primary_worktree().and_then(|wt| wt.target());
                }
                State::Ready {
                    snapshot: Box::new(snapshot),
                    layout: Box::new(layout),
                }
            }
            LoadMessage::Failed(error) => State::Failed(error),
        };

        // The highlight is a set of row positions, and the rows just changed.
        self.rebuild_focus();
        self.apply_initial_focus(ctx);

        self.start_watching();
        ctx.request_repaint();
    }

    /// Recomputes the highlight against the snapshot that has just arrived.
    ///
    /// An ancestry is a mask over rows, so one built for a previous snapshot
    /// would highlight whatever now happens to sit at those positions.
    fn rebuild_focus(&mut self) {
        let Some(tip) = self.focus.as_ref().map(Ancestry::tip) else {
            return;
        };
        let State::Ready { snapshot, .. } = &self.state else {
            return;
        };

        let rebuilt = Ancestry::of(snapshot, tip);
        // The focused commit can be gone: its branch was deleted, or it fell
        // outside the loaded page. Releasing the focus is better than dimming
        // the entire window.
        self.focus = (!rebuilt.is_empty()).then_some(rebuilt);
    }

    fn start_watching(&mut self) {
        if self.watcher.is_some() {
            return;
        }
        let State::Ready { snapshot, .. } = &self.state else {
            return;
        };

        match RepositoryWatcher::watch(snapshot.root()) {
            Ok(watcher) => self.watcher = Some(watcher),
            // Not fatal: the window still reloads on focus and after every
            // operation. Say so once rather than failing to open.
            Err(error) => {
                self.status = Some((format!("not watching for changes: {error}"), true));
            }
        }
    }

    /// Honours `--focus NAME`, once, as soon as there is a repository to look
    /// the name up in.
    fn apply_initial_focus(&mut self, ctx: &egui::Context) {
        let Some(name) = self.initial_focus.take() else {
            return;
        };
        let State::Ready { snapshot, .. } = &self.state else {
            return;
        };

        let Some(tip) = snapshot
            .worktrees()
            .iter()
            .find(|worktree| worktree.dir_name() == name)
            .and_then(|worktree| worktree.target())
        else {
            eprintln!("sylva: no worktree named {name}");
            return;
        };

        self.selected = Some(tip);
        self.apply(PanelAction::Focus(tip), ctx);
    }

    fn apply(&mut self, action: PanelAction, ctx: &egui::Context) {
        match action {
            PanelAction::Focus(tip) => {
                let State::Ready { snapshot, .. } = &self.state else {
                    return;
                };
                self.scroll_to = snapshot.position_of(&tip);
                self.focus = Some(Ancestry::of(snapshot, tip));
            }
            PanelAction::ClearFocus => self.focus = None,
            PanelAction::NewWorktree => self.form = Some(WorktreeForm::default()),
            PanelAction::Prune => self.start(Operation::Prune, ctx),
            PanelAction::AskToRemove(dir_name) => self.confirming = Some(dir_name),
            PanelAction::CancelRemoval => self.confirming = None,
            PanelAction::Remove { dir_name, force } => {
                self.confirming = None;
                self.start(Operation::Remove { dir_name, force }, ctx);
            }
            PanelAction::OpenTerminal(dir_name) => self.open_terminal(dir_name, ctx),
        }
    }

    /// Resolves and starts a terminal for a worktree, on a worker thread.
    ///
    /// Not routed through [`Self::start`]: a terminal launch is not a Git
    /// operation, `WorktreeOperations` has nothing to say about it, and a
    /// running worktree operation must not block it.
    fn open_terminal(&mut self, dir_name: String, ctx: &egui::Context) {
        if self.opening.is_some() {
            return;
        }
        let State::Ready { snapshot, .. } = &self.state else {
            return;
        };

        let metadata = snapshot.metadata_only();
        let terminal = Arc::clone(&self.terminal);

        self.opening = Some(background::spawn(
            move || {
                let launch = OpenTerminal::new(terminal.as_ref())
                    .execute(&metadata, &dir_name)
                    .map_err(|error| error.to_string())?;
                terminal.start(&launch).map_err(|error| error.to_string())
            },
            ctx.clone(),
        ));
    }

    /// Reports a failed terminal launch. A success is silent by design: no
    /// status, no spinner, no repository reload — nothing about the
    /// repository changed.
    fn poll_terminal(&mut self, _ctx: &egui::Context) {
        let Some(opening) = &mut self.opening else {
            return;
        };

        let outcome = match opening.poll() {
            Poll::Pending => return,
            Poll::Ready(result) => result,
            Poll::Lost => Err("the terminal could not be started".to_string()),
        };
        self.opening = None;

        if let Err(message) = outcome {
            self.status = Some((message, true));
        }
    }

    /// Runs an operation on a worker thread.
    ///
    /// Only the refs and checkouts go across, never the history: the guards
    /// never look at a commit, and copying tens of thousands of them to ask
    /// about a dozen directories would stall the frame this is meant to save.
    fn start(&mut self, operation: Operation, ctx: &egui::Context) {
        if self.running.is_some() {
            return;
        }
        let State::Ready { snapshot, .. } = &self.state else {
            return;
        };

        let Some(operations) = &self.operations else {
            return;
        };

        let metadata = snapshot.metadata_only();
        let operations = Arc::clone(operations);

        self.status = None;
        self.running = Some(background::spawn(
            move || run(operations.as_ref(), &metadata, operation),
            ctx.clone(),
        ));
    }

    fn poll_operation(&mut self, ctx: &egui::Context) {
        let Some(running) = &mut self.running else {
            return;
        };

        let outcome = match running.poll() {
            Poll::Pending => return,
            Poll::Ready(result) => result,
            Poll::Lost => Err("the operation stopped unexpectedly".to_string()),
        };
        self.running = None;

        match outcome {
            Ok(message) => {
                self.status = Some((message, false));
                self.form = None;
                // The repository changed under us; the watcher would catch it
                // too, but waiting out the quiet period would feel sluggish
                // right after a deliberate action.
                self.reload(ctx);
            }
            Err(message) => {
                // A failure while the dialog is open belongs in the dialog,
                // next to the field that caused it.
                match &mut self.form {
                    Some(form) => form.show_error(message),
                    None => self.status = Some((message, true)),
                }
            }
        }
    }

    /// Reloads when the repository changes on disk, or when the window comes
    /// back to the foreground.
    fn poll_for_changes(&mut self, ctx: &egui::Context) {
        let mut stale = self
            .watcher
            .as_ref()
            .is_some_and(RepositoryWatcher::take_change);

        // Editing a file without staging it touches nothing under .git, so the
        // watcher cannot see it. Coming back to this window is the moment that
        // matters in practice: you edited somewhere else, now you are here.
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            stale = true;
        }
        self.was_focused = focused;

        if stale {
            self.reload(ctx);
        }

        // An idle window draws no frames, and a change nobody looks for is a
        // change nobody sees.
        ctx.request_repaint_after(IDLE_POLL);
    }

    fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// Draws the header. Returns whether the user asked to go back to the list
    /// of repositories.
    fn title_bar(&self, ui: &mut egui::Ui) -> bool {
        let mut go_home = false;

        ui.horizontal(|ui| {
            go_home = ui
                .add(
                    egui::Button::new(
                        RichText::new("repositories")
                            .color(Palette::TEXT_DIM)
                            .size(11.0),
                    )
                    .frame(false),
                )
                .on_hover_text("Go back and open another repository")
                .clicked();

            // The snapshot's root is the primary checkout, which is the name
            // worth showing: it stays the same whichever worktree was opened,
            // and it survives being given a relative path like `.`.
            let shown = match &self.state {
                State::Ready { snapshot, .. } => Some(snapshot.root().as_path()),
                _ => self.repo_path.as_deref(),
            };
            let name = shown
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .unwrap_or("repository");

            ui.label(RichText::new(name).color(Palette::CYAN).size(13.5).strong());

            if let State::Ready { snapshot, layout } = &self.state {
                for text in [
                    format!("{} commits", snapshot.commit_count()),
                    format!("{} lanes", layout.lane_count()),
                    format!("{} branches", snapshot.branches().len()),
                ] {
                    ui.label(RichText::new(text).color(Palette::TEXT_DIM).size(11.0));
                }
                if snapshot.is_truncated() {
                    ui.label(
                        RichText::new("history truncated")
                            .color(Palette::DIRTY)
                            .size(11.0),
                    );
                }
            }

            if self.is_busy() {
                ui.spinner();
            }
        });

        go_home
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let Some((message, is_error)) = &self.status else {
            return;
        };
        ui.label(
            RichText::new(message)
                .color(if *is_error {
                    Palette::DANGER
                } else {
                    Palette::OK
                })
                .size(11.0),
        );
    }
}

/// Carries out one operation. Runs on a worker thread, so it returns words
/// rather than touching any state.
fn run(
    operations: &dyn WorktreeOperations,
    metadata: &RepositorySnapshot,
    operation: Operation,
) -> OperationResult {
    match operation {
        Operation::Add(request) => AddWorktree::new(operations)
            .execute(metadata, &request)
            .map(|()| format!("created {}", request.path.display()))
            .map_err(|error| error.to_string()),

        Operation::Remove { dir_name, force } => RemoveWorktree::new(operations)
            .execute(metadata, &dir_name, force)
            .map(|()| format!("removed {dir_name}"))
            .map_err(|error| error.to_string()),

        Operation::Prune => PruneWorktrees::new(operations)
            .execute()
            .map(|count| match count {
                0 => "nothing to prune".to_string(),
                1 => "pruned 1 stale worktree record".to_string(),
                many => format!("pruned {many} stale worktree records"),
            })
            .map_err(|error| error.to_string()),
    }
}

impl eframe::App for SylvaApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Palette::BACKDROP.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_load(ctx);
        self.poll_operation(ctx);
        self.poll_browse(ctx);
        self.poll_terminal(ctx);
        self.poll_for_changes(ctx);

        let mut go_home = false;
        if !matches!(self.state, State::Welcome) {
            go_home = egui::TopBottomPanel::top("title")
                .frame(
                    egui::Frame::none()
                        .fill(Palette::PANEL)
                        .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
                )
                .show(ctx, |ui| self.title_bar(ui))
                .inner;
        }

        if self.status.is_some() {
            egui::TopBottomPanel::bottom("status")
                .frame(
                    egui::Frame::none()
                        .fill(Palette::PANEL)
                        .inner_margin(egui::Margin::symmetric(12.0, 6.0)),
                )
                .show(ctx, |ui| self.status_bar(ui));
        }

        let mut action = None;
        if let State::Ready { snapshot, .. } = &self.state {
            action = egui::SidePanel::left("worktrees")
                .resizable(true)
                .default_width(SIDEBAR_WIDTH)
                .frame(
                    egui::Frame::none()
                        .fill(Palette::PANEL)
                        .inner_margin(egui::Margin::same(10.0)),
                )
                .show(ctx, |ui| {
                    WorktreePanel {
                        snapshot,
                        focused: self.focus.as_ref().map(Ancestry::tip),
                        confirming: self.confirming.as_deref(),
                        busy: self.running.is_some(),
                    }
                    .show(ui, &mut self.selected)
                })
                .inner;

            egui::SidePanel::right("detail")
                .resizable(true)
                .default_width(DETAIL_WIDTH)
                .frame(
                    egui::Frame::none()
                        .fill(Palette::PANEL)
                        .inner_margin(egui::Margin::same(12.0)),
                )
                .show(ctx, |ui| {
                    CommitDetail {
                        snapshot,
                        selected: self.selected,
                    }
                    .show(ui);
                });
        }

        let mut welcome_action = WelcomeAction::None;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Palette::BACKDROP))
            .show(ctx, |ui| match &self.state {
                State::Welcome => {
                    welcome_action = Welcome {
                        recent: self.recent.entries(),
                        browsing: self.browsing.is_some(),
                    }
                    .show(ui);
                }
                State::Loading => {
                    ui.centered_and_justified(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new("Reading repository...")
                                    .color(Palette::TEXT_DIM)
                                    .size(13.0),
                            );
                        });
                    });
                }
                State::Failed(error) => {
                    // A failure has to offer a way out, or a launcher pointed at
                    // a directory that is not a repository is a dead window.
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 2.0 - 40.0);
                        ui.label(RichText::new(error).color(Palette::DANGER).size(13.0));
                        ui.add_space(14.0);
                        if ui
                            .button(RichText::new("Choose another repository").size(12.0))
                            .clicked()
                        {
                            welcome_action = WelcomeAction::Browse;
                        }
                    });
                }
                State::Ready { snapshot, layout } => {
                    GraphView {
                        snapshot,
                        layout,
                        focus: self.focus.as_ref(),
                        scroll_to: self.scroll_to,
                    }
                    .show(ui, &mut self.selected);
                }
            });

        // The jump is a one-shot: leaving it set would pin the scroll position
        // and make the graph impossible to scroll by hand.
        self.scroll_to = None;

        if let Some(form) = &mut self.form {
            let root = match &self.state {
                State::Ready { snapshot, .. } => snapshot.root().clone(),
                _ => self.repo_path.clone().unwrap_or_default(),
            };

            match form.show(ctx, &root) {
                FormOutcome::Open => {}
                FormOutcome::Cancelled => self.form = None,
                FormOutcome::Submit(request) => self.start(Operation::Add(request), ctx),
            }
        }

        if let Some(action) = action {
            self.apply(action, ctx);
        }

        match welcome_action {
            WelcomeAction::None => {}
            WelcomeAction::Open(path) => self.open(path, HistoryQuery::first_page(), ctx),
            WelcomeAction::Browse => self.browse(ctx),
            WelcomeAction::Forget(path) => {
                self.recent.forget(&path);
                if let Err(error) = self.recent.save() {
                    eprintln!("sylva: could not save the recent repository list: {error}");
                }
            }
        }

        if go_home {
            self.close_repository();
        }

        self.drive_capture(ctx);
    }
}

impl SylvaApp {
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
                eprintln!("sylva: could not write {}: {error}", capture.path.display());
            } else {
                println!(
                    "sylva: wrote {} ({}x{})",
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
