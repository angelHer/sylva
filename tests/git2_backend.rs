//! Integration tests for the libgit2 adapter, run against real repositories
//! built with the `git` CLI. Fakes cannot catch the things that actually break
//! here — worktree resolution, detached HEADs, status flags.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sylva::application::ports::{HistoryQuery, RepositoryReader, WorktreeReader};
use sylva::domain::WorktreeHead;
use sylva::Git2Backend;

/// A throwaway repository on disk. The `TempDir` is kept alive by the struct;
/// dropping it removes everything.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    /// A repository with three commits on `main`.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("create project dir");

        let fixture = Self { _dir: dir, root };

        fixture.git(&["init", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.com"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);

        fixture.commit_file("first.txt", "one", "first commit");
        fixture.commit_file("second.txt", "two", "second commit");
        fixture.commit_file(
            "third.txt",
            "three",
            "third commit\n\nWith a body explaining things.",
        );

        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.root, args)
    }

    fn git_in(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));

        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.root.join(name), contents).expect("write file");
    }

    fn commit_file(&self, name: &str, contents: &str, message: &str) {
        self.write(name, contents);
        self.git(&["add", name]);
        self.git(&["commit", "-m", message]);
    }

    /// Adds a linked worktree next to the project, mirroring the sibling
    /// layout the tool is designed around.
    fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.worktree_path(name);
        self.git(&[
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str().expect("utf-8 path"),
        ]);
        path
    }

    fn worktree_path(&self, name: &str) -> PathBuf {
        self.root
            .parent()
            .expect("parent dir")
            .join(format!("project-worktrees-{name}"))
    }

    fn head_oid(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn backend(&self) -> Git2Backend {
        Git2Backend::discover(&self.root).expect("discover repository")
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

#[test]
fn discovers_the_repository_and_reports_its_working_directory() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    assert!(same_path(backend.root(), &fixture.root));
}

#[test]
fn discovering_from_a_subdirectory_finds_the_repository_root() {
    let fixture = Fixture::new();
    let nested = fixture.root.join("src/deep");
    fs::create_dir_all(&nested).expect("nested dirs");

    let backend = Git2Backend::discover(&nested).expect("discover from subdirectory");

    assert!(same_path(backend.root(), &fixture.root));
}

#[test]
fn discovering_from_inside_a_linked_worktree_resolves_to_the_primary_checkout() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature", "feature/graph");

    let backend = Git2Backend::discover(&worktree).expect("discover from worktree");

    assert!(
        same_path(backend.root(), &fixture.root),
        "expected the primary checkout at {:?}, got {:?}",
        fixture.root,
        backend.root()
    );
}

#[test]
fn a_path_outside_any_repository_is_rejected() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(Git2Backend::discover(dir.path()).is_err());
}

#[test]
fn walks_history_newest_first_and_splits_the_commit_message() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let page = backend.commits(&HistoryQuery::full()).expect("commits");

    assert_eq!(page.commits.len(), 3);
    assert!(!page.truncated);
    assert_eq!(page.commits[0].summary, "third commit");
    assert_eq!(page.commits[0].body, "With a body explaining things.");
    assert_eq!(page.commits[2].summary, "first commit");
    assert!(page.commits[2].is_root());
}

#[test]
fn parent_links_match_the_recorded_history() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let page = backend.commits(&HistoryQuery::full()).unwrap();

    assert_eq!(page.commits[0].parents, vec![page.commits[1].id]);
    assert_eq!(page.commits[1].parents, vec![page.commits[2].id]);
    assert!(page.commits[2].parents.is_empty());
}

#[test]
fn a_bounded_walk_stops_at_the_limit_and_reports_truncation() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let page = backend
        .commits(&HistoryQuery::full().with_max_commits(Some(2)))
        .unwrap();

    assert_eq!(page.commits.len(), 2);
    assert!(page.truncated);
}

#[test]
fn a_limit_equal_to_the_history_length_is_not_truncation() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let page = backend
        .commits(&HistoryQuery::full().with_max_commits(Some(3)))
        .unwrap();

    assert_eq!(page.commits.len(), 3);
    assert!(!page.truncated);
}

#[test]
fn an_empty_repository_walks_to_nothing_instead_of_failing() {
    let dir = tempfile::tempdir().expect("temp dir");
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .expect("git init");

    let backend = Git2Backend::discover(dir.path()).expect("discover empty repo");
    let page = backend.commits(&HistoryQuery::full()).expect("commits");

    assert!(page.commits.is_empty());
    assert!(!page.truncated);
}

#[test]
fn lists_local_branches_and_marks_the_checked_out_one() {
    let fixture = Fixture::new();
    fixture.git(&["branch", "release"]);
    let backend = fixture.backend();

    let branches = backend.branches().expect("branches");

    let main = branches
        .iter()
        .find(|b| b.name == "main")
        .expect("main branch");
    assert!(main.is_local());
    assert!(main.is_head);

    let release = branches
        .iter()
        .find(|b| b.name == "release")
        .expect("release branch");
    assert!(!release.is_head);
}

#[test]
fn a_branch_without_an_upstream_reports_no_divergence() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let branches = backend.branches().unwrap();
    let main = branches.iter().find(|b| b.name == "main").unwrap();

    assert_eq!(main.upstream, None);
    assert_eq!(main.divergence, None);
}

#[test]
fn the_primary_checkout_is_reported_as_a_worktree() {
    let fixture = Fixture::new();
    let backend = fixture.backend();

    let worktrees = backend.worktrees().expect("worktrees");

    assert_eq!(worktrees.len(), 1);
    let primary = &worktrees[0];
    assert!(primary.is_primary);
    assert!(same_path(&primary.path, &fixture.root));
    assert_eq!(primary.head.branch_name(), Some("main"));
    assert!(primary.status.is_clean());
}

#[test]
fn linked_worktrees_are_listed_with_their_branches() {
    let fixture = Fixture::new();
    fixture.add_worktree("feature", "feature/graph");
    let backend = fixture.backend();

    let worktrees = backend.worktrees().expect("worktrees");

    assert_eq!(worktrees.len(), 2);
    let linked = worktrees
        .iter()
        .find(|wt| !wt.is_primary)
        .expect("linked worktree");
    assert_eq!(linked.head.branch_name(), Some("feature/graph"));
    assert!(!linked.is_locked);
    assert!(!linked.is_prunable);
}

#[test]
fn each_worktree_anchors_to_its_own_commit() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature", "feature/graph");

    // Move the linked worktree one commit ahead of main.
    fs::write(worktree.join("feature.txt"), "work").expect("write");
    fixture.git_in(&worktree, &["add", "feature.txt"]);
    fixture.git_in(&worktree, &["commit", "-m", "feature commit"]);

    let backend = fixture.backend();
    let worktrees = backend.worktrees().unwrap();

    let primary = worktrees.iter().find(|wt| wt.is_primary).unwrap();
    let linked = worktrees.iter().find(|wt| !wt.is_primary).unwrap();

    assert_ne!(
        primary.target(),
        linked.target(),
        "the two worktrees must anchor to different commits"
    );
}

#[test]
fn dirty_state_is_attributed_to_the_worktree_it_belongs_to() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature", "feature/graph");

    // One staged file and one untracked file, in the linked worktree only.
    fs::write(worktree.join("first.txt"), "changed").expect("write");
    fixture.git_in(&worktree, &["add", "first.txt"]);
    fs::write(worktree.join("scratch.txt"), "notes").expect("write");

    let backend = fixture.backend();
    let worktrees = backend.worktrees().unwrap();

    let primary = worktrees.iter().find(|wt| wt.is_primary).unwrap();
    let linked = worktrees.iter().find(|wt| !wt.is_primary).unwrap();

    assert!(primary.status.is_clean(), "primary must stay clean");
    assert_eq!(linked.status.staged, 1);
    assert_eq!(linked.status.untracked, 1);
    assert!(linked.status.is_dirty());
    assert!(linked.needs_attention());
}

#[test]
fn a_detached_worktree_is_reported_as_detached() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("detached", "temp/branch");
    let head = fixture.head_oid();
    fixture.git_in(&worktree, &["checkout", "--detach", &head]);

    let backend = fixture.backend();
    let worktrees = backend.worktrees().unwrap();

    let linked = worktrees.iter().find(|wt| !wt.is_primary).unwrap();
    assert!(matches!(linked.head, WorktreeHead::Detached { .. }));
    assert!(linked.target().is_some());
}

#[test]
fn a_commit_reachable_only_from_a_detached_worktree_still_enters_the_graph() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("detached", "temp/branch");

    // Detach, commit, then delete the branch: the new commit is now reachable
    // from nothing but this worktree's HEAD. It must not vanish from the graph.
    let head = fixture.head_oid();
    fixture.git_in(&worktree, &["checkout", "--detach", &head]);
    fs::write(worktree.join("orphan.txt"), "loose").expect("write");
    fixture.git_in(&worktree, &["add", "orphan.txt"]);
    fixture.git_in(&worktree, &["commit", "-m", "detached commit"]);
    fixture.git(&["branch", "-D", "temp/branch"]);

    let backend = fixture.backend();
    let page = backend.commits(&HistoryQuery::full()).unwrap();

    assert!(
        page.commits.iter().any(|c| c.summary == "detached commit"),
        "the detached worktree's commit is missing from the walk"
    );
}

#[test]
fn a_worktree_whose_directory_was_deleted_is_reported_as_prunable() {
    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("gone", "temp/gone");
    fs::remove_dir_all(&worktree).expect("remove worktree dir");

    let backend = fixture.backend();
    let worktrees = backend.worktrees().unwrap();

    let stale = worktrees
        .iter()
        .find(|wt| !wt.is_primary)
        .expect("stale worktree still listed");
    assert!(stale.is_prunable);
    assert!(stale.needs_attention());
}

#[test]
fn a_full_snapshot_ties_worktrees_to_the_commits_they_sit_on() {
    use sylva::{HistoryQuery as Query, LoadRepository};

    let fixture = Fixture::new();
    let worktree = fixture.add_worktree("feature", "feature/graph");
    fs::write(worktree.join("feature.txt"), "work").expect("write");
    fixture.git_in(&worktree, &["add", "feature.txt"]);
    fixture.git_in(&worktree, &["commit", "-m", "feature commit"]);

    let backend = fixture.backend();
    let snapshot = LoadRepository::new(&backend, &backend)
        .execute(&Query::full())
        .expect("snapshot");

    assert_eq!(snapshot.commit_count(), 4);
    assert_eq!(snapshot.worktrees().len(), 2);

    let anchors = snapshot.worktree_anchors();
    assert_eq!(anchors.len(), 2, "each worktree anchors to its own commit");

    for worktree in snapshot.worktrees() {
        let target = worktree.target().expect("worktree has a target");
        assert!(
            snapshot.contains(&target),
            "worktree {} anchors to a commit missing from the graph",
            worktree.name
        );
    }
}
