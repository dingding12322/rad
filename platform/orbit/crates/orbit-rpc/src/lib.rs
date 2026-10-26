//! `orbit-rpc` — a Rust client for the pi CLI RPC protocol.
//!
//! The pi CLI (`pi --mode rpc`) exposes a strict-JSONL protocol over stdio:
//! commands on stdin, events on stdout. This crate owns one pi child process
//! per [`PiClient`], routes responses, and streams typed [`Event`]s.
//!
//! Design rules (see AGENT.md):
//! - framing is LF-only; never split on Unicode separators
//! - envelopes parse loosely; unknown shapes flow through as raw JSON
//! - the client never blocks a UI frame: draining is `try_recv`-based

pub mod client;
pub mod types;

pub use client::{
    augmented_path, hide_console, pi_bin_candidates, pi_binary, pi_package_root,
    pi_package_root_from, PiClient, PI_BIN_ENV, PI_PACKAGE_ROOT_ENV,
};
pub use types::{
    is_auth_event, parse_quota_reports, AssistantMessageEvent, AuthErrorCode, AuthEvent,
    AuthMethodCapability, AuthProvider, Command, CommandBody, ContextUsage, Event, MessageUsage,
    PendingQueue, QuotaBalance, QuotaKind, QuotaReport, QuotaWindow, SessionState, SessionUsage,
};
