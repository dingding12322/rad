//! The in-app toast stack: transient cards that confirm actions and carry
//! background notifications while the window is frontmost.
//!
//! A toast is born in [`Toasts::push`], lives for its kind's TTL, fades over
//! [`FADE`], and leaves when [`Toasts::expire`] runs or the user dismisses it.
//! The stack is capped at [`MAX_VISIBLE`]: a burst of events can never bury
//! the window, and the oldest card drops instead.
//!
//! This is the model only — `app/toast_ui.rs` paints it. Failures keep their
//! persistent banner; a toast is for facts that survive being missed.

use std::time::{Duration, Instant};

/// Cards visible at once; the oldest is dropped when a new one arrives.
pub const MAX_VISIBLE: usize = 4;

/// How long a card spends fading out. It is removed at TTL + FADE, so the
/// paint animation (sized TTL + FADE) reaches zero exactly as it leaves.
pub const FADE: Duration = Duration::from_millis(180);

/// The four toast roles. The kind picks the glyph, the tint, and the TTL —
/// an error deserves more reading time than a copy confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    /// How long the card holds before it starts fading.
    pub fn ttl(self) -> Duration {
        match self {
            Self::Success => Duration::from_secs(3),
            Self::Info => Duration::from_secs(4),
            Self::Warning => Duration::from_secs(6),
            Self::Error => Duration::from_secs(8),
        }
    }
}

/// One queued card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    /// Optional second line (a turn summary, an error detail).
    pub body: Option<String>,
    pub created: Instant,
    /// Bumped when a repeat push refreshes this card, so its life animation
    /// restarts instead of resuming a completed (invisible) one.
    pub revision: u64,
}

impl Toast {
    /// Whether the card has finished its TTL and fade-out.
    pub fn is_expired(&self, now: Instant, fade: Duration) -> bool {
        now.saturating_duration_since(self.created) >= self.kind.ttl() + fade
    }
}

/// The stack, oldest first (the layer paints newest nearest the corner).
#[derive(Debug, Default)]
pub struct Toasts {
    next_id: u64,
    items: Vec<Toast>,
}

impl Toasts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a toast and return its id. A repeat of a card still on screen
    /// (same kind, title, and body — a retried command, a repeated settle)
    /// refreshes it instead of stacking a twin.
    pub fn push(&mut self, kind: ToastKind, title: impl Into<String>, body: Option<String>) -> u64 {
        let title = title.into();
        let now = Instant::now();
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|toast| toast.kind == kind && toast.title == title && toast.body == body)
        {
            existing.created = now;
            existing.revision += 1;
            return existing.id;
        }
        let id = self.next_id;
        self.next_id += 1;
        if self.len() >= MAX_VISIBLE {
            self.items.remove(0);
        }
        self.items.push(Toast {
            id,
            kind,
            title,
            body,
            created: now,
            revision: 0,
        });
        id
    }

    /// Remove a card; returns whether it was on the stack.
    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|toast| toast.id != id);
        self.items.len() != before
    }

    /// Drop expired cards; returns whether the stack changed.
    pub fn expire(&mut self, now: Instant, fade: Duration) -> bool {
        let before = self.items.len();
        self.items.retain(|toast| !toast.is_expired(now, fade));
        self.items.len() != before
    }

    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_push_refreshes_instead_of_stacking() {
        let mut toasts = Toasts::new();
        let first = toasts.push(ToastKind::Success, "Copied", None);
        let again = toasts.push(ToastKind::Success, "Copied", None);
        assert_eq!(first, again);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts.items()[0].revision, 1);
        // A different kind or body is a different card.
        toasts.push(ToastKind::Success, "Copied", Some("path".into()));
        assert_eq!(toasts.len(), 2);
    }

    #[test]
    fn push_caps_the_stack_dropping_the_oldest() {
        let mut toasts = Toasts::new();
        let mut ids = Vec::new();
        for ix in 0..MAX_VISIBLE + 2 {
            ids.push(toasts.push(ToastKind::Info, format!("event {ix}"), None));
        }
        assert_eq!(toasts.len(), MAX_VISIBLE);
        let kept: Vec<u64> = toasts.items().iter().map(|toast| toast.id).collect();
        assert_eq!(kept, ids[2..].to_vec());
    }

    #[test]
    fn dismiss_removes_only_the_named_card() {
        let mut toasts = Toasts::new();
        let a = toasts.push(ToastKind::Info, "a", None);
        let b = toasts.push(ToastKind::Info, "b", None);
        assert!(toasts.dismiss(a));
        assert!(!toasts.dismiss(a));
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts.items()[0].id, b);
    }

    #[test]
    fn expire_honors_kind_ttl_and_fade() {
        let mut toasts = Toasts::new();
        toasts.push(ToastKind::Success, "done", None);
        let created = toasts.items()[0].created;
        let ttl = ToastKind::Success.ttl();
        // Still holding at the end of the TTL, gone once the fade elapsed.
        assert!(!toasts.expire(created + ttl, FADE));
        assert_eq!(toasts.len(), 1);
        assert!(toasts.expire(created + ttl + FADE, FADE));
        assert!(toasts.is_empty());
    }

    #[test]
    fn longer_kinds_outlive_shorter_ones() {
        assert!(ToastKind::Error.ttl() > ToastKind::Warning.ttl());
        assert!(ToastKind::Warning.ttl() > ToastKind::Info.ttl());
        assert!(ToastKind::Info.ttl() > ToastKind::Success.ttl());
    }
}
