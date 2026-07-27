//! sylva — a fast, native Git client built around worktrees.
//!
//! Named for the Latin for woodland: many trees sharing one ground, which is
//! what a repository with worktrees is.
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
pub mod ui;

pub use application::{GitError, HistoryQuery, LoadRepository};
pub use domain::{GraphLayout, RepositorySnapshot};
pub use infrastructure::Git2Backend;
pub use ui::SylvaApp;
