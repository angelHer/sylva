//! Validating a worktree before anything touches the disk.
//!
//! Creating a worktree writes a directory and a ref. Both fail in ugly,
//! half-finished ways when handed a bad name, so the name is checked here —
//! away from the UI, away from the process that would run `git`, and under
//! test.

use std::fmt;
use std::path::{Path, PathBuf};

/// A Git branch name that has passed `git check-ref-format`'s rules.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchNameError {
    Empty,
    /// Contains a character Git forbids in a ref.
    ForbiddenCharacter(char),
    /// `..`, a leading `-`, a trailing `.lock`, an empty path component, and
    /// the other shapes `git check-ref-format` rejects.
    Malformed(&'static str),
}

impl fmt::Display for BranchNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a branch name cannot be empty"),
            Self::ForbiddenCharacter(c) => {
                write!(f, "a branch name cannot contain {c:?}")
            }
            Self::Malformed(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for BranchNameError {}

/// Characters Git refuses outright in a ref name.
const FORBIDDEN: [char; 7] = ['~', '^', ':', '?', '*', '[', '\\'];

impl BranchName {
    pub fn parse(name: &str) -> Result<Self, BranchNameError> {
        let name = name.trim();

        if name.is_empty() {
            return Err(BranchNameError::Empty);
        }

        for character in name.chars() {
            if FORBIDDEN.contains(&character) {
                return Err(BranchNameError::ForbiddenCharacter(character));
            }
            // Control characters and spaces are rejected together: both break
            // ref files, and a space is the mistake people actually make.
            if character.is_control() || character == ' ' {
                return Err(BranchNameError::ForbiddenCharacter(character));
            }
        }

        if name.starts_with('-') {
            return Err(BranchNameError::Malformed(
                "a branch name cannot start with '-'; git would read it as an option",
            ));
        }
        if name.contains("..") {
            return Err(BranchNameError::Malformed("a branch name cannot contain '..'"));
        }
        if name.contains("@{") {
            return Err(BranchNameError::Malformed("a branch name cannot contain '@{'"));
        }
        if name == "@" {
            return Err(BranchNameError::Malformed("'@' alone is not a branch name"));
        }
        if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
            return Err(BranchNameError::Malformed(
                "a branch name cannot have an empty path component",
            ));
        }
        if name.ends_with('.') {
            return Err(BranchNameError::Malformed("a branch name cannot end with '.'"));
        }

        for component in name.split('/') {
            if component.starts_with('.') {
                return Err(BranchNameError::Malformed(
                    "no part of a branch name may start with '.'",
                ));
            }
            if component.ends_with(".lock") {
                return Err(BranchNameError::Malformed(
                    "no part of a branch name may end with '.lock'",
                ));
            }
        }

        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A filesystem-safe form of the name, for use as a directory.
    ///
    /// `feature/graph-lanes` becomes `feature-graph-lanes`: nesting a worktree
    /// directory per path component would bury checkouts inside each other.
    pub fn to_directory_name(&self) -> String {
        self.0.replace('/', "-")
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a new worktree should live by default.
///
/// A sibling directory next to the repository, never inside it: a worktree
/// nested in its own repository shows up as untracked junk in every status,
/// and `git worktree add` is happy to let that happen.
///
/// `/home/dev/project` + `feature/graph` becomes
/// `/home/dev/project-worktrees/feature-graph`.
pub fn default_worktree_path(repository_root: &Path, branch: &BranchName) -> PathBuf {
    let directory = branch.to_directory_name();

    let name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repository");

    let parent = repository_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    parent.join(format!("{name}-worktrees")).join(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_accepted() {
        assert_eq!(BranchName::parse("main").unwrap().as_str(), "main");
    }

    #[test]
    fn a_namespaced_name_is_accepted() {
        assert_eq!(
            BranchName::parse("feature/graph-lanes").unwrap().as_str(),
            "feature/graph-lanes"
        );
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_rather_than_rejected() {
        assert_eq!(BranchName::parse("  main\n").unwrap().as_str(), "main");
    }

    #[test]
    fn an_empty_or_blank_name_is_rejected() {
        assert_eq!(BranchName::parse(""), Err(BranchNameError::Empty));
        assert_eq!(BranchName::parse("   "), Err(BranchNameError::Empty));
    }

    #[test]
    fn a_name_with_a_space_inside_is_rejected() {
        assert_eq!(
            BranchName::parse("my branch"),
            Err(BranchNameError::ForbiddenCharacter(' '))
        );
    }

    #[test]
    fn every_character_git_forbids_is_rejected() {
        for forbidden in FORBIDDEN {
            let name = format!("feat{forbidden}x");
            assert_eq!(
                BranchName::parse(&name),
                Err(BranchNameError::ForbiddenCharacter(forbidden)),
                "expected {forbidden:?} to be rejected"
            );
        }
    }

    #[test]
    fn a_name_starting_with_a_dash_is_rejected_because_git_would_read_an_option() {
        assert!(matches!(
            BranchName::parse("-force"),
            Err(BranchNameError::Malformed(_))
        ));
    }

    #[test]
    fn the_shapes_git_check_ref_format_rejects_are_rejected() {
        for bad in [
            "feat..ure",
            "feat@{1}",
            "@",
            "/leading",
            "trailing/",
            "double//slash",
            "trailing.",
            ".hidden",
            "feature/.hidden",
            "feature.lock",
            "feature/thing.lock",
        ] {
            assert!(
                matches!(BranchName::parse(bad), Err(BranchNameError::Malformed(_))),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn a_name_containing_a_dot_that_is_not_at_a_boundary_is_fine() {
        assert!(BranchName::parse("release/1.2.3").is_ok());
    }

    #[test]
    fn slashes_become_dashes_in_a_directory_name() {
        let branch = BranchName::parse("feature/graph/lanes").unwrap();
        assert_eq!(branch.to_directory_name(), "feature-graph-lanes");
    }

    #[test]
    fn the_default_path_is_a_sibling_of_the_repository() {
        let branch = BranchName::parse("feature/graph").unwrap();
        let path = default_worktree_path(Path::new("/home/dev/project"), &branch);

        assert_eq!(
            path,
            PathBuf::from("/home/dev/project-worktrees/feature-graph")
        );
    }

    #[test]
    fn the_default_path_is_never_inside_the_repository() {
        // A worktree nested in its own repository turns up as untracked files
        // in every status it reports.
        let root = Path::new("/home/dev/project");
        let branch = BranchName::parse("feature/graph").unwrap();

        assert!(!default_worktree_path(root, &branch).starts_with(root));
    }

    #[test]
    fn a_repository_at_the_filesystem_root_still_gets_a_usable_path() {
        let branch = BranchName::parse("main").unwrap();
        let path = default_worktree_path(Path::new("/project"), &branch);

        assert_eq!(path, PathBuf::from("/project-worktrees/main"));
    }
}
