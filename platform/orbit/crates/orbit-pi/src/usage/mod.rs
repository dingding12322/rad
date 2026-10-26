//! The **Usage** page: where the agent's work is measured.
//!
//! Layered deliberately, so the rendering path never touches a record:
//!
//! ```text
//! pi session store (~/.pi/agent/sessions/**.jsonl)
//!         ↓  collect::UsageScanner      — incremental, off-thread
//! collect::UsageIndex                   — normalized records + interned tables
//!         ↓  aggregate::UsageSnapshot   — one pass per (index, filter)
//! page::UsagePage                       — cached snapshot + filter/view state
//!         ↓  view / chart / filters     — GPUI, aggregates only
//!         ↓  table                      — rows dressed as native GPUI rows
//! ```
//!
//! Accounting rules (what counts as a request, how cache hit rate is defined,
//! why retries are unavailable) are documented once, in [`model`].
//!
//! The data tables and the timeline plot are drawn with GPUI's own elements —
//! `div`, `canvas` and the app's theme — so they read exactly like the rest of
//! the surface.

pub mod aggregate;
pub mod chart;
pub mod collect;
pub mod filters;
pub mod format;
pub mod heatmap;
pub mod model;
pub mod page;
pub mod table;
pub mod tooltip;
pub mod view;

#[cfg(test)]
mod tests;
