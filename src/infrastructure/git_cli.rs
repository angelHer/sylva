//! Worktree operations, run through the `git` command.
//!
//! Deliberately not libgit2. `git worktree` does bookkeeping libgit2 leaves to
//! the caller — writing the `gitdir` link, registering the administrative
//! directory, refusing a branch that is checked out elsewhere — and getting
//! that wrong leaves a repository in a state the user has to repair by hand.
//! Shelling out to the tool that owns the format is the conservative choice.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::application::ports::GitError;
use crate::application::worktree_ops::{
    AddWorktreeRequest, ChangeKind, UncommittedEntry, WorktreeOperations,
};

pub struct GitCli {
    root: PathBuf,
}

impl GitCli {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn run<I, S>(&self, context: &str, args: I) -> Result<GitOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            // Never block waiting for a credential or an editor: this runs on
            // a worker thread with nobody watching a terminal.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|error| GitError::backend(context, format!("could not run git: {error}")))?;

        if !output.status.success() {
            // Git explains itself well; pass its own words through rather than
            // inventing a message.
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if message.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                message
            };
            return Err(GitError::backend(context, message));
        }

        Ok(GitOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Both streams of a successful `git` run.
///
/// Both are kept because git does not put its progress reporting where you
/// would expect: `worktree prune --verbose` names what it removed on *stderr*,
/// with stdout left empty.
struct GitOutput {
    #[allow(dead_code)]
    stdout: String,
    stderr: String,
}

impl WorktreeOperations for GitCli {
    fn add(&self, request: &AddWorktreeRequest) -> Result<(), GitError> {
        let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("add")];

        // `add -b <branch> <path>` creates the branch;
        // `add <path> <branch>` checks out an existing one.
        if request.create_branch {
            args.push(OsStr::new("-b"));
            args.push(OsStr::new(request.branch.as_str()));
            args.push(request.path.as_os_str());
        } else {
            args.push(request.path.as_os_str());
            args.push(OsStr::new(request.branch.as_str()));
        }

        self.run("worktree add", args)?;
        Ok(())
    }

    fn remove(&self, path: &Path, force: bool) -> Result<(), GitError> {
        let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("remove")];
        if force {
            args.push(OsStr::new("--force"));
        }
        args.push(path.as_os_str());

        self.run("worktree remove", args)?;
        Ok(())
    }

    fn uncommitted_changes(&self, path: &Path) -> Result<Vec<UncommittedEntry>, GitError> {
        // The same question `git worktree remove` asks before refusing, so the
        // count matches its verdict: an entry here is one it would not delete.
        let output = self.run(
            "status",
            [
                OsStr::new("-C"),
                path.as_os_str(),
                OsStr::new("status"),
                OsStr::new("--porcelain"),
                // NUL-separated, so paths arrive unquoted and a space or a
                // newline in a name cannot split an entry.
                OsStr::new("-z"),
                OsStr::new("--ignore-submodules=none"),
            ],
        )?;

        Ok(parse_porcelain(&output.stdout))
    }

    fn prune(&self) -> Result<usize, GitError> {
        // `--verbose` makes git name each record it drops, one per line. It
        // writes those lines to stderr, not stdout, and they are translated
        // into the user's locale — so they are counted, never parsed.
        let output = self.run(
            "worktree prune",
            [OsStr::new("worktree"), OsStr::new("prune"), OsStr::new("--verbose")],
        )?;

        Ok(output
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    fn path_is_free(&self, path: &Path) -> bool {
        // `try_exists` distinguishes "not there" from "cannot tell"; anything
        // it cannot answer is treated as taken, so a doubtful path is never
        // written to.
        !path.try_exists().unwrap_or(true)
    }
}

/// Reads `git status --porcelain -z`: one `XY path` field per entry, where a
/// rename or copy is followed by a second field holding its old path.
fn parse_porcelain(output: &str) -> Vec<UncommittedEntry> {
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    let mut entries = Vec::new();

    while let Some(field) = fields.next() {
        let mut code = field.chars();
        let (Some(index), Some(worktree), Some(path)) = (code.next(), code.next(), field.get(3..))
        else {
            continue;
        };

        let change = match (index, worktree) {
            ('?', '?') => ChangeKind::New,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => ChangeKind::Conflicted,
            ('R' | 'C', _) => {
                fields.next();
                ChangeKind::Renamed
            }
            ('D', _) | (_, 'D') => ChangeKind::Deleted,
            (_, 'M' | 'T') => ChangeKind::Modified,
            _ => ChangeKind::Staged,
        };

        entries.push(UncommittedEntry {
            path: path.to_string(),
            change,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, change: ChangeKind) -> UncommittedEntry {
        UncommittedEntry {
            path: path.to_string(),
            change,
        }
    }

    #[test]
    fn porcelain_entries_are_read_in_the_words_the_sidebar_uses() {
        let output = "?? notes.txt\0 M src/main.rs\0M  staged.rs\0 D gone.rs\0UU clash.rs\0";

        assert_eq!(
            parse_porcelain(output),
            vec![
                entry("notes.txt", ChangeKind::New),
                entry("src/main.rs", ChangeKind::Modified),
                entry("staged.rs", ChangeKind::Staged),
                entry("gone.rs", ChangeKind::Deleted),
                entry("clash.rs", ChangeKind::Conflicted),
            ]
        );
    }

    #[test]
    fn a_rename_is_one_entry_under_its_new_name() {
        // With -z a rename carries its old path as a second, separate field.
        let output = "R  new name.rs\0ab\0?? after.txt\0";

        assert_eq!(
            parse_porcelain(output),
            vec![
                entry("new name.rs", ChangeKind::Renamed),
                entry("after.txt", ChangeKind::New),
            ]
        );
    }

    #[test]
    fn a_path_that_does_not_exist_is_free() {
        let cli = GitCli::at("/");
        assert!(cli.path_is_free(Path::new("/definitely/not/here/at/all")));
    }

    #[test]
    fn an_existing_path_is_not_free() {
        let cli = GitCli::at("/");
        assert!(!cli.path_is_free(Path::new("/")));
    }

    #[test]
    fn a_failing_command_carries_gits_own_message() {
        // `git -C <nonexistent>` fails, and the error should say so rather
        // than reporting a generic exit code.
        let cli = GitCli::at("/definitely/not/a/repository");
        let error = cli
            .run("probe", [OsStr::new("status")])
            .err()
            .expect("git must fail here");

        assert!(matches!(error, GitError::Backend { .. }));
    }
}
