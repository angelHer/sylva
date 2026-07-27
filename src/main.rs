//! Temporary entry point for phase 1.
//!
//! There is no window yet. This prints the snapshot the UI will eventually
//! render, so the Git layer can be validated against real repositories before
//! a single pixel is drawn.

use std::env;
use std::process::ExitCode;

use gitgui::application::ports::{RepositoryReader, WorktreeReader};
use gitgui::domain::RepositorySnapshot;
use gitgui::{Git2Backend, HistoryQuery, LoadRepository};

fn main() -> ExitCode {
    let path = env::args().nth(1).unwrap_or_else(|| ".".to_string());

    let backend = match Git2Backend::discover(&path) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("gitgui: {error}");
            return ExitCode::FAILURE;
        }
    };

    let reader: &dyn RepositoryReader = &backend;
    let worktrees: &dyn WorktreeReader = &backend;

    let started = std::time::Instant::now();
    let snapshot = match LoadRepository::new(reader, worktrees).execute(&HistoryQuery::first_page())
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("gitgui: {error}");
            return ExitCode::FAILURE;
        }
    };
    let elapsed = started.elapsed();

    print_snapshot(&snapshot);
    println!("\nloaded in {:.1?}", elapsed);

    ExitCode::SUCCESS
}

fn print_snapshot(snapshot: &RepositorySnapshot) {
    println!("repository: {}", snapshot.root().display());
    println!(
        "commits:    {}{}",
        snapshot.commit_count(),
        if snapshot.is_truncated() {
            " (truncated)"
        } else {
            ""
        }
    );
    println!("branches:   {}", snapshot.branches().len());

    println!("\nworktrees:");
    for worktree in snapshot.worktrees() {
        let marker = if worktree.is_primary { "*" } else { " " };
        let status = worktree.status;
        let state = if status.is_clean() {
            "clean".to_string()
        } else {
            format!(
                "{} changed (+{} staged, ~{} unstaged, ?{} untracked, !{} conflicts)",
                status.total_changed(),
                status.staged,
                status.unstaged,
                status.untracked,
                status.conflicted
            )
        };

        let sync = match worktree.divergence {
            Some(d) if d.is_in_sync() => "in sync".to_string(),
            Some(d) => format!("+{}/-{}", d.ahead, d.behind),
            None => "no upstream".to_string(),
        };

        println!(
            "{marker} {:<24} {:<28} {:<12} {}",
            worktree.dir_name(),
            worktree.head.label(),
            sync,
            state
        );
        if worktree.is_prunable {
            println!("    ^ prunable: directory is missing or stale");
        }
    }

    println!("\nrecent history:");
    for commit in snapshot.commits().iter().take(15) {
        let refs: Vec<&str> = snapshot
            .branch_tips()
            .get(&commit.id)
            .map(|branches| branches.iter().map(|b| b.name.as_str()).collect())
            .unwrap_or_default();

        let decoration = if refs.is_empty() {
            String::new()
        } else {
            format!(" [{}]", refs.join(", "))
        };

        println!(
            "  {} {}{}",
            commit.id.to_short_hex(8),
            commit.summary,
            decoration
        );
    }
}
