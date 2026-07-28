//! Entry point.
//!
//! Opens the window by default. The text modes exist for benchmarking and for
//! inspecting a repository without a compositor:
//!
//! ```text
//! sylva [PATH]              open the window
//! sylva --welcome           start on the list of recent repositories
//! sylva --cli [PATH]        print the graph as text
//! sylva --profile [PATH]    time the load and layout paths
//! sylva --full              load all history instead of the first page
//! sylva --focus NAME        open with one worktree's history highlighted
//! ```

mod cli;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use sylva::{Git2Backend, HistoryQuery, SylvaApp};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "--cli" || a == "--profile") {
        return cli::run(&args);
    }

    let path = repository_to_open(&args);

    let query = if args.iter().any(|a| a == "--full") {
        HistoryQuery::full()
    } else {
        HistoryQuery::first_page()
    };

    // `--screenshot FILE` renders the window, writes it out and exits.
    let screenshot = value_of(&args, "--screenshot").map(PathBuf::from);
    let focus = value_of(&args, "--focus").cloned();

    run_window(path, query, screenshot, focus)
}

/// Decides which repository to open, or `None` for the welcome screen.
///
/// Three cases, in order:
/// - an explicit path wins, wherever it came from — including the folder a file
///   manager passed to the desktop entry;
/// - `--welcome` is how that desktop entry says "there is no path, and the
///   working directory is the user's home, so do not guess";
/// - otherwise the working directory, which is what makes `sylva` on its own
///   work from a terminal — unless it is not inside a repository, where the
///   welcome screen is more use than an error.
fn repository_to_open(args: &[String]) -> Option<PathBuf> {
    if let Some(path) = positional(args) {
        return Some(path);
    }

    if args.iter().any(|arg| arg == "--welcome") {
        return None;
    }

    let cwd = PathBuf::from(".");
    Git2Backend::exists_at(&cwd).then_some(cwd)
}

/// The first argument that is not a flag or a flag's value.
fn positional(args: &[String]) -> Option<PathBuf> {
    let mut rest = args.iter();
    loop {
        match rest.next() {
            // These flags take a value, which is not the repository path.
            Some(arg) if arg == "--screenshot" || arg == "--focus" => {
                rest.next();
            }
            Some(arg) if arg.starts_with("--") => continue,
            Some(arg) => return Some(PathBuf::from(arg)),
            None => return None,
        }
    }
}

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
}

fn run_window(
    path: Option<PathBuf>,
    query: HistoryQuery,
    screenshot: Option<PathBuf>,
    focus: Option<String>,
) -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([880.0, 560.0])
            // Matches `StartupWMClass` in sylva.desktop, which is how the
            // desktop shell pairs this window with its launcher and its icon.
            .with_app_id("sylva")
            .with_title("sylva"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "sylva",
        options,
        Box::new(move |cc| Ok(Box::new(SylvaApp::new(cc, path, query, screenshot, focus)))),
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sylva: {error}");
            ExitCode::FAILURE
        }
    }
}
