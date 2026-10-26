//! Explorer — the workspace project panel and its file viewer.
//!
//! See `docs/explorer.md` for the full design. Layered so no I/O touches a
//! frame:
//!
//! - [`walk`] — background snapshot of the workspace, gitignore-aware.
//! - [`tree`] — pure in-memory tree, expand/collapse, filter, git badges.
//! - [`ops`] — pure filesystem mutations (create/rename) + name validation.
//! - [`panel`] — the project panel entity (right dock, virtualized tree).
//! - [`viewer`] — background file loading with binary/size guards + the
//!   full-page file view entity.

pub mod ops;
pub mod panel;
pub mod tree;
pub mod viewer;
pub mod walk;

pub use panel::{ExplorerResize, FileOpRequest, ProjectPanel};
pub use viewer::FileViewer;
