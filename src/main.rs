//! Entry point.
//!
//! Opens the window by default. The text modes exist for benchmarking and for
//! inspecting a repository without a compositor:
//!
//! ```text
//! gitgui [PATH]              open the window
//! gitgui --cli [PATH]        print the graph as text
//! gitgui --profile [PATH]    time the load and layout paths
//! gitgui --full              load all history instead of the first page
//! gitgui --focus NAME        open with one worktree's history highlighted
//! ```

mod cli;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use gitgui::{GitGuiApp, HistoryQuery};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "--cli" || a == "--profile") {
        return cli::run(&args);
    }

    // The value after `--screenshot` is a file to write, not the repository.
    let mut positional = args.iter();
    let path = loop {
        match positional.next() {
            // These flags take a value, which is not the repository path.
            Some(arg) if arg == "--screenshot" || arg == "--focus" => {
                positional.next();
            }
            Some(arg) if arg.starts_with("--") => continue,
            Some(arg) => break PathBuf::from(arg),
            None => break PathBuf::from("."),
        }
    };

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

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
}

fn run_window(
    path: PathBuf,
    query: HistoryQuery,
    screenshot: Option<PathBuf>,
    focus: Option<String>,
) -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title("gitgui"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "gitgui",
        options,
        Box::new(move |cc| Ok(Box::new(GitGuiApp::new(cc, path, query, screenshot, focus)))),
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gitgui: {error}");
            ExitCode::FAILURE
        }
    }
}
