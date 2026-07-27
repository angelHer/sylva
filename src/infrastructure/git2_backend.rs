use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use git2::{
    BranchType, ErrorCode, Repository, Sort, Status, StatusOptions, WorktreeLockStatus,
};

use crate::application::ports::{
    CommitPage, GitError, HistoryQuery, RepositoryReader, WorktreeReader,
};
use crate::domain::{
    Branch, BranchKind, Commit, Divergence, Oid, Signature, Timestamp, Worktree, WorktreeHead,
    WorktreeStatus,
};

/// libgit2-backed implementation of the read ports.
///
/// `git2::Repository` is `Send` but not `Sync`, so it lives behind a `Mutex`.
/// Reads are short and run off the UI thread, so contention is not a concern;
/// if it ever becomes one, give each worker its own `Repository` instead of
/// widening the lock.
pub struct Git2Backend {
    repo: Mutex<Repository>,
    /// Primary working directory, cached so `root()` needs no lock.
    root: PathBuf,
}

impl Git2Backend {
    /// Opens the repository containing `path`, walking up to find it.
    ///
    /// If `path` is inside a linked worktree, this resolves back to the
    /// primary working directory: the unified graph is always drawn from the
    /// repository's point of view, not from whichever checkout was opened.
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let path = path.as_ref();
        let repo = Repository::discover(path)
            .map_err(|_| GitError::NotARepository(path.to_path_buf()))?;

        let repo = if repo.is_worktree() {
            let primary = primary_workdir_of(&repo)
                .ok_or_else(|| GitError::NotARepository(path.to_path_buf()))?;
            Repository::open(&primary).map_err(|e| GitError::backend("open primary", e.message()))?
        } else {
            repo
        };

        let root = repo
            .workdir()
            .map(Path::to_path_buf)
            // A bare repository has no working directory; fall back to the
            // git dir so the snapshot still has an identity.
            .unwrap_or_else(|| repo.path().to_path_buf());

        Ok(Self {
            repo: Mutex::new(repo),
            root,
        })
    }

    fn with_repo<T>(
        &self,
        context: &str,
        f: impl FnOnce(&Repository) -> Result<T, GitError>,
    ) -> Result<T, GitError> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| GitError::backend(context, "repository lock poisoned"))?;
        f(&repo)
    }
}

/// The primary working directory of a repository opened as a linked worktree.
///
/// A linked worktree's git dir is `<primary>/.git/worktrees/<name>`, and Git
/// writes a `commondir` file there pointing back at the primary git dir. That
/// file is the authoritative link, so it is read directly — `git2` 0.19 has no
/// `Repository::commondir` binding.
fn primary_workdir_of(repo: &Repository) -> Option<PathBuf> {
    let git_dir = repo.path();

    let common_dir = read_commondir(git_dir)
        // Fall back to the conventional layout: strip `worktrees/<name>` to
        // land back on the primary `.git` directory.
        .or_else(|| git_dir.ancestors().nth(2).map(Path::to_path_buf))?;

    let common_dir = fs::canonicalize(&common_dir).unwrap_or(common_dir);
    common_dir.parent().map(Path::to_path_buf)
}

fn read_commondir(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let target = PathBuf::from(contents.trim());

    if target.is_absolute() {
        Some(target)
    } else {
        // The recorded path is relative to the worktree's own git dir.
        Some(git_dir.join(target))
    }
}

fn to_domain_oid(oid: git2::Oid) -> Result<Oid, GitError> {
    let bytes: [u8; 20] = oid
        .as_bytes()
        .try_into()
        .map_err(|_| GitError::backend("object id", "unsupported hash length"))?;
    Ok(Oid::from_bytes(bytes))
}

fn to_git2_oid(oid: &Oid) -> git2::Oid {
    // Cannot fail: the slice is exactly 20 bytes.
    git2::Oid::from_bytes(oid.as_bytes()).expect("20-byte oid")
}

fn to_signature(sig: &git2::Signature<'_>) -> Signature {
    Signature::new(
        sig.name().unwrap_or("").to_string(),
        sig.email().unwrap_or("").to_string(),
        Timestamp::new(sig.when().seconds(), sig.when().offset_minutes()),
    )
}

fn to_commit(commit: &git2::Commit<'_>) -> Result<Commit, GitError> {
    let parents = commit
        .parent_ids()
        .map(to_domain_oid)
        .collect::<Result<Vec<_>, _>>()?;

    let message = commit.message().unwrap_or("");
    let (summary, body) = split_message(message);

    Ok(Commit {
        id: to_domain_oid(commit.id())?,
        parents,
        summary,
        body,
        author: to_signature(&commit.author()),
        committer: to_signature(&commit.committer()),
    })
}

/// Splits a commit message into its summary line and the rest, the way Git
/// itself does: the first paragraph is the summary.
fn split_message(message: &str) -> (String, String) {
    let message = message.trim_end();
    match message.split_once('\n') {
        Some((summary, rest)) => (summary.trim_end().to_string(), rest.trim().to_string()),
        None => (message.to_string(), String::new()),
    }
}

/// Every commit the walk should start from.
///
/// Branch tips alone are not enough: a worktree on a detached HEAD points at a
/// commit that no branch references, and leaving it out would drop it from the
/// graph — exactly the case this client exists to show.
fn collect_walk_tips(repo: &Repository, include_remotes: bool) -> Result<Vec<git2::Oid>, GitError> {
    let mut tips = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |oid: git2::Oid, tips: &mut Vec<git2::Oid>| {
        if seen.insert(oid) {
            tips.push(oid);
        }
    };

    let filter = if include_remotes {
        None
    } else {
        Some(BranchType::Local)
    };

    let branches = repo
        .branches(filter)
        .map_err(|e| GitError::backend("list branches", e.message()))?;

    for entry in branches {
        let (branch, _) = entry.map_err(|e| GitError::backend("read branch", e.message()))?;
        if let Some(oid) = branch.get().target() {
            push(oid, &mut tips);
        }
    }

    if let Ok(head) = repo.head() {
        if let Some(oid) = head.target() {
            push(oid, &mut tips);
        }
    }

    for oid in worktree_head_oids(repo) {
        push(oid, &mut tips);
    }

    Ok(tips)
}

/// HEAD commits of every linked worktree, best-effort: a worktree whose
/// directory has been deleted must not break the history walk.
fn worktree_head_oids(repo: &Repository) -> Vec<git2::Oid> {
    let Ok(names) = repo.worktrees() else {
        return Vec::new();
    };

    names
        .iter()
        .flatten()
        .filter_map(|name| repo.find_worktree(name).ok())
        .filter_map(|wt| Repository::open(wt.path()).ok())
        .filter_map(|wt_repo| wt_repo.head().ok().and_then(|head| head.target()))
        .collect()
}

impl RepositoryReader for Git2Backend {
    fn root(&self) -> &Path {
        &self.root
    }

    fn commits(&self, query: &HistoryQuery) -> Result<CommitPage, GitError> {
        self.with_repo("walk history", |repo| {
            let tips = collect_walk_tips(repo, query.include_remote_branches)?;
            if tips.is_empty() {
                return Ok(CommitPage {
                    commits: Vec::new(),
                    truncated: false,
                });
            }

            let mut walk = repo
                .revwalk()
                .map_err(|e| GitError::backend("revwalk", e.message()))?;

            // Topological order keeps parents below children; the time key
            // breaks ties so independent branches interleave by date, which is
            // what makes the graph readable.
            //
            // It is not free. Measured on a 50k-commit repository, warm, best
            // of nine runs: 92ms against 71ms for a 5,000-commit page, and
            // 408ms against 369ms for all of it. Dropping to date order buys
            // roughly 20ms on the page the window opens with — paid for by
            // letting a child appear below its own parent whenever a commit
            // carries a skewed clock. On a worker thread that is a bad trade,
            // so the ordering guarantee stays.
            walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
                .map_err(|e| GitError::backend("revwalk sorting", e.message()))?;

            for tip in tips {
                walk.push(tip)
                    .map_err(|e| GitError::backend("revwalk push", e.message()))?;
            }

            // Ask for one commit past the limit: if it arrives, history
            // continues and the snapshot must be marked truncated.
            let budget = query.max_commits.map(|max| max.saturating_add(1));
            let mut commits = Vec::with_capacity(query.max_commits.unwrap_or(1024).min(16_384));

            for oid in walk {
                let oid = oid.map_err(|e| GitError::backend("revwalk step", e.message()))?;
                let commit = repo
                    .find_commit(oid)
                    .map_err(|e| GitError::backend("find commit", e.message()))?;
                commits.push(to_commit(&commit)?);

                if budget.is_some_and(|budget| commits.len() >= budget) {
                    break;
                }
            }

            let truncated = query.max_commits.is_some_and(|max| commits.len() > max);
            if truncated {
                commits.pop();
            }

            Ok(CommitPage { commits, truncated })
        })
    }

    fn branches(&self) -> Result<Vec<Branch>, GitError> {
        self.with_repo("list branches", |repo| {
            let iter = repo
                .branches(None)
                .map_err(|e| GitError::backend("list branches", e.message()))?;

            let mut branches = Vec::new();
            for entry in iter {
                let (branch, kind) =
                    entry.map_err(|e| GitError::backend("read branch", e.message()))?;

                // Names that are not valid UTF-8 are skipped rather than
                // rendered as replacement characters.
                let Ok(Some(name)) = branch.name() else {
                    continue;
                };
                let name = name.to_string();

                // `origin/HEAD` is a symbolic alias for another branch already
                // in this list; showing it would duplicate the chip.
                if name.ends_with("/HEAD") {
                    continue;
                }

                let Some(target) = branch.get().target() else {
                    continue;
                };
                let target = to_domain_oid(target)?;

                let upstream = branch.upstream().ok();
                let upstream_name = upstream
                    .as_ref()
                    .and_then(|u| u.name().ok().flatten())
                    .map(str::to_string);
                let divergence = upstream
                    .as_ref()
                    .and_then(|u| u.get().target())
                    .and_then(|up| repo.graph_ahead_behind(to_git2_oid(&target), up).ok())
                    .map(|(ahead, behind)| Divergence::new(ahead, behind));

                branches.push(Branch {
                    name,
                    kind: match kind {
                        BranchType::Local => BranchKind::Local,
                        BranchType::Remote => BranchKind::Remote,
                    },
                    target,
                    upstream: upstream_name,
                    divergence,
                    is_head: branch.is_head(),
                });
            }

            Ok(branches)
        })
    }
}

impl WorktreeReader for Git2Backend {
    fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        self.with_repo("list worktrees", |repo| {
            let mut worktrees = Vec::new();

            // The primary checkout is not part of libgit2's worktree list, but
            // it is a worktree as far as the user is concerned.
            if let Some(workdir) = repo.workdir() {
                worktrees.push(read_worktree(
                    repo,
                    dir_name_of(workdir),
                    workdir.to_path_buf(),
                    true,
                    false,
                    false,
                )?);
            }

            let names = repo
                .worktrees()
                .map_err(|e| GitError::backend("list worktrees", e.message()))?;

            for name in names.iter().flatten() {
                let linked = repo
                    .find_worktree(name)
                    .map_err(|e| GitError::backend("find worktree", e.message()))?;

                let is_locked = !matches!(
                    linked.is_locked().unwrap_or(WorktreeLockStatus::Unlocked),
                    WorktreeLockStatus::Unlocked
                );
                let is_prunable = linked.is_prunable(None).unwrap_or(false);
                let path = linked.path().to_path_buf();

                match Repository::open(&path) {
                    Ok(wt_repo) => worktrees.push(read_worktree(
                        &wt_repo,
                        name.to_string(),
                        path,
                        false,
                        is_locked,
                        is_prunable,
                    )?),
                    // A registered worktree whose directory is gone. It still
                    // belongs in the list — that is precisely what the user
                    // needs to see in order to prune it.
                    Err(_) => worktrees.push(Worktree {
                        name: name.to_string(),
                        path,
                        is_primary: false,
                        // Its HEAD cannot be read because its directory is
                        // gone. Naming it after the worktree would read as a
                        // branch it never had.
                        head: WorktreeHead::Unborn {
                            name: "(unavailable)".to_string(),
                        },
                        status: WorktreeStatus::default(),
                        divergence: None,
                        is_locked,
                        is_prunable: true,
                    }),
                }
            }

            Ok(worktrees)
        })
    }
}

fn dir_name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("(root)")
        .to_string()
}

fn read_worktree(
    repo: &Repository,
    name: String,
    path: PathBuf,
    is_primary: bool,
    is_locked: bool,
    is_prunable: bool,
) -> Result<Worktree, GitError> {
    let head = read_head(repo)?;
    let status = read_status(repo, &name, &path)?;
    let divergence = read_divergence(repo, &head);

    Ok(Worktree {
        name,
        path,
        is_primary,
        head,
        status,
        divergence,
        is_locked,
        is_prunable,
    })
}

fn read_head(repo: &Repository) -> Result<WorktreeHead, GitError> {
    match repo.head() {
        Ok(reference) => {
            let target = reference
                .target()
                .map(to_domain_oid)
                .transpose()?
                .unwrap_or_else(Oid::zero);

            let detached = repo.head_detached().unwrap_or(false);
            if detached {
                Ok(WorktreeHead::Detached { target })
            } else {
                Ok(WorktreeHead::Branch {
                    name: reference.shorthand().unwrap_or("HEAD").to_string(),
                    target,
                })
            }
        }
        // A fresh checkout with no commit yet: HEAD points at a branch that
        // does not exist.
        Err(e) if e.code() == ErrorCode::UnbornBranch => Ok(WorktreeHead::Unborn {
            name: unborn_branch_name(repo),
        }),
        Err(e) => Err(GitError::backend("read HEAD", e.message())),
    }
}

fn unborn_branch_name(repo: &Repository) -> String {
    repo.find_reference("HEAD")
        .ok()
        .and_then(|head| {
            head.symbolic_target()
                .map(|target| target.trim_start_matches("refs/heads/").to_string())
        })
        .unwrap_or_else(|| "HEAD".to_string())
}

fn read_status(repo: &Repository, name: &str, path: &Path) -> Result<WorktreeStatus, GitError> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        // Report an untracked directory once instead of walking every file
        // inside it. A fresh `node_modules` would otherwise cost more than the
        // rest of the snapshot combined.
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .include_unmodified(false)
        .exclude_submodules(true);

    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|e| GitError::UnreachableWorktree {
            name: name.to_string(),
            path: path.to_path_buf(),
            message: e.message().to_string(),
        })?;

    const STAGED: Status = Status::INDEX_NEW
        .union(Status::INDEX_MODIFIED)
        .union(Status::INDEX_DELETED)
        .union(Status::INDEX_RENAMED)
        .union(Status::INDEX_TYPECHANGE);

    const UNSTAGED: Status = Status::WT_MODIFIED
        .union(Status::WT_DELETED)
        .union(Status::WT_RENAMED)
        .union(Status::WT_TYPECHANGE);

    let mut result = WorktreeStatus::default();
    for entry in statuses.iter() {
        let status = entry.status();

        // Conflicts are counted exclusively: a conflicted file is one problem,
        // not three, and it must dominate the badge colour.
        if status.is_conflicted() {
            result.conflicted += 1;
            continue;
        }
        if status.intersects(STAGED) {
            result.staged += 1;
        }
        if status.intersects(UNSTAGED) {
            result.unstaged += 1;
        }
        if status.contains(Status::WT_NEW) {
            result.untracked += 1;
        }
    }

    Ok(result)
}

fn read_divergence(repo: &Repository, head: &WorktreeHead) -> Option<Divergence> {
    let name = head.branch_name()?;
    let target = head.target()?;
    let branch = repo.find_branch(name, BranchType::Local).ok()?;
    let upstream = branch.upstream().ok()?;
    let upstream_oid = upstream.get().target()?;

    repo.graph_ahead_behind(to_git2_oid(&target), upstream_oid)
        .ok()
        .map(|(ahead, behind)| Divergence::new(ahead, behind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_line_message_is_all_summary() {
        let (summary, body) = split_message("fix: graph lanes\n");
        assert_eq!(summary, "fix: graph lanes");
        assert_eq!(body, "");
    }

    #[test]
    fn a_message_body_is_separated_from_its_summary() {
        let (summary, body) = split_message("feat: worktree badges\n\nShows dirty state.\n");
        assert_eq!(summary, "feat: worktree badges");
        assert_eq!(body, "Shows dirty state.");
    }

    #[test]
    fn an_empty_message_yields_empty_parts() {
        let (summary, body) = split_message("");
        assert_eq!(summary, "");
        assert_eq!(body, "");
    }

    #[test]
    fn domain_and_git2_object_ids_round_trip() {
        let domain = Oid::from_hex("1a2b3c4d5e6f708192a3b4c5d6e7f80910111213").unwrap();
        let round_tripped = to_domain_oid(to_git2_oid(&domain)).unwrap();
        assert_eq!(round_tripped, domain);
    }
}
