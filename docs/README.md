# Overbridge Scenes documentation

Project documentation, organized by purpose.

## Start here

- [`../README.md`](../README.md) — feature overview and quick start.
- [`architecture.md`](architecture.md) — layered architecture, CLI flags, project layout, and development notes.
- [`api-reference.md`](api-reference.md) — HTTP / WebSocket / MIDI control API, physical-controller mapping, and client examples.

## Structure

- **[`active-issues/`](active-issues/)** — open problems with enough context to pick up
  later: symptoms, root-cause analysis so far, what's been tried, and leading hypotheses.
  - [`jitter-on-param-sync.md`](active-issues/jitter-on-param-sync.md) — periodic
    choppiness during parameter changes.

- **[`designs/`](designs/)** — design decisions and the reasoning ("why"), so the
  architecture is understandable without re-deriving it from the code.
  - [`vst3-hosting.md`](designs/vst3-hosting.md) — how plugins are loaded and driven.
  - [`overbridge-param-sync.md`](designs/overbridge-param-sync.md) — host ↔ device state
    sync; what Overbridge does and does not expose.
  - [`audio-routing-and-control-options.md`](designs/audio-routing-and-control-options.md) —
    control-only host vs. program audio in the box, and why hosting the VST is the only
    full programmatic-control option.
  - [`linux-overbridge-testing.md`](designs/linux-overbridge-testing.md) — phased strategy
    for developing on Linux (fake-plugin CI, native VST3 smoke, Wine, hardware lab).
  - [`scenes-crossfader.md`](designs/scenes-crossfader.md) — Octatrack-style scenes and the
    A/B crossfader morph engine (standalone web surface at `/scenes.html`).

- **[`machines/`](machines/)** — device-specific implementation notes and observed behavior.
  - [`analog-heat-mk2.md`](machines/analog-heat-mk2.md) — Elektron Analog Heat MKII.

- [`hardware-sync.md`](hardware-sync.md) — overview of hardware/IPC access (see
  `designs/overbridge-param-sync.md` for the authoritative sync design).

## Conventions

- An **active issue** doc should let someone resume cold: symptom, where it lives, root
  causes found, mitigations applied, and what to try next.
- A **design** doc records a decision and its rationale, including trade-offs and known
  limitations — not just what the code does.
- A **machine** doc captures device-specific facts and observed quirks (param counts,
  callback behavior, async timing) that the rest of the design relies on.
