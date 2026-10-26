# Orbit Privacy

Orbit is local-first. Your source code, projects, terminal, AI conversations,
and usage history stay on your machine.

**Orbit collects no telemetry.** There is no analytics client, no installation
or session identifier, no usage reporting, and no third-party tracking SDK in
the app. Nothing about how you use Orbit is sent anywhere.

## What leaves your machine

Orbit talks to the network only for things you asked for:

* **Update checks** — the app fetches its own signed release feed to see whether
  a newer version exists, and to download it if you accept.
* **GitHub metadata** — author avatars and repository data shown in the Git and
  GitHub panels.
* **Model providers** — prompts, context, and tool output are sent to whichever
  provider your `pi` agent is configured to use, exactly as the `pi` CLI does on
  its own. That traffic is governed by the provider's policy, not Orbit's.

None of these carry a telemetry identifier, and Orbit adds no tracking data to
them.

## What Orbit stores locally

Orbit keeps its own settings and state under `~/.orbit-pi/`, and reads session
data that `pi` writes. This never leaves your machine except through the
provider calls above:

* workspace list, marks, and sort preferences;
* theme, appearance, and interface preferences, including your favorite
  and recently used models;
* window, terminal, and layout state;
* session-retention policy.

There is no `analytics.json`, no pseudonymous identifier, and no queue of
events.

## Privacy controls

**Settings → Privacy** holds both local cleanup controls. **Session retention**
lets you archive or delete old `pi` sessions on a schedule you choose, entirely
on your machine. **Temporary files** lists Orbit's own scratch entries in
its `orbit-pi` folder inside the system temp directory — bug-report
screenshots, update downloads, and helper scripts — with **Open folder** and
**Clean up now**; older top-level `orbit-*` leftovers are included too, and
entries a live Orbit process still needs are kept. **AI scratch files** lists
pi's own `pi-*` entries in the temp root — shell output, codemode images, and
share/editor working files — and only removes files last used more than an
hour ago. Orbit never uploads either.

## Source of truth

The absence of telemetry is enforced structurally: there is no
`crates/orbit-analytics` crate and no `crate::analytics` module. If a
telemetry path is ever reconsidered, this document must change first.
