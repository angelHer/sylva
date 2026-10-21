//! Opening a terminal in a worktree's directory.
//!
//! Behind a port, the same way `WorktreeOperations` is: `application` decides
//! *whether* a terminal may be opened for a given worktree, never *how* one is
//! found or started. That knowledge belongs to `infrastructure`, which is the
//! only layer allowed to name an emulator or a multiplexer.

use std::path::PathBuf;

use crate::domain::RepositorySnapshot;

/// Where to open a terminal, and what to call the session there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRequest {
    pub cwd: PathBuf,
    pub session: String,
}

/// A fully resolved command, ready to be spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("no worktree named {0}")]
    UnknownWorktree(String),
    #[error("{0} is gone; prune it before opening a terminal there")]
    WorktreeGone(PathBuf),
    #[error("no terminal emulator found; set TERMINAL to the one you use")]
    NoTerminal,
    #[error("{program}: {message}")]
    Spawn { program: String, message: String },
}

/// Turns a request into a runnable command, and runs it.
///
/// Two steps rather than one because the UI needs to hand the resolved
/// `Launch` to a background task before starting it — resolving is cheap
/// and can happen inline, starting is the part that must not block a frame.
pub trait TerminalLauncher: Send + Sync {
    fn resolve(&self, request: &TerminalRequest) -> Result<Launch, TerminalError>;

    fn start(&self, launch: &Launch) -> Result<(), TerminalError>;
}

pub struct OpenTerminal<'a> {
    launcher: &'a dyn TerminalLauncher,
}

impl<'a> OpenTerminal<'a> {
    pub fn new(launcher: &'a dyn TerminalLauncher) -> Self {
        Self { launcher }
    }

    /// Finds the worktree by directory name and asks the launcher to resolve
    /// a command for it.
    ///
    /// The guards run before the launcher is ever reached: a name that
    /// matches nothing, or a worktree Git already considers gone, is refused
    /// here rather than handed down to be discovered by a failed `stat`.
    pub fn execute(
        &self,
        snapshot: &RepositorySnapshot,
        dir_name: &str,
    ) -> Result<Launch, TerminalError> {
        let worktree = snapshot
            .worktrees()
            .iter()
            .find(|worktree| worktree.dir_name() == dir_name)
            .ok_or_else(|| TerminalError::UnknownWorktree(dir_name.to_string()))?;

        if worktree.is_prunable {
            return Err(TerminalError::WorktreeGone(worktree.path.clone()));
        }

        let request = TerminalRequest {
            cwd: worktree.path.clone(),
            session: worktree.dir_name().to_string(),
        };

        self.launcher.resolve(&request)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::domain::{Oid, RepositorySnapshot, Worktree, WorktreeHead, WorktreeStatus};

    #[derive(Default)]
    struct FakeLauncher {
        resolved: Mutex<Vec<TerminalRequest>>,
    }

    impl TerminalLauncher for FakeLauncher {
        fn resolve(&self, request: &TerminalRequest) -> Result<Launch, TerminalError> {
            self.resolved.lock().unwrap().push(request.clone());
            Ok(Launch {
                program: PathBuf::from("/usr/bin/tmux"),
                args: vec![
                    "new-session".into(),
                    "-A".into(),
                    "-s".into(),
                    request.session.clone(),
                ],
                cwd: request.cwd.clone(),
            })
        }

        fn start(&self, _launch: &Launch) -> Result<(), TerminalError> {
            Ok(())
        }
    }

    fn worktree(dir: &str, primary: bool, prunable: bool) -> Worktree {
        Worktree {
            name: dir.into(),
            path: PathBuf::from(format!("/home/dev/{dir}")),
            is_primary: primary,
            head: WorktreeHead::Branch {
                name: "main".into(),
                target: Oid::zero(),
            },
            status: WorktreeStatus::default(),
            divergence: None,
            is_locked: false,
            is_prunable: prunable,
        }
    }

    fn snapshot(worktrees: Vec<Worktree>) -> RepositorySnapshot {
        RepositorySnapshot::new(
            PathBuf::from("/home/dev/project"),
            vec![],
            vec![],
            worktrees,
            false,
        )
    }

    #[test]
    fn a_prunable_worktree_is_refused_without_reaching_the_launcher() {
        let launcher = FakeLauncher::default();
        let snap = snapshot(vec![worktree("gone", false, true)]);

        let error = OpenTerminal::new(&launcher)
            .execute(&snap, "gone")
            .unwrap_err();

        assert!(matches!(error, TerminalError::WorktreeGone(_)));
        assert!(
            launcher.resolved.lock().unwrap().is_empty(),
            "the launcher was never asked"
        );
    }

    #[test]
    fn an_unknown_worktree_name_is_refused() {
        let launcher = FakeLauncher::default();
        let snap = snapshot(vec![worktree("project", true, false)]);

        let error = OpenTerminal::new(&launcher)
            .execute(&snap, "ghost")
            .unwrap_err();

        assert!(matches!(error, TerminalError::UnknownWorktree(name) if name == "ghost"));
        assert!(launcher.resolved.lock().unwrap().is_empty());
    }

    #[test]
    fn the_request_carries_the_worktree_path_and_dir_name() {
        let launcher = FakeLauncher::default();
        let snap = snapshot(vec![worktree("feature-graph", false, false)]);

        OpenTerminal::new(&launcher)
            .execute(&snap, "feature-graph")
            .expect("resolved");

        let requests = launcher.resolved.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].cwd, PathBuf::from("/home/dev/feature-graph"));
        assert_eq!(requests[0].session, "feature-graph");
    }

    #[test]
    fn the_primary_worktree_is_allowed() {
        let launcher = FakeLauncher::default();
        let snap = snapshot(vec![worktree("project", true, false)]);

        let launch = OpenTerminal::new(&launcher)
            .execute(&snap, "project")
            .expect("the primary worktree gets a terminal too");

        assert_eq!(launch.cwd, PathBuf::from("/home/dev/project"));
    }
}
