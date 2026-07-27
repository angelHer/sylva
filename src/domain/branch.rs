use super::oid::Oid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchKind {
    Local,
    Remote,
}

/// How far a branch has diverged from its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Divergence {
    pub ahead: usize,
    pub behind: usize,
}

impl Divergence {
    pub const fn new(ahead: usize, behind: usize) -> Self {
        Self { ahead, behind }
    }

    pub const fn is_in_sync(&self) -> bool {
        self.ahead == 0 && self.behind == 0
    }

    /// True when the histories have both moved: a plain fast-forward is no
    /// longer possible and the UI should warn before pulling.
    pub const fn has_diverged(&self) -> bool {
        self.ahead > 0 && self.behind > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Short name as the user reads it: `main`, `origin/main`.
    pub name: String,
    pub kind: BranchKind,
    pub target: Oid,
    /// Short name of the tracked branch, when one is configured.
    pub upstream: Option<String>,
    pub divergence: Option<Divergence>,
    /// True when the *primary* worktree has this branch checked out.
    pub is_head: bool,
}

impl Branch {
    pub fn is_local(&self) -> bool {
        self.kind == BranchKind::Local
    }

    pub fn is_remote(&self) -> bool {
        self.kind == BranchKind::Remote
    }

    /// The remote part of a remote branch name: `origin/feature/x` -> `origin`.
    pub fn remote_name(&self) -> Option<&str> {
        if self.is_remote() {
            self.name.split_once('/').map(|(remote, _)| remote)
        } else {
            None
        }
    }

    /// The branch name without its remote prefix, for display next to a local
    /// branch of the same name.
    pub fn short_name(&self) -> &str {
        match self.kind {
            BranchKind::Local => &self.name,
            BranchKind::Remote => self
                .name
                .split_once('/')
                .map(|(_, rest)| rest)
                .unwrap_or(&self.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, kind: BranchKind) -> Branch {
        Branch {
            name: name.into(),
            kind,
            target: Oid::zero(),
            upstream: None,
            divergence: None,
            is_head: false,
        }
    }

    #[test]
    fn a_local_branch_has_no_remote_and_keeps_its_full_name() {
        let b = branch("feature/graph", BranchKind::Local);
        assert!(b.is_local());
        assert_eq!(b.remote_name(), None);
        assert_eq!(b.short_name(), "feature/graph");
    }

    #[test]
    fn a_remote_branch_splits_into_remote_and_short_name() {
        let b = branch("origin/feature/graph", BranchKind::Remote);
        assert!(b.is_remote());
        assert_eq!(b.remote_name(), Some("origin"));
        assert_eq!(b.short_name(), "feature/graph");
    }

    #[test]
    fn a_remote_branch_without_a_slash_falls_back_to_its_full_name() {
        let b = branch("weird", BranchKind::Remote);
        assert_eq!(b.remote_name(), None);
        assert_eq!(b.short_name(), "weird");
    }

    #[test]
    fn a_branch_level_with_its_upstream_is_in_sync() {
        let d = Divergence::default();
        assert!(d.is_in_sync());
        assert!(!d.has_diverged());
    }

    #[test]
    fn being_only_ahead_is_not_divergence() {
        let d = Divergence::new(3, 0);
        assert!(!d.is_in_sync());
        assert!(!d.has_diverged());
    }

    #[test]
    fn being_ahead_and_behind_at_once_is_divergence() {
        let d = Divergence::new(2, 5);
        assert!(d.has_diverged());
    }
}
