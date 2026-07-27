//! Noticing that the repository changed underneath us.
//!
//! # What is watched, and what is not
//!
//! Only the repository's common Git directory, recursively. That covers
//! everything that changes the graph: commits, branch creation and deletion,
//! HEAD moving, and the index of every linked worktree — a linked worktree's
//! administrative directory lives inside the primary one.
//!
//! It deliberately does **not** watch the working directories. Watching a
//! developer's checkout recursively means watching `node_modules`, `target`
//! and every build artefact, which costs more than everything else this
//! program does put together. The gap is that editing a file without staging
//! it changes nothing under `.git`, so the dirty count does not move on its
//! own. The window reloads when it regains focus, which covers the way this
//! actually gets used: edit in an editor, switch back here.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::application::ports::GitError;

/// How long the filesystem must be quiet before a change is reported.
///
/// A single `git commit` produces a burst of writes — lock files, refs, the
/// index, reflogs. Reloading on each one would mean several full reads of the
/// repository for one user action.
const QUIET_PERIOD: Duration = Duration::from_millis(250);

pub struct RepositoryWatcher {
    /// Held only to keep the watch alive; dropping it stops the watching.
    _watcher: RecommendedWatcher,
    changes: Receiver<()>,
}

impl RepositoryWatcher {
    /// Starts watching the Git directory of the repository at `root`.
    pub fn watch(root: &Path) -> Result<Self, GitError> {
        let git_dir = git_dir_of(root)
            .ok_or_else(|| GitError::backend("watch", "no .git directory to watch"))?;

        let (raw_sender, raw_receiver) = mpsc::channel();
        let (sender, changes) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |event| {
            // A send failure means the debouncer is gone, which happens only
            // while shutting down. Nothing to report to.
            let _ = raw_sender.send(event);
        })
        .map_err(|error| GitError::backend("watch", error.to_string()))?;

        watcher
            .watch(&git_dir, RecursiveMode::Recursive)
            .map_err(|error| GitError::backend("watch", error.to_string()))?;

        thread::spawn(move || debounce(raw_receiver, sender));

        Ok(Self {
            _watcher: watcher,
            changes,
        })
    }

    /// Whether the repository changed since this was last asked.
    ///
    /// Drains the queue, so a burst that produced several notifications still
    /// means one reload. Never blocks.
    pub fn take_change(&self) -> bool {
        let mut changed = false;
        loop {
            match self.changes.try_recv() {
                Ok(()) => changed = true,
                Err(TryRecvError::Empty) => return changed,
                // The debouncer stopped. Report what was already seen; there
                // will simply be no more notifications.
                Err(TryRecvError::Disconnected) => return changed,
            }
        }
    }
}

/// Collapses a burst of filesystem events into one notification, sent once the
/// filesystem has been quiet for [`QUIET_PERIOD`].
fn debounce<T>(events: Receiver<T>, notifications: mpsc::Sender<()>) {
    let mut pending = false;

    loop {
        match events.recv_timeout(QUIET_PERIOD) {
            Ok(_) => pending = true,
            Err(RecvTimeoutError::Timeout) => {
                if pending {
                    pending = false;
                    if notifications.send(()).is_err() {
                        return; // The window is gone.
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Report anything still outstanding before stopping, so a
                // change immediately before shutdown is not swallowed.
                if pending {
                    let _ = notifications.send(());
                }
                return;
            }
        }
    }
}

/// The Git directory for a working directory.
///
/// In a normal checkout `.git` is a directory. This only ever runs against the
/// primary checkout, whose `.git` is always the real directory, so the `.git`
/// *file* used by linked worktrees needs no handling here.
fn git_dir_of(root: &Path) -> Option<PathBuf> {
    let candidate = root.join(".git");
    if candidate.is_dir() {
        Some(candidate)
    } else if root.ends_with(".git") && root.is_dir() {
        // A bare repository: the root is the Git directory.
        Some(root.to_path_buf())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_burst_of_events_becomes_a_single_notification() {
        let (events, event_receiver) = channel::<u8>();
        let (notifications, notification_receiver) = channel();

        let worker = thread::spawn(move || debounce(event_receiver, notifications));

        for _ in 0..50 {
            events.send(1).expect("send");
        }
        drop(events);
        worker.join().expect("debouncer finishes");

        let mut count = 0;
        while notification_receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1, "one user action must mean one reload");
    }

    #[test]
    fn silence_produces_no_notification_at_all() {
        let (events, event_receiver) = channel::<u8>();
        let (notifications, notification_receiver) = channel();

        let worker = thread::spawn(move || debounce(event_receiver, notifications));
        drop(events);
        worker.join().expect("debouncer finishes");

        assert!(notification_receiver.try_recv().is_err());
    }

    #[test]
    fn a_notification_waits_for_the_filesystem_to_go_quiet() {
        let (events, event_receiver) = channel::<u8>();
        let (notifications, notification_receiver) = channel();

        thread::spawn(move || debounce(event_receiver, notifications));

        let started = Instant::now();
        events.send(1).expect("send");
        notification_receiver
            .recv_timeout(QUIET_PERIOD * 8)
            .expect("a notification arrives");

        assert!(
            started.elapsed() >= QUIET_PERIOD,
            "reported after {:?}, before the quiet period elapsed",
            started.elapsed()
        );
    }

    #[test]
    fn separate_actions_are_reported_separately() {
        let (events, event_receiver) = channel::<u8>();
        let (notifications, notification_receiver) = channel();

        thread::spawn(move || debounce(event_receiver, notifications));

        for _ in 0..2 {
            events.send(1).expect("send");
            notification_receiver
                .recv_timeout(QUIET_PERIOD * 8)
                .expect("a notification per settled burst");
        }
    }

    #[test]
    fn a_real_change_under_the_git_directory_is_reported() {
        // End to end through inotify, not just the debouncer: this is the
        // path that actually has to work.
        let dir = tempfile::tempdir().expect("temp dir");
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).expect("create .git");

        let watcher = RepositoryWatcher::watch(dir.path()).expect("watch");
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if watcher.take_change() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("a write under .git produced no notification");
    }

    #[test]
    fn a_quiet_repository_reports_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join(".git")).expect("create .git");

        let watcher = RepositoryWatcher::watch(dir.path()).expect("watch");
        thread::sleep(QUIET_PERIOD * 3);

        assert!(!watcher.take_change(), "nothing happened, nothing to report");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_cannot_be_watched() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(git_dir_of(dir.path()).is_none());
        assert!(RepositoryWatcher::watch(dir.path()).is_err());
    }

    #[test]
    fn a_checkout_with_a_git_directory_can_be_watched() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join(".git")).expect("create .git");

        assert_eq!(git_dir_of(dir.path()), Some(dir.path().join(".git")));
        assert!(RepositoryWatcher::watch(dir.path()).is_ok());
    }
}
