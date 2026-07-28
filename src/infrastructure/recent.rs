//! The list of recently opened repositories.
//!
//! Started from a terminal, the working directory says which repository you
//! mean. Started from a desktop launcher it says nothing — the working
//! directory is your home. This list is what the welcome screen offers instead.
//!
//! It is deliberately a plain text file, one absolute path per line, newest
//! first: it has to be readable and fixable with an editor, and a corrupt line
//! must cost one entry rather than the whole list.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the list is kept, relative to the configuration directory.
const FILE_NAME: &str = "recent";

pub struct RecentRepositories {
    file: PathBuf,
    /// Newest first.
    entries: Vec<PathBuf>,
}

impl RecentRepositories {
    /// How many repositories to remember. Long enough to cover what you are
    /// working on, short enough to stay a list rather than a history.
    pub const LIMIT: usize = 10;

    /// Reads the list from the user's configuration directory.
    pub fn load() -> Self {
        Self::at(default_file())
    }

    /// Reads the list from a specific file. The seam the tests use, and what
    /// [`load`](Self::load) is built on.
    ///
    /// Entries that are no longer repositories are dropped as they are read: a
    /// directory you deleted should not sit in the list forever, and a stale
    /// path that fails on click is worse than one that quietly disappears.
    pub fn at(file: PathBuf) -> Self {
        let entries = fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .filter(|path| is_repository(path))
            .take(Self::LIMIT)
            .collect();

        Self { file, entries }
    }

    /// The remembered repositories, newest first.
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Puts a repository at the front, whether or not it was already there.
    ///
    /// The path is made absolute first, so the same repository reached as `.`
    /// and by its full path is one entry rather than two.
    pub fn record(&mut self, path: &Path) {
        let path = absolute(path);

        self.entries.retain(|known| known != &path);
        self.entries.insert(0, path);
        self.entries.truncate(Self::LIMIT);
    }

    /// Drops a repository from the list.
    pub fn forget(&mut self, path: &Path) {
        let path = absolute(path);
        self.entries.retain(|known| known != &path);
    }

    /// Writes the list out, creating the configuration directory if needed.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut body = String::new();
        for entry in &self.entries {
            body.push_str(&entry.to_string_lossy());
            body.push('\n');
        }
        fs::write(&self.file, body)
    }
}

/// `$XDG_CONFIG_HOME/sylva/recent`, falling back to `~/.config/sylva/recent`.
fn default_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));

    config.join("sylva").join(FILE_NAME)
}

/// Whether a directory still holds a repository.
///
/// A primary checkout has a `.git` directory; a linked worktree has a `.git`
/// file pointing at one. Either counts — sylva exists to open worktrees.
fn is_repository(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Resolves a path against the working directory without requiring it to
/// exist, and without following symlinks: `canonicalize` would rewrite a path
/// the user recognises into one they do not.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize(path);
    }

    match std::env::current_dir() {
        Ok(cwd) => normalize(&cwd.join(path)),
        Err(_) => path.to_path_buf(),
    }
}

/// Removes `.` and `..` segments, so `/home/a/repo` and `/home/a/./repo` are
/// one entry.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
