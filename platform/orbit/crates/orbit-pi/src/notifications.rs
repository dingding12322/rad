//! Notification channels for background agent events: a system banner, the
//! alert sound, and an in-app toast.
//!
//! Three independent channels, all persisted to
//! `~/.orbit-pi/notifications.json` alongside the other UI prefs:
//! **desktop** — a system banner, **sound** — the system alert sound, and
//! **toasts** — the in-app card that stands in for the banner while the
//! window is frontmost. Banner and sound are suppressed while the window is
//! frontmost (the transcript is the notification then, and a banner over a
//! window you are reading is noise); the toast channel is what those events
//! surface through instead.
//!
//! macOS is the shipping target (INTENT.md D5). Banners go through
//! `UNUserNotificationCenter` when Orbit runs from its `.app` bundle — the
//! only API that attributes the banner to Orbit itself. A bare `cargo run`
//! binary cannot use that API (it raises without a bundle), so it falls back
//! to `osascript`'s `display notification`, whose banner macOS attributes to
//! Script Editor; Settings says so rather than pretending otherwise. A
//! bundled banner carries the session path in its identifier, and the
//! notification delegate routes a click back to that session.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

/// Prefix marking a banner identifier as carrying a session path, so a
/// clicked banner knows which session to open.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SESSION_PREFIX: &str = "orbit-session:";

/// A session path parked by the notification delegate. The ObjC callback has
/// no `App` handle, so it writes here and the heartbeat (`OrbitApp::tick`)
/// drains it on the UI thread.
static CLICKED_SESSION: Mutex<Option<String>> = Mutex::new(None);

/// Install the click handler that routes a clicked banner back to its
/// session. Called once during startup. A no-op without a bundle: osascript
/// banners have no click callback.
pub fn init() {
    #[cfg(target_os = "macos")]
    notification_delegate::install();
}

/// The session a clicked banner pointed at, if one is waiting to be routed.
pub fn take_clicked_session() -> Option<String> {
    CLICKED_SESSION.lock().ok().and_then(|mut slot| slot.take())
}

/// System sound played by the sound channel. A stock macOS alert — no asset
/// to ship, and the user can tune it with the system volume.
#[cfg(target_os = "macos")]
const SOUND_NAME: &str = "Glass";

/// Longest notification body kept, in characters. Smart-reply cutoffs and
/// Notification Center both truncate anyway; a preview is what gets read.
pub(crate) const BODY_PREVIEW_CHARS: usize = 180;

/// Longest title / subtitle kept, in characters.
#[cfg(target_os = "macos")]
const LINE_PREVIEW_CHARS: usize = 64;

/// The three notification channels, persisted between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    /// Show a system banner for background events.
    pub desktop: bool,
    /// Play the alert sound for background events.
    pub sound: bool,
    /// Show an in-app toast for background events while the window is up.
    pub toasts: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            desktop: true,
            sound: true,
            toasts: true,
        }
    }
}

impl Prefs {
    fn persist_path() -> PathBuf {
        crate::platform::home_dir()
            .join(".orbit-pi")
            .join("notifications.json")
    }

    pub fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::persist_path()) else {
            return Self::default();
        };
        serde_json::from_str::<Value>(&raw)
            .map(|value| Self::from_value(&value))
            .unwrap_or_default()
    }

    /// Read prefs from JSON, falling back to defaults per field so a partial
    /// or malformed file never silences a channel by accident.
    fn from_value(value: &Value) -> Self {
        let mut prefs = Self::default();
        if let Some(desktop) = value.get("desktop").and_then(Value::as_bool) {
            prefs.desktop = desktop;
        }
        if let Some(sound) = value.get("sound").and_then(Value::as_bool) {
            prefs.sound = sound;
        }
        if let Some(toasts) = value.get("toasts").and_then(Value::as_bool) {
            prefs.toasts = toasts;
        }
        prefs
    }

    pub fn persist(self) {
        let path = Self::persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            serde_json::json!({
                "desktop": self.desktop,
                "sound": self.sound,
                "toasts": self.toasts,
            })
            .to_string(),
        );
    }
}

/// How macOS currently treats Orbit's banners. `Unbundled` is the honest
/// state of a `cargo run` binary: there is no app identity to attribute a
/// notification to, so the osascript fallback speaks as Script Editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAuth {
    Unknown,
    /// Only macOS can observe the system's notification authorization, so
    /// these two are never constructed elsewhere.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Granted,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Denied,
    /// The process is not a `.app` bundle, so banners go out through the
    /// osascript fallback and macOS attributes them to Script Editor. Only
    /// macOS reaches this state; other platforms report [`Unknown`].
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Unbundled,
}

/// Collapse whitespace and truncate on a word boundary. Notification bodies
/// arrive as model output: multi-line markdown would otherwise render as a
/// wall of text in a banner — or toast — that shows a few lines.
pub(crate) fn preview(text: &str, max_chars: usize) -> String {
    // Split first, then drop non-whitespace control characters: filtering
    // before the split would glue words together across the removed spaces.
    let collapsed = text
        .split_whitespace()
        .map(|word| word.chars().filter(|c| !c.is_control()).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    // `nth(max_chars)` is the byte offset just past the last kept character.
    let cut = collapsed
        .char_indices()
        .nth(max_chars)
        .map(|(ix, _)| ix)
        .unwrap_or(collapsed.len());
    let end = collapsed[..cut]
        .rfind(' ')
        .filter(|space| *space >= cut / 2)
        .unwrap_or(cut);
    format!("{}…", collapsed[..end].trim_end())
}

// ── macOS ───────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn notify(session: Option<&Path>, title: &str, subtitle: &str, body: &str) {
    let title = preview(title, LINE_PREVIEW_CHARS);
    let subtitle = preview(subtitle, LINE_PREVIEW_CHARS);
    let body = preview(body, BODY_PREVIEW_CHARS);
    if is_bundled() {
        post_banner(session, &title, &subtitle, &body);
    } else {
        post_osascript(&title, &subtitle, &body);
    }
}

/// Whether the process runs from a `.app` bundle. The UserNotifications
/// framework raises without a bundle identifier, so this gate is what keeps
/// `cargo run` from crashing.
#[cfg(target_os = "macos")]
fn is_bundled() -> bool {
    objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some()
}

/// Post through `UNUserNotificationCenter` — the banner carries Orbit's own
/// name and icon. No sound is attached: the sound channel plays separately,
/// so the two switches never double up.
#[cfg(target_os = "macos")]
fn post_banner(session: Option<&Path>, title: &str, subtitle: &str, body: &str) {
    use objc2_foundation::NSString;
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter,
    };

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    if !subtitle.is_empty() {
        content.setSubtitle(&NSString::from_str(subtitle));
    }
    content.setBody(&NSString::from_str(body));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&banner_identifier(session)),
        &content,
        None,
    );
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, None);
}

/// Banner identifier. A session banner encodes the session file path so a
/// click can route back to it; one without a session falls back to a
/// per-process sequence that merely keeps identifiers unique.
#[cfg(target_os = "macos")]
fn banner_identifier(session: Option<&Path>) -> String {
    match session {
        Some(path) => format!("{SESSION_PREFIX}{}", path.to_string_lossy()),
        None => format!("orbit-{}", request_id()),
    }
}

#[cfg(target_os = "macos")]
fn request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Fallback for unbundled dev builds: AppleScript Standard Additions.
/// `osascript` only receives `argv` when running a script *file*, so the
/// strings are escaped into the statement instead.
#[cfg(target_os = "macos")]
fn post_osascript(title: &str, subtitle: &str, body: &str) {
    use std::process::{Command, Stdio};

    let script = format!(
        "display notification \"{}\" with title \"{}\" subtitle \"{}\"",
        applescript_literal(body),
        applescript_literal(title),
        applescript_literal(subtitle),
    );
    let _ = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Escape a string for an AppleScript double-quoted literal. `preview`
/// already removed control characters, so backslashes and quotes are the
/// whole grammar that matters.
#[cfg(target_os = "macos")]
fn applescript_literal(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
pub fn play_sound() {
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;

    if let Some(sound) = NSSound::soundNamed(&NSString::from_str(SOUND_NAME)) {
        sound.play();
    }
}

/// Record the session a click announced. Only paths Orbit itself wrote are
/// routed; anything else (another app's banner, a stale one) is ignored.
#[cfg(target_os = "macos")]
fn record_click(identifier: &str) {
    let Some(path) = identifier.strip_prefix(SESSION_PREFIX) else {
        return;
    };
    if let Ok(mut slot) = CLICKED_SESSION.lock() {
        *slot = Some(path.to_string());
    }
}

#[cfg(target_os = "macos")]
mod notification_delegate {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread};
    use objc2_user_notifications::{
        UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::record_click;

    define_class!(
        // SAFETY: `NSObject` has no subclassing requirements, the class holds
        // no ivars, and it implements only the delegate protocol, whose
        // methods the notification center calls on the main thread.
        #[unsafe(super(NSObject))]
        #[name = "OrbitNotificationDelegate"]
        struct NotificationDelegate;

        unsafe impl NSObjectProtocol for NotificationDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
            #[unsafe(
                method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)
            )]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                record_click(&response.notification().request().identifier().to_string());
                completion.call(());
            }
        }
    );

    impl NotificationDelegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: `NSObject`'s `init` has no requirements; the object was
            // just allocated with this class and its (empty) ivars.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Install the delegate that turns a banner click into a session to
    /// reopen. `UNUserNotificationCenter` keeps its delegate weak, so the
    /// app owns one for the process lifetime.
    pub(super) fn install() {
        if !super::is_bundled() {
            return;
        }
        static DELEGATE: OnceLock<Retained<NotificationDelegate>> = OnceLock::new();
        let delegate = DELEGATE.get_or_init(NotificationDelegate::new);
        let protocol = ProtocolObject::from_ref(&**delegate);
        UNUserNotificationCenter::currentNotificationCenter().setDelegate(Some(protocol));
    }
}

/// Ask macOS for permission to post banners. The system prompt appears only
/// while the decision is `NotDetermined`; the answer is read back later
/// through [`permission`]. A no-op without a bundle — the osascript fallback
/// answers to Script Editor's own permission.
#[cfg(target_os = "macos")]
pub fn request_permission() {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    if !is_bundled() {
        return;
    }
    let handler = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &handler);
}

/// Read the current banner authorization. Blocks briefly on the framework's
/// completion handler, so call it from a background executor thread, never
/// from the UI thread.
#[cfg(target_os = "macos")]
pub fn permission() -> DesktopAuth {
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2_user_notifications::{
        UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
    };

    if !is_bundled() {
        return DesktopAuth::Unbundled;
    }
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the system passes a valid settings object that is alive for
        // the duration of the completion handler.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = tx.send(status);
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&handler);
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(status)
            if status == UNAuthorizationStatus::Authorized
                || status == UNAuthorizationStatus::Provisional
                || status == UNAuthorizationStatus::Ephemeral =>
        {
            DesktopAuth::Granted
        }
        Ok(status) if status == UNAuthorizationStatus::Denied => DesktopAuth::Denied,
        _ => DesktopAuth::Unknown,
    }
}

// ── other platforms (D5: Windows/Linux land after macOS) ────────────────

#[cfg(not(target_os = "macos"))]
pub fn notify(_session: Option<&Path>, _title: &str, _subtitle: &str, _body: &str) {}

#[cfg(not(target_os = "macos"))]
pub fn play_sound() {}

#[cfg(not(target_os = "macos"))]
pub fn request_permission() {}

#[cfg(not(target_os = "macos"))]
pub fn permission() -> DesktopAuth {
    DesktopAuth::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_collapses_whitespace_and_truncates_on_a_word() {
        assert_eq!(preview("  hello\n\nworld\t!", 64), "hello world !");
        let long = "the quick brown fox jumps over the lazy dog";
        let cut = preview(long, 20);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(cut.chars().count() <= 21, "{cut}");
        assert!(!cut.contains("  "), "{cut}");
        assert!(long.starts_with(cut.trim_end_matches('…')), "{cut}");
    }

    #[test]
    fn preview_drops_control_characters() {
        assert_eq!(preview("a\u{7}b\u{1b}c", 64), "abc");
    }

    #[test]
    fn prefs_read_known_fields_and_default_the_rest() {
        assert_eq!(
            Prefs::from_value(&json!({ "desktop": false })),
            Prefs {
                desktop: false,
                sound: true,
                toasts: true,
            }
        );
        assert_eq!(Prefs::from_value(&json!({})), Prefs::default());
        assert_eq!(
            Prefs::from_value(&json!({ "sound": false, "desktop": true })),
            Prefs {
                desktop: true,
                sound: false,
                toasts: true,
            }
        );
        assert_eq!(
            Prefs::from_value(&json!({ "toasts": false })),
            Prefs {
                desktop: true,
                sound: true,
                toasts: false,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_literal_escapes_quotes_and_backslashes() {
        assert_eq!(
            applescript_literal(r#"say "hi" \ now"#),
            r#"say \"hi\" \\ now"#
        );
    }
}
