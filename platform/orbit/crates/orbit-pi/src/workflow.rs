//! Workflow mode — what a session is *for*, and its plan progress.
//!
//! Orbit sessions are scoped as **Plan** (explore and produce a plan, change
//! nothing), **Build** (normal agent work — the default), or **Ask** (read-only
//! question answering). This is orthogonal to [`crate::access`]: access mode
//! answers "may this mutating call run?", workflow mode answers "what is this
//! session for?".
//!
//! Workflow state is **per session**, keyed by pi's session id in
//! `~/.orbit-pi/workflow.json`:
//!
//! ```json
//! { "0192…-uuid": "plan", "0193…-uuid": "build" }
//! ```
//!
//! The bundled workflow extension (`contrib/orbit-workflow-extension/`) reads
//! that file fresh on every agent hook, so changing the mode re-arms a live
//! session with no restart — the same contract the access guard relies on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The workflow a session is scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkflowMode {
    /// Explore and produce a plan; mutating tools are disabled.
    Plan,
    /// Normal agent work. The default, so existing sessions are unchanged.
    #[default]
    Build,
    /// Answer questions from the codebase; mutating tools are disabled.
    Ask,
}

impl WorkflowMode {
    /// Every mode, in picker order.
    pub const ALL: [WorkflowMode; 3] = [Self::Plan, Self::Build, Self::Ask];

    /// The wire/persistence id. Kept in sync with `MODES` in the extension's
    /// `policy.js`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Build => "build",
            Self::Ask => "ask",
        }
    }

    /// Parse a stored id, falling back to [`WorkflowMode::default`] (Build) on
    /// anything unknown — a hand-edited file never widens what a session may do.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "plan" => Self::Plan,
            "build" => Self::Build,
            "ask" => Self::Ask,
            _ => Self::default(),
        }
    }

    /// Short label for the composer chip and picker rows.
    pub fn label(self) -> String {
        match self {
            Self::Plan => tr!("workflow.plan"),
            Self::Build => tr!("workflow.build"),
            Self::Ask => tr!("workflow.ask"),
        }
    }

    /// One-line explanation shown under the label in the picker.
    pub fn description(self) -> String {
        match self {
            Self::Plan => tr!("workflow.plan_hint"),
            Self::Build => tr!("workflow.build_hint"),
            Self::Ask => tr!("workflow.ask_hint"),
        }
    }

    /// The bundled icon path for the chip and picker row.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Plan => "icons/tools/todo.svg",
            Self::Build => "icons/rocket-01.svg",
            Self::Ask => "icons/tools/ask.svg",
        }
    }

    /// Whether the mode keeps the agent read-only (Plan and Ask). The
    /// extension disables `edit`/`write` and gates bash for these.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Plan | Self::Ask)
    }
}

// ── per-session store ───────────────────────────────────────────────────────

/// `~/.orbit-pi/workflow.json` — session id → wire id.
fn store_path() -> PathBuf {
    crate::platform::home_dir()
        .join(".orbit-pi")
        .join("workflow.json")
}

fn load_map_at(path: &Path) -> BTreeMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, String>>(&raw).unwrap_or_default()
}

fn write_map_at(path: &Path, map: &BTreeMap<String, String>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(body) = serde_json::to_string(map) else {
        return;
    };
    // Temp-file + rename: the extension reads this file fresh while a pi
    // process is running, so a reader must never observe a partial write.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Load the mode recorded for a session, falling back to Build.
pub fn load_for(session_id: &str) -> WorkflowMode {
    load_map_at(&store_path())
        .get(session_id)
        .map(|wire| WorkflowMode::from_wire(wire))
        .unwrap_or_default()
}

/// Record the mode for a session. The workflow extension reads the file fresh
/// on every hook, so a write re-arms a live session.
pub fn persist_for(session_id: &str, mode: WorkflowMode) {
    let path = store_path();
    let mut map = load_map_at(&path);
    if map.get(session_id).map(String::as_str) == Some(mode.as_wire()) {
        return;
    }
    map.insert(session_id.to_string(), mode.as_wire().to_string());
    write_map_at(&path, &map);
}

/// Drop entries whose session no longer exists, so the store cannot grow
/// without bound as sessions are deleted.
pub fn prune(known_session_ids: &[String]) {
    let path = store_path();
    let mut map = load_map_at(&path);
    let before = map.len();
    map.retain(|id, _| known_session_ids.iter().any(|known| known == id));
    if map.len() != before {
        write_map_at(&path, &map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let unique = format!(
            "orbit-workflow-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("workflow.json")
    }

    #[test]
    fn wire_round_trips() {
        for mode in WorkflowMode::ALL {
            assert_eq!(WorkflowMode::from_wire(mode.as_wire()), mode);
        }
    }

    #[test]
    fn unknown_wire_falls_back_to_build() {
        assert_eq!(WorkflowMode::from_wire("yolo"), WorkflowMode::Build);
        assert_eq!(WorkflowMode::from_wire(""), WorkflowMode::Build);
        assert_eq!(WorkflowMode::default(), WorkflowMode::Build);
    }

    #[test]
    fn every_mode_has_distinct_copy() {
        let labels: std::collections::HashSet<_> =
            WorkflowMode::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(labels.len(), WorkflowMode::ALL.len());
        for mode in WorkflowMode::ALL {
            assert!(!mode.description().is_empty());
            assert!(mode.icon().starts_with("icons/"));
        }
    }

    #[test]
    fn read_only_flags() {
        assert!(WorkflowMode::Plan.is_read_only());
        assert!(WorkflowMode::Ask.is_read_only());
        assert!(!WorkflowMode::Build.is_read_only());
    }

    #[test]
    fn store_round_trips_per_session() {
        let path = temp_path("store");
        let mut map = load_map_at(&path);
        map.insert("s1".into(), "plan".into());
        map.insert("s2".into(), "build".into());
        write_map_at(&path, &map);

        let read = load_map_at(&path);
        assert_eq!(read.get("s1").map(String::as_str), Some("plan"));
        assert_eq!(read.get("s2").map(String::as_str), Some("build"));
        assert_eq!(
            WorkflowMode::from_wire(read.get("s1").unwrap()),
            WorkflowMode::Plan
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
