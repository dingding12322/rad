//! Persisted workbench layout — the resizable columns' widths and the
//! terminal panel's height, so a layout the user dials in survives a restart.
//!
//! Orbit-owned (`~/.orbit-pi/layout.json`), like the pins and favorites
//! stores. A panel records its live size here as it changes; the heartbeat
//! flushes once the size holds still ([`SETTLE`]), so a drag costs one write
//! rather than one per pixel. Reads happen once, when each panel is built.
//!
//! Only the four sizes the user can drag live here. Everything else about the
//! workbench (open panels, theme, fonts) has its own store.

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long a size must hold still before it is written to disk.
const SETTLE: Duration = Duration::from_millis(500);

/// Sanity bounds for a stored size, px. Guards against a corrupted file;
/// each panel still clamps to its own min/max when it applies the value.
const MIN_PX: f32 = 80.;
const MAX_PX: f32 = 4000.;

/// The four user-resizable sizes, all optional: `None` means the user has
/// never set that panel, so the panel keeps its built-in default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Layout {
    pub sidebar_width: Option<f32>,
    pub sidepane_width: Option<f32>,
    pub explorer_width: Option<f32>,
    pub terminal_height: Option<f32>,
}

impl Layout {
    /// Parse a stored payload, dropping any field that is missing or out of
    /// the sanity bounds.
    fn from_value(value: &Value) -> Self {
        Self {
            sidebar_width: field(value, "sidebar_width"),
            sidepane_width: field(value, "sidepane_width"),
            explorer_width: field(value, "explorer_width"),
            terminal_height: field(value, "terminal_height"),
        }
    }

    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(persist_path()) else {
            return Self::default();
        };
        serde_json::from_str::<Value>(&raw)
            .map(|value| Self::from_value(&value))
            .unwrap_or_default()
    }

    fn persist(self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            json!({
                "sidebar_width": self.sidebar_width,
                "sidepane_width": self.sidepane_width,
                "explorer_width": self.explorer_width,
                "terminal_height": self.terminal_height,
            })
            .to_string(),
        );
    }
}

/// One stored size, rejected when missing, non-finite, or out of bounds.
fn field(value: &Value, key: &str) -> Option<f32> {
    let px = value.get(key).and_then(Value::as_f64)? as f32;
    (px.is_finite() && (MIN_PX..=MAX_PX).contains(&px)).then_some(px)
}

struct Store {
    layout: Layout,
    /// When a size last changed; the flush waits out [`SETTLE`].
    changed_at: Option<Instant>,
}

static STORE: RwLock<Option<Store>> = RwLock::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = STORE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(|| Store {
        layout: Layout::load(),
        changed_at: None,
    }))
}

/// The stored size for each panel, if the user has ever set one.
pub fn sidebar_width() -> Option<f32> {
    with_store(|store| store.layout.sidebar_width)
}
pub fn sidepane_width() -> Option<f32> {
    with_store(|store| store.layout.sidepane_width)
}
pub fn explorer_width() -> Option<f32> {
    with_store(|store| store.layout.explorer_width)
}
pub fn terminal_height() -> Option<f32> {
    with_store(|store| store.layout.terminal_height)
}

/// Apply `set` to the layout and arm the settle timer only when the value
/// actually changed, so a stationary drag never re-arms the writer.
fn record(store: &mut Store, now: Instant, set: impl FnOnce(&mut Layout)) {
    let before = store.layout;
    set(&mut store.layout);
    if store.layout != before {
        store.changed_at = Some(now);
    }
}

pub fn set_sidebar_width(px: f32) {
    let now = Instant::now();
    with_store(|store| record(store, now, |layout| layout.sidebar_width = Some(px)));
}
pub fn set_sidepane_width(px: f32) {
    let now = Instant::now();
    with_store(|store| record(store, now, |layout| layout.sidepane_width = Some(px)));
}
pub fn set_explorer_width(px: f32) {
    let now = Instant::now();
    with_store(|store| record(store, now, |layout| layout.explorer_width = Some(px)));
}
pub fn set_terminal_height(px: f32) {
    let now = Instant::now();
    with_store(|store| record(store, now, |layout| layout.terminal_height = Some(px)));
}

/// Write the layout to disk once it has held still past [`SETTLE`]. Called
/// from the heartbeat, so a drag costs a single write when it stops.
pub fn flush_if_settled() {
    with_store(|store| {
        let Some(changed_at) = store.changed_at else {
            return;
        };
        if changed_at.elapsed() < SETTLE {
            return;
        }
        store.layout.persist();
        store.changed_at = None;
    });
}

fn persist_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("layout.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let layout = Layout {
            sidebar_width: Some(248.),
            sidepane_width: Some(460.),
            explorer_width: Some(300.),
            terminal_height: Some(260.),
        };
        let value = json!({
            "sidebar_width": layout.sidebar_width,
            "sidepane_width": layout.sidepane_width,
            "explorer_width": layout.explorer_width,
            "terminal_height": layout.terminal_height,
        });
        assert_eq!(Layout::from_value(&value), layout);
    }

    #[test]
    fn missing_and_out_of_bounds_fields_are_dropped() {
        let value = json!({
            "sidebar_width": 248.,
            "sidepane_width": 10.,      // below MIN_PX
            "explorer_width": 99999.,   // above MAX_PX
        });
        let layout = Layout::from_value(&value);
        assert_eq!(layout.sidebar_width, Some(248.));
        assert_eq!(layout.sidepane_width, None);
        assert_eq!(layout.explorer_width, None);
        assert_eq!(layout.terminal_height, None);
    }

    #[test]
    fn non_numeric_field_is_dropped() {
        let value = json!({ "sidebar_width": "wide" });
        assert_eq!(Layout::from_value(&value), Layout::default());
    }

    #[test]
    fn record_arms_the_settle_timer_only_on_change() {
        let now = Instant::now();
        let mut store = Store {
            layout: Layout::default(),
            changed_at: None,
        };
        // First set changes the value → armed.
        record(&mut store, now, |layout| layout.sidebar_width = Some(248.));
        assert!(store.changed_at.is_some());
        // Re-setting the same value must not re-arm (a still drag stays quiet).
        store.changed_at = None;
        record(&mut store, now, |layout| layout.sidebar_width = Some(248.));
        assert!(store.changed_at.is_none());
    }
}
