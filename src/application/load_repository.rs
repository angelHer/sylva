use crate::domain::RepositorySnapshot;

use super::ports::{GitError, HistoryQuery, RepositoryReader, WorktreeReader};

/// Builds one immutable snapshot from the read ports.
///
/// This runs on a worker thread. It is the only place that combines history,
/// refs and worktrees, so the UI receives a set of facts that were all read
/// from the same repository state.
pub struct LoadRepository<'a> {
    repository: &'a dyn RepositoryReader,
    worktrees: &'a dyn WorktreeReader,
}

impl<'a> LoadRepository<'a> {
    pub fn new(repository: &'a dyn RepositoryReader, worktrees: &'a dyn WorktreeReader) -> Self {
        Self {
            repository,
            worktrees,
        }
    }

    pub fn execute(&self, query: &HistoryQuery) -> Result<RepositorySnapshot, GitError> {
        let page = self.repository.commits(query)?;
        let branches = self.repository.branches()?;
        let worktrees = self.worktrees.worktrees()?;

        Ok(RepositorySnapshot::new(
            self.repository.root().to_path_buf(),
            page.commits,
            branches,
            worktrees,
            page.truncated,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::application::ports::CommitPage;
    use crate::domain::{
        Branch, BranchKind, Commit, Oid, Signature, Timestamp, Worktree, WorktreeHead,
        WorktreeStatus,
    };

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    fn commit(id: u8, parents: &[u8]) -> Commit {
        let sig = Signature::new("Dev", "dev@example.com", Timestamp::from_utc(id as i64));
        Commit {
            id: oid(id),
            parents: parents.iter().copied().map(oid).collect(),
            summary: format!("commit {id}"),
            body: String::new(),
            author: sig.clone(),
            committer: sig,
        }
    }

    fn worktree(name: &str, primary: bool, target: u8) -> Worktree {
        Worktree {
            name: name.into(),
            path: PathBuf::from(format!("/repo/{name}")),
            is_primary: primary,
            head: WorktreeHead::Branch {
                name: name.into(),
                target: oid(target),
            },
            status: WorktreeStatus::default(),
            divergence: None,
            is_locked: false,
            is_prunable: false,
        }
    }

    struct FakeRepository {
        root: PathBuf,
        page: CommitPage,
        branches: Vec<Branch>,
        fail_on_branches: bool,
    }

    impl FakeRepository {
        fn new(commits: Vec<Commit>, truncated: bool) -> Self {
            Self {
                root: PathBuf::from("/repo"),
                page: CommitPage {
                    commits,
                    truncated,
                },
                branches: vec![Branch {
                    name: "main".into(),
                    kind: BranchKind::Local,
                    target: oid(2),
                    upstream: None,
                    divergence: None,
                    is_head: true,
                }],
                fail_on_branches: false,
            }
        }
    }

    impl RepositoryReader for FakeRepository {
        fn root(&self) -> &Path {
            &self.root
        }

        fn commits(&self, query: &HistoryQuery) -> Result<CommitPage, GitError> {
            let mut commits = self.page.commits.clone();
            let mut truncated = self.page.truncated;
            if let Some(max) = query.max_commits {
                if commits.len() > max {
                    commits.truncate(max);
                    truncated = true;
                }
            }
            Ok(CommitPage { commits, truncated })
        }

        fn branches(&self) -> Result<Vec<Branch>, GitError> {
            if self.fail_on_branches {
                return Err(GitError::backend("branches", "boom"));
            }
            Ok(self.branches.clone())
        }
    }

    struct FakeWorktrees {
        worktrees: Vec<Worktree>,
        failure: Option<GitError>,
    }

    impl FakeWorktrees {
        fn with(worktrees: Vec<Worktree>) -> Self {
            Self {
                worktrees,
                failure: None,
            }
        }

        fn failing() -> Self {
            Self {
                worktrees: vec![],
                failure: Some(GitError::backend("worktrees", "disk gone")),
            }
        }
    }

    impl WorktreeReader for FakeWorktrees {
        fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
            match &self.failure {
                Some(GitError::Backend { context, message }) => {
                    Err(GitError::backend(context.clone(), message.clone()))
                }
                Some(_) => Err(GitError::backend("worktrees", "failed")),
                None => Ok(self.worktrees.clone()),
            }
        }
    }

    #[test]
    fn combines_history_refs_and_worktrees_into_one_snapshot() {
        let repo = FakeRepository::new(vec![commit(2, &[1]), commit(1, &[])], false);
        let worktrees = FakeWorktrees::with(vec![
            worktree("main", true, 2),
            worktree("feature", false, 1),
        ]);

        let snapshot = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::full())
            .expect("snapshot");

        assert_eq!(snapshot.root(), &PathBuf::from("/repo"));
        assert_eq!(snapshot.commit_count(), 2);
        assert_eq!(snapshot.branches().len(), 1);
        assert_eq!(snapshot.worktrees().len(), 2);
        assert!(!snapshot.is_truncated());
    }

    #[test]
    fn worktrees_land_on_the_commits_they_point_at() {
        let repo = FakeRepository::new(vec![commit(2, &[1]), commit(1, &[])], false);
        let worktrees = FakeWorktrees::with(vec![
            worktree("main", true, 2),
            worktree("feature", false, 1),
        ]);

        let snapshot = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::full())
            .unwrap();

        assert_eq!(snapshot.worktrees_at(&oid(2))[0].name, "main");
        assert_eq!(snapshot.worktrees_at(&oid(1))[0].name, "feature");
    }

    #[test]
    fn a_bounded_query_truncates_history_and_says_so() {
        let repo = FakeRepository::new(vec![commit(3, &[2]), commit(2, &[1]), commit(1, &[])], false);
        let worktrees = FakeWorktrees::with(vec![]);

        let snapshot = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::default().with_max_commits(Some(2)))
            .unwrap();

        assert_eq!(snapshot.commit_count(), 2);
        assert!(snapshot.is_truncated());
        // The parent of the oldest loaded commit is absent: the renderer must
        // draw that edge as dangling.
        assert!(!snapshot.contains(&oid(1)));
    }

    #[test]
    fn an_empty_repository_yields_an_empty_snapshot_rather_than_an_error() {
        let repo = FakeRepository::new(vec![], false);
        let worktrees = FakeWorktrees::with(vec![]);

        let snapshot = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::full())
            .unwrap();

        assert_eq!(snapshot.commit_count(), 0);
        assert!(snapshot.worktrees().is_empty());
    }

    #[test]
    fn a_failing_worktree_read_fails_the_whole_load() {
        let repo = FakeRepository::new(vec![commit(1, &[])], false);
        let worktrees = FakeWorktrees::failing();

        let error = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::full())
            .unwrap_err();

        assert!(matches!(error, GitError::Backend { .. }));
    }

    #[test]
    fn a_failing_branch_read_fails_the_whole_load() {
        let mut repo = FakeRepository::new(vec![commit(1, &[])], false);
        repo.fail_on_branches = true;
        let worktrees = FakeWorktrees::with(vec![]);

        let error = LoadRepository::new(&repo, &worktrees)
            .execute(&HistoryQuery::full())
            .unwrap_err();

        assert!(matches!(error, GitError::Backend { .. }));
    }
}
