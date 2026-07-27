//! Use cases and the ports they depend on.
//!
//! This layer orchestrates the domain and declares the traits that
//! `infrastructure` implements. It never names a concrete Git backend.

pub mod load_repository;
pub mod ports;

pub use load_repository::LoadRepository;
pub use ports::{CommitPage, GitError, HistoryQuery, RepositoryReader, WorktreeReader};
