//! Pure domain model.
//!
//! Nothing here knows that libgit2, the filesystem, or a GPU exist. Everything
//! in this module must stay compilable with zero external crates so the rules
//! of the product can be tested without a repository on disk.

pub mod branch;
pub mod commit;
pub mod oid;
pub mod snapshot;
pub mod worktree;

pub use branch::{Branch, BranchKind, Divergence};
pub use commit::{Commit, Signature, Timestamp};
pub use oid::{Oid, OidParseError};
pub use snapshot::RepositorySnapshot;
pub use worktree::{Worktree, WorktreeHead, WorktreeStatus};
