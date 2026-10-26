# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

A native desktop surface (macOS first, Windows/Linux later) drawn with GPUI — Zed's GPU-composited, keyboard-first workbench language. No webview, no DOM. The `adaptive` value marks it as one cross-platform native codebase rather than a web app.

Orbit is a native application first. Its interaction model, layout behavior, keyboard navigation, focus management, scrolling, panels, menus, and window behavior should feel like a professional desktop application rather than a web application running inside a desktop shell.

---

## Users

Pi coding agent users — developers who already run the pi CLI and want a fast desktop GUI for their agent sessions.

They arrive with existing agent knowledge (models, thinking effort, sessions, tool access); the UI can use pi's own terminology without teaching it.

Released publicly, not a personal-only tool, so it cannot assume access to the author's machine, providers, models, paths, credentials, or habits.

The primary user is a developer who may spend hours inside Orbit during long-running coding sessions. The interface therefore prioritizes scanability, information density, keyboard interaction, stable spatial layout, performance, and clear agent state over decorative visual expression.

---

## Product Purpose

Orbit is a native desktop workbench for the pi coding agent — a chat and development workspace rendered entirely in Rust by GPUI, the same GPU-accelerated UI framework Zed is built on.

Every pixel (transcript, markdown, diffs, charts, chrome) is drawn by the GPU: no browser, no webview, no Node daemon.

The app speaks the pi CLI's native RPC protocol directly over stdio, so the agent runtime is the same `pi` binary users already know.

Orbit is not a generic AI chat frontend. The transcript is one surface of a broader development workbench that connects:

- Agent sessions
- Projects
- Files
- Git
- Models
- Thinking effort
- Tools
- Agent activity
- Usage
- Extensions
- Future terminals and parallel agents

Success: pi users reach for Orbit instead of (or alongside) the terminal and feel the difference in daily work — from sub-frame scroll on 10k-message transcripts to responsive keyboard navigation, contextual actions, fast file interaction, and zero web overhead.

The single-session chat is a waypoint, not the destination.

---

## Positioning

Orbit is a native desktop workbench for the pi coding agent.

It brings pi's existing agent runtime, sessions, models, tools, and project context into a fast GPU-rendered desktop environment.

Orbit is not a replacement agent runtime and not a generic AI chat client.

The pi CLI remains the source of truth for agent execution, sessions, models, and persistence. Orbit provides a native interface and workbench around that runtime.

Orbit differentiates through:

- Native Rust + GPUI rendering
- Direct pi CLI RPC over stdio
- Local-first operation
- Long-session performance
- Keyboard-first workflows
- Integrated project and file context
- Agent activity visibility
- Git and development workflows
- Native desktop interaction
- Extensible workbench architecture

Sessions created in Orbit and in the CLI are the same sessions (`~/.pi/agent/sessions/`).

The model catalog, thinking levels, and persistence are pi's own, presented through a pure-Rust client rather than an SDK daemon.

---

## UX Philosophy

Orbit should feel like a native professional developer tool rather than a web-based AI chat application.

The UX prioritizes:

1. **Workspace over chat** — the primary experience is a persistent development workspace containing sessions, agent activity, files, Git, and contextual tools.
2. **Keyboard-first interaction** — frequent actions should have keyboard paths and be discoverable through the command palette.
3. **Contextual complexity** — secondary actions should appear when relevant instead of permanently occupying the interface.
4. **High information density** — show useful information without creating visual clutter.
5. **Progressive disclosure** — detailed tool output, diffs, metadata, and activity should be expandable rather than permanently visible.
6. **Stable spatial memory** — navigation, panels, and frequently used controls should remain predictable between sessions.
7. **Native behavior** — focus, selection, scrolling, resizing, shortcuts, menus, and window behavior should feel native to a desktop application.
8. **Quiet chrome** — application chrome should support the task rather than compete with the active workspace.
9. **Performance as UX** — responsiveness during long-running agent tasks is part of the product experience.
10. **Truth over decoration** — visual indicators must represent real application or pi state.

Orbit should feel dense and capable without feeling cramped.

---

## Information Architecture

Orbit should be structured around a persistent workbench rather than a collection of independent pages.

The primary information model is:

```text
Workspace
├── Projects
│   ├── Sessions
│   ├── Files
│   └── Git
│
├── Agent
│   ├── Sessions
│   ├── Models
│   ├── Thinking effort
│   ├── Tools
│   └── Activity
│
├── Insights
│   └── Usage
│
└── System
    ├── Extensions
    └── Settings
