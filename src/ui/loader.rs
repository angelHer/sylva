//! Loading a repository without blocking the UI thread.
//!
//! The window must stay responsive while Git is read — on a 50,000-commit
//! repository that takes roughly 400ms, which is many dropped frames. So the
//! work happens on a worker thread and arrives as a finished, immutable
//! snapshot over a channel.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

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
    receiver: Receiver<LoadMessage>,
    finished: bool,
}

impl PendingLoad {
    /// Takes the result if the worker has finished, without ever blocking.
    ///
    /// Returns `None` while the load is still running, and keeps returning
    /// `None` once a result has been taken.
    pub fn poll(&mut self) -> Option<LoadMessage> {
        if self.finished {
            return None;
        }

        match self.receiver.try_recv() {
            Ok(message) => {
                self.finished = true;
                Some(message)
            }
            Err(TryRecvError::Empty) => None,
            // The worker died without sending: report it rather than spinning
            // forever on a channel that will never produce anything.
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Some(LoadMessage::Failed(
                    "the repository worker stopped unexpectedly".to_string(),
                ))
            }
        }
    }

    pub fn is_running(&self) -> bool {
        !self.finished
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
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let message = match load(make_backend, &query) {
            Ok((snapshot, layout)) => LoadMessage::Loaded { snapshot, layout },
            Err(error) => LoadMessage::Failed(error.to_string()),
        };

        // If the send fails the window is already gone; there is nothing to
        // report to and nothing to clean up.
        if sender.send(message).is_ok() {
            // Wake the UI thread. Without this the result would sit in the
            // channel until some unrelated input caused a repaint.
            ctx.request_repaint();
        }
    });

    PendingLoad {
        receiver,
        finished: false,
    }
}

/// The layout is built here, on the worker, not on the UI thread: it is pure
/// computation over the snapshot and the window should never pay for it.
fn load<B, F>(make_backend: F, query: &HistoryQuery) -> Result<(RepositorySnapshot, GraphLayout), GitError>
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
    fn polling_a_finished_load_yields_the_result_once() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(LoadMessage::Failed("boom".into()))
            .expect("send");

        let mut pending = PendingLoad {
            receiver,
            finished: false,
        };

        assert!(pending.is_running());
        assert!(matches!(pending.poll(), Some(LoadMessage::Failed(_))));
        assert!(!pending.is_running());
        // Taking it again must not resurrect the result or panic.
        assert!(pending.poll().is_none());
    }

    #[test]
    fn a_worker_that_dies_without_sending_is_reported_rather_than_hung_on() {
        let (sender, receiver) = mpsc::channel::<LoadMessage>();
        drop(sender);

        let mut pending = PendingLoad {
            receiver,
            finished: false,
        };

        match pending.poll() {
            Some(LoadMessage::Failed(message)) => assert!(message.contains("stopped")),
            _ => panic!("expected a failure once the worker is gone"),
        }
        assert!(!pending.is_running());
    }

    #[test]
    fn a_load_still_running_reports_nothing_and_stays_pending() {
        let (_sender, receiver) = mpsc::channel::<LoadMessage>();

        let mut pending = PendingLoad {
            receiver,
            finished: false,
        };

        assert!(pending.poll().is_none());
        assert!(pending.is_running());
    }
}
