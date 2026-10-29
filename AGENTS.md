# Development Rules

Shared reading, output, checkpoint, verification, and delegation rules live in `~/.codex/AGENTS.md`. Keep only repository-specific rules here.

## Conversational Style

- No emojis in commits, issues, PR comments, or code
- Explain non-trivial designs and problems as: problem, concrete example or short trace, then solution. State why the solution is necessary and distinguish it from optional complexity.
- When the user asks a question, answer it first before making edits or running implementation commands.
- When responding to user feedback or an analysis, explicitly say whether you agree or disagree before saying what you changed.

## Code Quality

- No `any` unless absolutely necessary.
- Check node_modules for external API types; don't guess.
- **No inline imports** (`await import()`, `import("pkg").Type`, dynamic type imports). Top-level imports only.
- In `packages/coding-agent`, resolve package assets through helpers in `src/config.ts`. Do not use `__dirname` directly; the helpers account for source checkouts, npm installations, and standalone binaries.
- Never remove or downgrade code to fix type errors from outdated deps; upgrade the dep instead.
- Use only erasable TypeScript syntax (Node strip-only mode) in code checked by the root config (`packages/*/src`, `packages/*/test`, `packages/coding-agent/examples`): no parameter properties, `enum`, `namespace`/`module`, `import =`, `export =`, or other constructs needing JS emit. Use explicit fields with constructor assignments.
- Always ask before removing functionality or code that appears intentional.
- Do not preserve backward compatibility unless the user asks for it.
- Never hardcode key checks (e.g. `matchesKey(keyData, "ctrl+x")`). Add defaults to `DEFAULT_EDITOR_KEYBINDINGS` or `DEFAULT_APP_KEYBINDINGS` so they stay configurable.
- Never modify `packages/ai/src/models.generated.ts` directly; update `packages/ai/scripts/generate-models.ts` instead, then regenerate. Including the resulting `models.generated.ts` diff is always OK, even if regeneration includes unrelated upstream model metadata changes. The generated model data in `packages/ai/src/providers/data/` is gitignored: never stage it, and don't look for it in git history.

## Commands

- After code changes (not docs): run `npm run check` once. If output is large, preserve the full log on disk and return the result, relevant diagnostics, and log path. Fix all errors, warnings, and infos before committing. Does not run tests.
- Never run `npm run build` or `npm test` unless requested by the user.
- For ad-hoc scripts, `write` them to a temp file (e.g. `/tmp`), run, edit if needed, remove when done. Don't embed multi-line scripts in `bash` commands.

## On-Demand Workflows

Read only the matching section of [.pi/agent-workflows.md](.pi/agent-workflows.md) before the relevant action. Loading a procedure does not authorize the action or override Personal Overrides.

- Tests: before selecting or running tests; includes the ban on running the full Vitest suite directly.
- Dependencies: before install/update commands or dependency/lockfile edits.
- Git: before staging, committing, rebasing, branch changes, or other mutations. Routine read-only status/diff checks need no extra procedure.
- Issues And PRs: before reviewing or acting on GitHub issues or PRs.
- Changelog: before editing package changelogs.
- Releasing: before release preparation, publishing, verification, or recovery.

## User Override

If the user's instructions conflict with any rule in this document, ask for explicit confirmation before overriding. Only then execute their instructions.
