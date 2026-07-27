//! Presentation layer.
//!
//! Renders an immutable [`crate::domain::RepositorySnapshot`] and its
//! [`crate::domain::GraphLayout`]. It never reads Git itself: everything
//! arrives from a worker thread through [`loader`].

pub mod app;
pub mod detail;
pub mod graph_view;
pub mod loader;
pub mod sidebar;
pub mod theme;

pub use app::GitGuiApp;
