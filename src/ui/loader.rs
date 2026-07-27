//! Loading a repository without blocking the UI thread.
//!
//! The window must stay responsive while Git is read — on a 50,000-commit
//! repository that takes roughly 400ms, which is many dropped frames. So the
//! work happens on a worker thread and arrives as a finished, immutable
//! snapshot over a channel.

use eframe::egui;

use super::background::{self, Poll, Task};
use crate::application::{GitError, HistoryQuery, LoadRepository, RepositoryReader, WorktreeReader};
use crate::domain::{GraphLayout, RepositorySnapshot};

/// What a worker sends back when it is done.
pub enum LoadMessage {
    Loaded {
        snapshot: RepositorySnapshot,
        layout: GraphLayout,
    },
    Failed(String),
}

/// A load in flight.
pub struct PendingLoad {
    task: Task<LoadMessage>,
}

impl PendingLoad {
    /// Takes the result if the worker has finished, without ever blocking.
    pub fn poll(&mut self) -> Option<LoadMessage> {
        match self.task.poll() {
            Poll::Pending => None,
            Poll::Ready(message) => Some(message),
            Poll::Lost => Some(LoadMessage::Failed(
                "the repository worker stopped unexpectedly".to_string(),
            )),
        }
    }

    pub fn is_running(&self) -> bool {
        self.task.is_running()
    }
}

/// Starts a load on a worker thread.
///
/// The backend is built *inside* the worker: opening a repository is itself
/// disk work, and doing it on the UI thread would stall the first frame.
/// Taking a factory rather than a backend also keeps this module free of any
/// dependency on a concrete Git implementation.
pub fn spawn<B, F>(make_backend: F, query: HistoryQuery, ctx: egui::Context) -> PendingLoad
where
    B: RepositoryReader + WorktreeReader + 'static,
    F: FnOnce() -> Result<B, GitError> + Send + 'static,
{
    let task = background::spawn(
        move || match load(make_backend, &query) {
            Ok((snapshot, layout)) => LoadMessage::Loaded { snapshot, layout },
            Err(error) => LoadMessage::Failed(error.to_string()),
        },
        ctx,
    );

    PendingLoad { task }
}

/// The layout is built here, on the worker, not on the UI thread: it is pure
/// computation over the snapshot and the window should never pay for it.
fn load<B, F>(
    make_backend: F,
    query: &HistoryQuery,
) -> Result<(RepositorySnapshot, GraphLayout), GitError>
where
    B: RepositoryReader + WorktreeReader,
    F: FnOnce() -> Result<B, GitError>,
{
    let backend = make_backend()?;
    let snapshot = LoadRepository::new(&backend, &backend).execute(query)?;
    let layout = GraphLayout::build(&snapshot);
    Ok((snapshot, layout))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::application::ports::CommitPage;
    use crate::domain::{Branch, Commit, Oid, Signature, Timestamp, Worktree};

    struct FakeBackend {
        root: PathBuf,
        commits: Vec<Commit>,
    }

    impl RepositoryReader for FakeBackend {
        fn root(&self) -> &Path {
            &self.root
        }

        fn commits(&self, _query: &HistoryQuery) -> Result<CommitPage, GitError> {
            Ok(CommitPage {
                commits: self.commits.clone(),
                truncated: false,
            })
        }

        fn branches(&self) -> Result<Vec<Branch>, GitError> {
            Ok(vec![])
        }
    }

    impl WorktreeReader for FakeBackend {
        fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
            Ok(vec![])
        }
    }

    fn commit(id: u8, parents: &[u8]) -> Commit {
        let sig = Signature::new("Dev", "dev@example.com", Timestamp::from_utc(id as i64));
        Commit {
            id: Oid::from_bytes([id; 20]),
            parents: parents.iter().map(|p| Oid::from_bytes([*p; 20])).collect(),
            summary: format!("commit {id}"),
            body: String::new(),
            author: sig.clone(),
            committer: sig,
        }
    }

    #[test]
    fn a_successful_load_produces_a_snapshot_and_its_layout() {
        let result = load(
            || {
                Ok(FakeBackend {
                    root: PathBuf::from("/repo"),
                    commits: vec![commit(2, &[1]), commit(1, &[])],
                })
            },
            &HistoryQuery::full(),
        );

        let (snapshot, layout) = result.expect("load succeeds");
        assert_eq!(snapshot.commit_count(), 2);
        assert_eq!(layout.len(), 2);
        assert_eq!(layout.lane_count(), 1);
    }

    #[test]
    fn a_backend_that_cannot_open_the_repository_reports_the_failure() {
        let result = load(
            || Err::<FakeBackend, _>(GitError::NotARepository(PathBuf::from("/nowhere"))),
            &HistoryQuery::full(),
        );

        assert!(matches!(result, Err(GitError::NotARepository(_))));
    }

    #[test]
    fn an_empty_repository_loads_to_an_empty_snapshot_rather_than_an_error() {
        let (snapshot, layout) = load(
            || {
                Ok(FakeBackend {
                    root: PathBuf::from("/repo"),
                    commits: vec![],
                })
            },
            &HistoryQuery::full(),
        )
        .expect("an empty repository is not a failure");

        assert_eq!(snapshot.commit_count(), 0);
        assert!(layout.is_empty());
    }
}
