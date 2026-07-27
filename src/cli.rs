//! Text output for the same snapshot the window renders.
//!
//! Kept because it is the only way to benchmark the Git and layout layers
//! without a compositor in the loop, and because it makes the graph
//! inspectable in a terminal.

use std::process::ExitCode;

use gitgui::application::ports::{RepositoryReader, WorktreeReader};
use gitgui::domain::{GraphLayout, GraphRow, RepositorySnapshot};
use gitgui::{Git2Backend, HistoryQuery, LoadRepository};

pub fn run(args: &[String]) -> ExitCode {
    let full = args.iter().any(|a| a == "--full");
    let quiet = args.iter().any(|a| a == "--quiet");
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| ".".to_string());

    let backend = match Git2Backend::discover(&path) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("gitgui: {error}");
            return ExitCode::FAILURE;
        }
    };

    let reader: &dyn RepositoryReader = &backend;
    let worktrees: &dyn WorktreeReader = &backend;

    let query = if full {
        HistoryQuery::full()
    } else {
        HistoryQuery::first_page()
    };

    // Times each port call on its own, to find which part of the load
    // dominates. Temporary, like the rest of this binary.
    if args.iter().any(|a| a == "--profile") {
        // Repeated runs, reported as min/median. A single timing on a machine
        // with other work running measures the noise, not the code: observed
        // spread on this workload reached 60% between identical runs.
        const RUNS: usize = 9;

        let mut cold = None;
        let mut samples = Vec::with_capacity(RUNS);
        let mut rows = 0;

        for run in 0..RUNS {
            let started = std::time::Instant::now();
            let page = reader.commits(&query).expect("commits");
            let elapsed = started.elapsed();
            rows = page.commits.len();

            // The first run also pays for warming the object store.
            if run == 0 {
                cold = Some(elapsed);
            } else {
                samples.push(elapsed);
            }
        }

        samples.sort_unstable();
        println!("rows            {rows}");
        println!("commits() cold  {:>10.1?}", cold.expect("one run"));
        println!("commits() min   {:>10.1?}", samples[0]);
        println!("commits() med   {:>10.1?}", samples[samples.len() / 2]);
        println!("commits() max   {:>10.1?}", samples[samples.len() - 1]);

        let mut layout_samples = Vec::with_capacity(RUNS);
        let snapshot = LoadRepository::new(reader, worktrees)
            .execute(&query)
            .expect("snapshot");
        for _ in 0..RUNS {
            let started = std::time::Instant::now();
            let layout = GraphLayout::build(&snapshot);
            layout_samples.push(started.elapsed());
            std::hint::black_box(layout);
        }
        layout_samples.sort_unstable();
        println!("layout    min   {:>10.1?}", layout_samples[0]);
        println!("layout    med   {:>10.1?}", layout_samples[layout_samples.len() / 2]);

        let started = std::time::Instant::now();
        let branches = reader.branches().expect("branches");
        println!("branches()  {:>10.1?}  ({} refs)", started.elapsed(), branches.len());

        let started = std::time::Instant::now();
        let trees = worktrees.worktrees().expect("worktrees");
        println!("worktrees() {:>10.1?}  ({} trees)", started.elapsed(), trees.len());

        return ExitCode::SUCCESS;
    }

    let started = std::time::Instant::now();
    let snapshot = match LoadRepository::new(reader, worktrees).execute(&query) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("gitgui: {error}");
            return ExitCode::FAILURE;
        }
    };
    let load_time = started.elapsed();

    let started = std::time::Instant::now();
    let layout = GraphLayout::build(&snapshot);
    let layout_time = started.elapsed();

    if !quiet {
        print_snapshot(&snapshot, &layout);
    }

    println!(
        "\ncommits {} | lanes {} | load {:.1?} | layout {:.1?} | total {:.1?}",
        snapshot.commit_count(),
        layout.lane_count(),
        load_time,
        layout_time,
        load_time + layout_time
    );

    ExitCode::SUCCESS
}

fn print_snapshot(snapshot: &RepositorySnapshot, layout: &GraphLayout) {
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
