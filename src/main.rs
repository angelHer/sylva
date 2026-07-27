//! Temporary entry point for phase 1.
//!
//! There is no window yet. This prints the snapshot the UI will eventually
//! render, so the Git layer can be validated against real repositories before
//! a single pixel is drawn.

use std::env;
use std::process::ExitCode;

use gitgui::application::ports::{RepositoryReader, WorktreeReader};
use gitgui::domain::{GraphLayout, GraphRow, RepositorySnapshot};
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

    let layout = GraphLayout::build(snapshot);
    println!(
        "\ngraph: {} rows, {} lanes wide",
        layout.len(),
        layout.lane_count()
    );

    let tips = snapshot.branch_tips();
    let anchors = snapshot.worktree_anchors();

    for (index, row) in layout.rows_in_range(0, 25).iter().enumerate() {
        let commit = snapshot
            .commit(&row.commit)
            .expect("layout rows mirror the snapshot");

        if index > 0 {
            println!("  {}", connector_line(row, layout.lane_count()));
        }

        let mut decorations: Vec<String> = tips
            .get(&row.commit)
            .map(|branches| branches.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default();

        // Worktree markers — the whole point of the unified graph.
        if let Some(worktrees) = anchors.get(&row.commit) {
            decorations.extend(worktrees.iter().map(|wt| {
                let dirty = if wt.status.is_dirty() { "*" } else { "" };
                format!("⌂{}{}", wt.dir_name(), dirty)
            }));
        }

        let decoration = if decorations.is_empty() {
            String::new()
        } else {
            format!(" [{}]", decorations.join(", "))
        };

        println!(
            "  {} {} {}{}",
            node_line(row, layout.lane_count()),
            row.commit.to_short_hex(8),
            commit.summary,
            decoration
        );
    }

    if !layout.trailing().is_empty() {
        println!("  {}  (history continues)", "⋮ ".repeat(layout.lane_count()));
    }
}

/// The row a commit's node sits on: a marker in its own lane, and the lines
/// passing straight through every other occupied lane.
fn node_line(row: &GraphRow, width: usize) -> String {
    let mut cells = vec![' '; width];

    for segment in &row.incoming {
        if let Some(cell) = cells.get_mut(segment.to_lane) {
            *cell = '│';
        }
    }
    if let Some(cell) = cells.get_mut(row.lane) {
        *cell = if row.is_merge { '◆' } else { '●' };
    }

    render(&cells)
}

/// The gap above a row: one character per lane summarising how its lines move.
fn connector_line(row: &GraphRow, width: usize) -> String {
    let mut cells = vec![' '; width];

    for segment in &row.incoming {
        let (column, glyph) = if segment.is_straight() {
            (segment.from_lane, '│')
        } else if segment.is_diverging() {
            (segment.to_lane, '╲')
        } else {
            (segment.from_lane, '╱')
        };

        if let Some(cell) = cells.get_mut(column) {
            // A straight line must never overwrite a diagonal: the diagonal
            // carries the information about where the branch went.
            if *cell == ' ' || glyph != '│' {
                *cell = glyph;
            }
        }
    }

    render(&cells)
}

fn render(cells: &[char]) -> String {
    cells.iter().flat_map(|c| [*c, ' ']).collect()
}
