//! Adapters that implement the application ports against the outside world.
//!
//! Swapping libgit2 for another backend means adding a sibling module here and
//! changing nothing else.

pub mod git2_backend;

pub use git2_backend::Git2Backend;
