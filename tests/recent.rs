//! Tests for the recent-repositories list that backs the welcome screen.
//!
//! The list is what makes a desktop launcher useful: opened from a menu there
//! is no working directory to infer a repository from, so the last few have to
//! be remembered.

use std::fs;
use std::path::{Path, PathBuf};

use sylva::infrastructure::RecentRepositories;

/// Creates `count` directories that look enough like repositories to be kept.
fn repositories(root: &Path, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|index| {
            let path = root.join(format!("repo-{index}"));
            fs::create_dir_all(path.join(".git")).expect("create repository");
            path
        })
        .collect()
}

#[test]
fn a_missing_file_reads_as_an_empty_list() {
    let dir = tempfile::tempdir().expect("temp dir");
    let recent = RecentRepositories::at(dir.path().join("never-written"));

    assert!(recent.entries().is_empty());
}

#[test]
fn recording_then_loading_keeps_the_paths() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("recent");
    let repos = repositories(dir.path(), 2);

    let mut recent = RecentRepositories::at(file.clone());
    recent.record(&repos[0]);
    recent.record(&repos[1]);
    recent.save().expect("save");

    let reloaded = RecentRepositories::at(file);
    assert_eq!(reloaded.entries(), &[repos[1].clone(), repos[0].clone()]);
}

#[test]
fn the_newest_repository_comes_first() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repos = repositories(dir.path(), 3);

    let mut recent = RecentRepositories::at(dir.path().join("recent"));
    for repo in &repos {
        recent.record(repo);
    }

    assert_eq!(
        recent.entries(),
        &[repos[2].clone(), repos[1].clone(), repos[0].clone()]
    );
}

#[test]
fn recording_a_known_repository_moves_it_to_the_front_without_duplicating_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repos = repositories(dir.path(), 3);

    let mut recent = RecentRepositories::at(dir.path().join("recent"));
    for repo in &repos {
        recent.record(repo);
    }
    recent.record(&repos[0]);

    assert_eq!(
        recent.entries(),
        &[repos[0].clone(), repos[2].clone(), repos[1].clone()]
    );
}

#[test]
fn the_list_stops_growing_at_its_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repos = repositories(dir.path(), RecentRepositories::LIMIT + 3);

    let mut recent = RecentRepositories::at(dir.path().join("recent"));
    for repo in &repos {
        recent.record(repo);
    }

    assert_eq!(recent.entries().len(), RecentRepositories::LIMIT);
    // The oldest three fell off the end, the newest is still at the front.
    assert_eq!(recent.entries()[0], *repos.last().expect("last"));
    assert!(!recent.entries().contains(&repos[0]));
}

#[test]
fn repositories_that_are_gone_are_dropped_when_the_list_is_read() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("recent");
    let repos = repositories(dir.path(), 2);

    let mut recent = RecentRepositories::at(file.clone());
    recent.record(&repos[0]);
    recent.record(&repos[1]);
    recent.save().expect("save");

    fs::remove_dir_all(&repos[1]).expect("remove repository");

    let reloaded = RecentRepositories::at(file);
    assert_eq!(reloaded.entries(), &[repos[0].clone()]);
}

#[test]
fn a_directory_that_is_no_longer_a_repository_is_dropped_too() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("recent");
    let repos = repositories(dir.path(), 1);

    let mut recent = RecentRepositories::at(file.clone());
    recent.record(&repos[0]);
    recent.save().expect("save");

    fs::remove_dir_all(repos[0].join(".git")).expect("remove git dir");

    let reloaded = RecentRepositories::at(file);
    assert!(reloaded.entries().is_empty());
}

#[test]
fn blank_lines_in_the_file_are_ignored() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("recent");
    let repos = repositories(dir.path(), 1);

    fs::write(&file, format!("\n{}\n\n", repos[0].display())).expect("write");

    let recent = RecentRepositories::at(file);
    assert_eq!(recent.entries(), &[repos[0].clone()]);
}

#[test]
fn saving_creates_the_directory_it_needs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("nested/deeper/recent");
    let repos = repositories(dir.path(), 1);

    let mut recent = RecentRepositories::at(file.clone());
    recent.record(&repos[0]);
    recent.save().expect("save");

    assert!(file.exists());
}

#[test]
fn forgetting_a_repository_removes_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repos = repositories(dir.path(), 2);

    let mut recent = RecentRepositories::at(dir.path().join("recent"));
    recent.record(&repos[0]);
    recent.record(&repos[1]);
    recent.forget(&repos[1]);

    assert_eq!(recent.entries(), &[repos[0].clone()]);
}

#[test]
fn a_worktree_is_remembered_as_itself_not_as_its_primary_checkout() {
    // A linked worktree has a `.git` *file*, not a directory. Remembering it
    // has to work the same way, or half the point of this program falls out of
    // the list.
    let dir = tempfile::tempdir().expect("temp dir");
    let worktree = dir.path().join("feature");
    fs::create_dir_all(&worktree).expect("create dir");
    fs::write(worktree.join(".git"), "gitdir: /elsewhere\n").expect("write git file");

    let mut recent = RecentRepositories::at(dir.path().join("recent"));
    recent.record(&worktree);

    assert_eq!(recent.entries(), &[worktree]);
}
