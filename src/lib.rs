//! gitgui — a fast, native Git client built around worktree visualization.
//!
//! Layering (dependencies point inwards only):
//!
//! ```text
//! ui  ->  application  ->  domain
//!             ^
//!     infrastructure
//! ```
//!
//! `domain` depends on nothing. `application` defines the ports. `infrastructure`
//! implements them. The UI layer, added in a later phase, only ever renders an
//! immutable `RepositorySnapshot`.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::{GitError, HistoryQuery, LoadRepository};
pub use domain::RepositorySnapshot;
pub use infrastructure::Git2Backend;
