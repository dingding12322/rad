# Security Policy

## Supported versions

Orbit is pre-1.0. Security fixes are made against the latest release and the
`main` branch.

| Version | Supported |
| ------- | --------- |
| `main` (latest) | ✅ |
| Latest tagged release | ✅ |
| Older releases | ❌ |

## Reporting a vulnerability

Please report vulnerabilities **privately** — do not open a public issue.

Use GitHub's [private vulnerability reporting](https://github.com/imrj05/orbit/security/advisories/new)
on this repository, or email the maintainer at
**info@orbitpi.app**.

Include, where possible:

- A description of the issue and its impact
- Steps to reproduce, or a proof of concept
- The Orbit version, macOS version, and how Orbit was installed
  (`cargo run`, `.app` bundle, DMG)
- The `pi` CLI version (`pi --version`)

You can expect an acknowledgement within a few days and a status update as the
report is triaged. Please give us a reasonable window to ship a fix before any
public disclosure.

## Scope and security model

Orbit is a local desktop client. It spawns the `pi` coding agent as a child
process and gives it the same local tool access you would have in a terminal.
A few properties are **by design** and are not vulnerabilities on their own:

- **Orbit runs the agent with full local tool access.** The agent can read,
  edit, and run commands in your workspace as your user. This is the point of
  the tool.
- **Access modes are a confirmation guard, not a sandbox.** Supervised /
  Auto-accept edits / Full access decide which mutating tool calls prompt first.
  `pi` ships no sandbox, and Orbit does not add one. A mode that auto-approves a
  call is not an isolation boundary.
- **Session data lives in pi's own store** (`~/.pi/agent/`, `~/.orbit-pi/`).
  Orbit reads and writes the same files the `pi` CLI does.

Reports we do want: memory-safety bugs, command or prompt injection that crosses
a boundary Orbit claims to enforce, credentials leaking across the RPC surface
(see `crates/orbit-rpc/docs/auth-rpc.md` and `quota-rpc.md`), or the signed
updater accepting an unverifiable artifact.
