//! Adapters that implement the application ports against the outside world.
//!
//! Swapping libgit2 for another backend means adding a sibling module here and
//! changing nothing else.

pub mod git2_backend;
pub mod git_cli;
pub mod recent;
pub mod terminal;
pub mod watcher;

pub use git2_backend::Git2Backend;
pub use git_cli::GitCli;
pub use recent::RecentRepositories;
pub use terminal::SystemTerminal;
pub use watcher::RepositoryWatcher;
