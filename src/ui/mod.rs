//! Presentation layer.
//!
//! Renders an immutable [`crate::domain::RepositorySnapshot`] and its
//! [`crate::domain::GraphLayout`]. It never reads Git itself: everything slow
//! arrives from a worker thread through [`background`].

pub mod app;
pub mod background;
pub mod detail;
pub mod graph_view;
pub mod loader;
pub mod sidebar;
pub mod theme;
pub mod welcome;
pub mod worktree_form;

pub use app::SylvaApp;
