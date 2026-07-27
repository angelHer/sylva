//! Creating, removing and pruning worktrees.
//!
//! Everything here is a guard. `git worktree` will happily remove a checkout
//! holding uncommitted work, and will create one at a path that turns out to
//! be nested inside the repository. The cost of getting it wrong is a
//! developer's unsaved work, so the refusals live in one tested place rather
//! than in whichever button happened to be wired up.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::domain::{BranchName, RepositorySnapshot};

use super::ports::GitError;

/// What to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddWorktreeRequest {
    pub branch: BranchName,
    pub path: PathBuf,
    /// True to create the branch, false to check out one that already exists.
    pub create_branch: bool,
}

#[derive(Debug)]
pub enum WorktreeOpError {
    Git(GitError),
    /// Something is already at the target path.
    PathTaken(PathBuf),
    /// The path is inside the repository, where a worktree must never go.
    PathInsideRepository(PathBuf),
    /// Creating a branch that is already there.
    BranchExists(String),
    /// Checking out a branch that is not there.
    BranchMissing(String),
    /// Git allows one checkout per branch.
    BranchAlreadyCheckedOut { branch: String, worktree: String },
    /// The primary checkout is the repository; it cannot be removed like a
    /// linked worktree.
    CannotRemovePrimary,
    /// Removing a worktree that still holds work.
    HasUncommittedChanges { worktree: String, files: usize },
    UnknownWorktree(String),
}

impl fmt::Display for WorktreeOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => write!(f, "{error}"),
            Self::PathTaken(path) => write!(f, "{} already exists", path.display()),
            Self::PathInsideRepository(path) => write!(
                f,
                "{} is inside the repository; a worktree there would show up as untracked files",
                path.display()
            ),
            Self::BranchExists(branch) => write!(f, "branch {branch} already exists"),
            Self::BranchMissing(branch) => write!(f, "no branch named {branch}"),
            Self::BranchAlreadyCheckedOut { branch, worktree } => {
                write!(f, "{branch} is already checked out in {worktree}")
            }
            Self::CannotRemovePrimary => {
                write!(f, "the primary checkout cannot be removed")
            }
            Self::HasUncommittedChanges { worktree, files } => write!(
                f,
                "{worktree} has {files} uncommitted file(s); removing it would discard them"
            ),
            Self::UnknownWorktree(name) => write!(f, "no worktree named {name}"),
        }
    }
}

impl std::error::Error for WorktreeOpError {}

impl From<GitError> for WorktreeOpError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

/// The write side of the worktree set.
///
/// Separate from the read ports because it is the only part of this client
/// that changes anything, and because it is backed by the `git` command rather
/// than libgit2 — `git worktree` does bookkeeping libgit2 leaves to the caller.
pub trait WorktreeOperations: Send + Sync {
    fn add(&self, request: &AddWorktreeRequest) -> Result<(), GitError>;

    /// Removes a linked worktree. `force` discards uncommitted work.
    fn remove(&self, path: &Path, force: bool) -> Result<(), GitError>;

    /// Drops the administrative records of worktrees whose directories are
    /// gone. Returns how many were pruned.
    fn prune(&self) -> Result<usize, GitError>;

    /// Whether anything already exists at a path.
    ///
    /// Behind the port because it is a question about the world, and because
    /// the tests for the guards must be able to answer it without a disk.
    fn path_is_free(&self, path: &Path) -> bool;
}

pub struct AddWorktree<'a> {
    operations: &'a dyn WorktreeOperations,
}

impl<'a> AddWorktree<'a> {
    pub fn new(operations: &'a dyn WorktreeOperations) -> Self {
        Self { operations }
    }

    pub fn execute(
        &self,
        snapshot: &RepositorySnapshot,
        request: &AddWorktreeRequest,
    ) -> Result<(), WorktreeOpError> {
        let branch = request.branch.as_str();

        // A worktree inside its own repository is a trap: every status in
        // every other checkout fills with its files.
        if request.path.starts_with(snapshot.root()) {
            return Err(WorktreeOpError::PathInsideRepository(request.path.clone()));
        }

        if !self.operations.path_is_free(&request.path) {
            return Err(WorktreeOpError::PathTaken(request.path.clone()));
        }

        let exists = snapshot
            .branches()
            .iter()
            .any(|candidate| candidate.is_local() && candidate.name == branch);

        if request.create_branch && exists {
            return Err(WorktreeOpError::BranchExists(branch.to_string()));
        }
        if !request.create_branch && !exists {
            return Err(WorktreeOpError::BranchMissing(branch.to_string()));
        }

        // Git allows a branch to be checked out in exactly one worktree.
        if let Some(holder) = snapshot
            .worktrees()
            .iter()
            .find(|worktree| worktree.head.branch_name() == Some(branch))
        {
            return Err(WorktreeOpError::BranchAlreadyCheckedOut {
                branch: branch.to_string(),
                worktree: holder.dir_name().to_string(),
            });
        }

        self.operations.add(request)?;
        Ok(())
    }
}

pub struct RemoveWorktree<'a> {
    operations: &'a dyn WorktreeOperations,
}

impl<'a> RemoveWorktree<'a> {
    pub fn new(operations: &'a dyn WorktreeOperations) -> Self {
        Self { operations }
    }

    /// Removes the worktree with this directory name.
    ///
    /// `force` is the caller confirming that uncommitted work may be
    /// discarded. Without it, a dirty worktree is refused rather than
    /// silently emptied.
    pub fn execute(
        &self,
        snapshot: &RepositorySnapshot,
        dir_name: &str,
        force: bool,
    ) -> Result<(), WorktreeOpError> {
        let worktree = snapshot
            .worktrees()
            .iter()
            .find(|worktree| worktree.dir_name() == dir_name)
            .ok_or_else(|| WorktreeOpError::UnknownWorktree(dir_name.to_string()))?;

        if worktree.is_primary {
            return Err(WorktreeOpError::CannotRemovePrimary);
        }

        if worktree.status.is_dirty() && !force {
            return Err(WorktreeOpError::HasUncommittedChanges {
                worktree: worktree.dir_name().to_string(),
                files: worktree.status.total_changed(),
            });
        }

        self.operations.remove(&worktree.path, force)?;
        Ok(())
    }
}

pub struct PruneWorktrees<'a> {
    operations: &'a dyn WorktreeOperations,
}

impl<'a> PruneWorktrees<'a> {
    pub fn new(operations: &'a dyn WorktreeOperations) -> Self {
        Self { operations }
    }

    /// Drops records for worktrees whose directories are gone. Never touches
    /// a directory that still exists, so it needs no confirmation.
    pub fn execute(&self) -> Result<usize, WorktreeOpError> {
        Ok(self.operations.prune()?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::{
        Branch, BranchKind, Oid, Worktree, WorktreeHead, WorktreeStatus,
    };

    #[derive(Default)]
    struct FakeOperations {
        taken_paths: Vec<PathBuf>,
        added: Mutex<Vec<AddWorktreeRequest>>,
        removed: Mutex<Vec<(PathBuf, bool)>>,
        pruned: Mutex<usize>,
    }

    impl WorktreeOperations for FakeOperations {
        fn add(&self, request: &AddWorktreeRequest) -> Result<(), GitError> {
            self.added.lock().unwrap().push(request.clone());
            Ok(())
        }

        fn remove(&self, path: &Path, force: bool) -> Result<(), GitError> {
            self.removed.lock().unwrap().push((path.to_path_buf(), force));
            Ok(())
        }

        fn prune(&self) -> Result<usize, GitError> {
            *self.pruned.lock().unwrap() += 1;
            Ok(2)
        }

        fn path_is_free(&self, path: &Path) -> bool {
            !self.taken_paths.iter().any(|taken| taken == path)
        }
    }

    fn branch(name: &str) -> Branch {
        Branch {
            name: name.into(),
            kind: BranchKind::Local,
            target: Oid::zero(),
            upstream: None,
            divergence: None,
            is_head: false,
        }
    }

    fn worktree(dir: &str, primary: bool, branch: Option<&str>) -> Worktree {
        Worktree {
            name: dir.into(),
            path: PathBuf::from(format!("/home/dev/{dir}")),
            is_primary: primary,
            head: match branch {
                Some(name) => WorktreeHead::Branch {
                    name: name.into(),
                    target: Oid::zero(),
                },
                None => WorktreeHead::Detached {
                    target: Oid::zero(),
                },
            },
            status: WorktreeStatus::default(),
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    fn snapshot(branches: Vec<Branch>, worktrees: Vec<Worktree>) -> RepositorySnapshot {
        RepositorySnapshot::new(
            PathBuf::from("/home/dev/project"),
            vec![],
            branches,
            worktrees,
            false,
        )
    }

    fn request(name: &str, create: bool) -> AddWorktreeRequest {
        AddWorktreeRequest {
            branch: BranchName::parse(name).unwrap(),
            path: PathBuf::from(format!("/home/dev/project-worktrees/{name}")),
            create_branch: create,
        }
    }

    #[test]
    fn a_new_branch_at_a_free_path_is_created() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![branch("main")], vec![worktree("project", true, Some("main"))]);

        AddWorktree::new(&operations)
            .execute(&snap, &request("feature", true))
            .expect("the worktree is created");

        assert_eq!(operations.added.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_path_inside_the_repository_is_refused() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![], vec![]);

        let mut nested = request("feature", true);
        nested.path = PathBuf::from("/home/dev/project/worktrees/feature");

        let error = AddWorktree::new(&operations)
            .execute(&snap, &nested)
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::PathInsideRepository(_)));
        assert!(operations.added.lock().unwrap().is_empty(), "nothing was created");
    }

    #[test]
    fn an_occupied_path_is_refused_before_git_is_asked() {
        let operations = FakeOperations {
            taken_paths: vec![PathBuf::from("/home/dev/project-worktrees/feature")],
            ..Default::default()
        };
        let snap = snapshot(vec![], vec![]);

        let error = AddWorktree::new(&operations)
            .execute(&snap, &request("feature", true))
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::PathTaken(_)));
        assert!(operations.added.lock().unwrap().is_empty());
    }

    #[test]
    fn creating_a_branch_that_already_exists_is_refused() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![branch("feature")], vec![]);

        let error = AddWorktree::new(&operations)
            .execute(&snap, &request("feature", true))
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::BranchExists(_)));
    }

    #[test]
    fn checking_out_a_branch_that_does_not_exist_is_refused() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![branch("main")], vec![]);

        let error = AddWorktree::new(&operations)
            .execute(&snap, &request("feature", false))
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::BranchMissing(_)));
    }

    #[test]
    fn checking_out_an_existing_branch_is_allowed() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![branch("feature")], vec![]);

        AddWorktree::new(&operations)
            .execute(&snap, &request("feature", false))
            .expect("an existing branch can be checked out");

        assert_eq!(operations.added.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_branch_already_checked_out_elsewhere_is_refused_and_names_the_holder() {
        let operations = FakeOperations::default();
        let snap = snapshot(
            vec![branch("feature")],
            vec![worktree("other", false, Some("feature"))],
        );

        let error = AddWorktree::new(&operations)
            .execute(&snap, &request("feature", false))
            .unwrap_err();

        match error {
            WorktreeOpError::BranchAlreadyCheckedOut { worktree, .. } => {
                assert_eq!(worktree, "other");
            }
            other => panic!("expected a checked-out clash, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_linked_worktree_is_removed() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![], vec![worktree("feature", false, Some("feature"))]);

        RemoveWorktree::new(&operations)
            .execute(&snap, "feature", false)
            .expect("a clean worktree is removed");

        assert_eq!(operations.removed.lock().unwrap().len(), 1);
        assert!(!operations.removed.lock().unwrap()[0].1, "no force was needed");
    }

    #[test]
    fn the_primary_checkout_is_never_removed() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![], vec![worktree("project", true, Some("main"))]);

        let error = RemoveWorktree::new(&operations)
            .execute(&snap, "project", false)
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::CannotRemovePrimary));
        assert!(operations.removed.lock().unwrap().is_empty());
    }

    #[test]
    fn forcing_does_not_make_the_primary_checkout_removable() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![], vec![worktree("project", true, Some("main"))]);

        let error = RemoveWorktree::new(&operations)
            .execute(&snap, "project", true)
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::CannotRemovePrimary));
        assert!(operations.removed.lock().unwrap().is_empty());
    }

    #[test]
    fn a_worktree_holding_uncommitted_work_is_refused_and_says_how_much() {
        let operations = FakeOperations::default();
        let mut dirty = worktree("feature", false, Some("feature"));
        dirty.status.unstaged = 2;
        dirty.status.untracked = 1;
        let snap = snapshot(vec![], vec![dirty]);

        let error = RemoveWorktree::new(&operations)
            .execute(&snap, "feature", false)
            .unwrap_err();

        match error {
            WorktreeOpError::HasUncommittedChanges { files, .. } => assert_eq!(files, 3),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(operations.removed.lock().unwrap().is_empty(), "nothing was deleted");
    }

    #[test]
    fn a_dirty_worktree_is_removed_only_when_the_caller_forces_it() {
        let operations = FakeOperations::default();
        let mut dirty = worktree("feature", false, Some("feature"));
        dirty.status.unstaged = 2;
        let snap = snapshot(vec![], vec![dirty]);

        RemoveWorktree::new(&operations)
            .execute(&snap, "feature", true)
            .expect("forcing removes it");

        assert!(operations.removed.lock().unwrap()[0].1, "force was passed through");
    }

    #[test]
    fn removing_an_unknown_worktree_is_an_error_not_a_silent_success() {
        let operations = FakeOperations::default();
        let snap = snapshot(vec![], vec![]);

        let error = RemoveWorktree::new(&operations)
            .execute(&snap, "ghost", false)
            .unwrap_err();

        assert!(matches!(error, WorktreeOpError::UnknownWorktree(_)));
    }

    #[test]
    fn pruning_reports_how_many_records_were_dropped() {
        let operations = FakeOperations::default();
        assert_eq!(PruneWorktrees::new(&operations).execute().unwrap(), 2);
        assert_eq!(*operations.pruned.lock().unwrap(), 1);
    }
}
