//! Pure domain model.
//!
//! Nothing here knows that libgit2, the filesystem, or a GPU exist. Everything
//! in this module must stay compilable with zero external crates so the rules
//! of the product can be tested without a repository on disk.

pub mod ancestry;
pub mod branch;
pub mod commit;
pub mod graph;
pub mod oid;
pub mod snapshot;
pub mod worktree;
pub mod worktree_request;

pub use ancestry::Ancestry;
pub use branch::{Branch, BranchKind, Divergence};
pub use commit::{Commit, Signature, Timestamp};
pub use graph::{GraphLayout, GraphRow, Segment, LANE_COLOR_COUNT};
pub use oid::{Oid, OidParseError};
pub use snapshot::RepositorySnapshot;
pub use worktree::{Worktree, WorktreeHead, WorktreeStatus};
pub use worktree_request::{default_worktree_path, BranchName, BranchNameError};
