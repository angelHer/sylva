//! Integration tests for the worktree write path, run against real
//! repositories. These operations create and delete directories, so nothing
//! here may be verified with a fake.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gitgui::application::worktree_ops::{
    AddWorktree, AddWorktreeRequest, PruneWorktrees, RemoveWorktree, WorktreeOpError,
    WorktreeOperations,
};
use gitgui::application::{HistoryQuery, LoadRepository};
use gitgui::domain::{default_worktree_path, BranchName, RepositorySnapshot};
use gitgui::infrastructure::GitCli;
use gitgui::Git2Backend;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("create project dir");

        let fixture = Self { _dir: dir, root };
        fixture.git(&["init", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.com"]);

        fs::write(fixture.root.join("first.txt"), "one").expect("write");
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "first commit"]);

        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn cli(&self) -> GitCli {
        GitCli::at(&self.root)
    }

    /// A fresh snapshot, so guards are checked against the repository as it is
    /// now rather than as it was when the test started.
    fn snapshot(&self) -> RepositorySnapshot {
        let backend = Git2Backend::discover(&self.root).expect("discover");
        LoadRepository::new(&backend, &backend)
            .execute(&HistoryQuery::full())
            .expect("snapshot")
    }

    fn worktree_path(&self, name: &str) -> PathBuf {
        self.root
            .parent()
            .expect("parent")
            .join(format!("wt-{name}"))
    }

    fn request(&self, branch: &str, create: bool) -> AddWorktreeRequest {
        AddWorktreeRequest {
            branch: BranchName::parse(branch).expect("valid branch"),
            path: self.worktree_path(branch),
            create_branch: create,
        }
    }

    fn worktree_names(&self) -> Vec<String> {
        self.snapshot()
            .worktrees()
            .iter()
            .map(|wt| wt.dir_name().to_string())
            .collect()
    }
}

#[test]
fn adding_a_worktree_creates_the_directory_and_the_branch() {
    let fixture = Fixture::new();
    let cli = fixture.cli();
    let request = fixture.request("feature", true);

    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("the worktree is created");

    assert!(request.path.is_dir(), "the directory exists");
    assert!(request.path.join("first.txt").is_file(), "it is checked out");

    let branches = fixture.git(&["branch", "--list", "feature"]);
    assert!(!branches.is_empty(), "the branch was created");
}

#[test]
fn a_created_worktree_shows_up_in_the_next_snapshot() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &fixture.request("feature", true))
        .expect("created");

    assert!(fixture.worktree_names().iter().any(|n| n == "wt-feature"));
}

#[test]
fn an_existing_branch_can_be_checked_out_into_a_worktree() {
    let fixture = Fixture::new();
    fixture.git(&["branch", "release"]);
    let cli = fixture.cli();

    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &fixture.request("release", false))
        .expect("checked out");

    assert!(fixture.worktree_path("release").is_dir());
}

#[test]
fn a_branch_already_checked_out_is_refused_without_touching_the_disk() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    // `main` is held by the primary checkout.
    let request = fixture.request("main", false);
    let error = AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect_err("git allows one checkout per branch");

    assert!(matches!(
        error,
        WorktreeOpError::BranchAlreadyCheckedOut { .. }
    ));
    assert!(!request.path.exists(), "nothing was written");
}

#[test]
fn a_path_that_is_already_taken_is_refused_without_disturbing_it() {
    let fixture = Fixture::new();
    let cli = fixture.cli();
    let request = fixture.request("feature", true);

    fs::create_dir_all(&request.path).expect("occupy the path");
    fs::write(request.path.join("precious.txt"), "do not lose me").expect("write");

    let error = AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect_err("an occupied path is refused");

    assert!(matches!(error, WorktreeOpError::PathTaken(_)));
    assert_eq!(
        fs::read_to_string(request.path.join("precious.txt")).unwrap(),
        "do not lose me",
        "the existing contents are untouched"
    );
}

#[test]
fn a_clean_worktree_is_removed_from_disk_and_from_the_repository() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    let request = fixture.request("feature", true);
    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("created");

    RemoveWorktree::new(&cli)
        .execute(&fixture.snapshot(), "wt-feature", false)
        .expect("removed");

    assert!(!request.path.exists(), "the directory is gone");
    assert!(!fixture.worktree_names().iter().any(|n| n == "wt-feature"));
}

#[test]
fn a_worktree_holding_uncommitted_work_is_not_removed() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    let request = fixture.request("feature", true);
    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("created");

    let precious = request.path.join("unsaved.txt");
    fs::write(&precious, "work in progress").expect("write");

    let error = RemoveWorktree::new(&cli)
        .execute(&fixture.snapshot(), "wt-feature", false)
        .expect_err("uncommitted work must block removal");

    assert!(matches!(
        error,
        WorktreeOpError::HasUncommittedChanges { .. }
    ));
    assert_eq!(
        fs::read_to_string(&precious).unwrap(),
        "work in progress",
        "the unsaved work is still there"
    );
}

#[test]
fn forcing_removes_a_worktree_that_holds_uncommitted_work() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    let request = fixture.request("feature", true);
    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("created");
    fs::write(request.path.join("unsaved.txt"), "throwaway").expect("write");

    RemoveWorktree::new(&cli)
        .execute(&fixture.snapshot(), "wt-feature", true)
        .expect("forcing removes it");

    assert!(!request.path.exists());
}

#[test]
fn the_primary_checkout_survives_a_removal_attempt() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    let error = RemoveWorktree::new(&cli)
        .execute(&fixture.snapshot(), "project", false)
        .expect_err("the primary checkout cannot be removed");

    assert!(matches!(error, WorktreeOpError::CannotRemovePrimary));
    assert!(fixture.root.is_dir(), "the repository is still there");
    assert!(fixture.root.join("first.txt").is_file());
}

#[test]
fn pruning_drops_the_record_of_a_worktree_whose_directory_was_deleted() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    let request = fixture.request("gone", true);
    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("created");
    fs::remove_dir_all(&request.path).expect("delete it behind git's back");

    assert!(
        fixture.worktree_names().iter().any(|n| n == "wt-gone"),
        "the stale record is still listed before pruning"
    );

    let pruned = PruneWorktrees::new(&cli).execute().expect("pruned");

    assert_eq!(pruned, 1);
    assert!(!fixture.worktree_names().iter().any(|n| n == "wt-gone"));
}

#[test]
fn pruning_a_healthy_repository_removes_nothing() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &fixture.request("feature", true))
        .expect("created");

    assert_eq!(PruneWorktrees::new(&cli).execute().unwrap(), 0);
    assert!(fixture.worktree_path("feature").is_dir(), "still there");
}

#[test]
fn the_default_path_git_would_be_given_is_outside_the_repository() {
    let fixture = Fixture::new();
    let branch = BranchName::parse("feature/graph").expect("valid");
    let path = default_worktree_path(&fixture.root, &branch);

    assert!(!path.starts_with(&fixture.root));
    assert!(path.ends_with("feature-graph"));
}

#[test]
fn a_worktree_created_at_the_default_path_works() {
    let fixture = Fixture::new();
    let cli = fixture.cli();
    let branch = BranchName::parse("feature/graph").expect("valid");

    let request = AddWorktreeRequest {
        path: default_worktree_path(&fixture.root, &branch),
        branch,
        create_branch: true,
    };

    AddWorktree::new(&cli)
        .execute(&fixture.snapshot(), &request)
        .expect("created at the suggested path");

    assert!(request.path.is_dir());
}

#[test]
fn an_occupied_path_is_reported_as_taken_and_a_free_one_as_free() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    assert!(!cli.path_is_free(&fixture.root));
    assert!(cli.path_is_free(&fixture.worktree_path("nothing-here")));
}

#[test]
fn a_failing_git_command_surfaces_gits_own_explanation() {
    let fixture = Fixture::new();
    let cli = fixture.cli();

    // Removing a path git does not know about.
    let error = cli
        .remove(Path::new("/tmp/not-a-worktree-at-all"), false)
        .expect_err("git refuses");

    let message = error.to_string();
    assert!(
        message.contains("worktree") || message.contains("not a"),
        "expected git's own wording, got {message:?}"
    );
}
