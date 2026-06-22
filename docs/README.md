# Overbridge Scenes documentation

Project documentation, organized by purpose.

## Structure

- **[`active-issues/`](active-issues/)** — open problems with enough context to pick up
  later: symptoms, root-cause analysis so far, what's been tried, and leading hypotheses.
  - [`jitter-on-param-sync.md`](active-issues/jitter-on-param-sync.md) — periodic
    choppiness during parameter changes.
  - [`audio-artifacts-duplex-monitoring.md`](active-issues/audio-artifacts-duplex-monitoring.md) —
    clicks/dropouts in the `--duplex` monitor path (resolved: monitoring decoupled
    from the plugin lock), with remaining optional optimizations.

- **[`designs/`](designs/)** — design decisions and the reasoning ("why"), so the
  architecture is understandable without re-deriving it from the code.
  - [`vst3-hosting.md`](designs/vst3-hosting.md) — how plugins are loaded and driven.
  - [`overbridge-param-sync.md`](designs/overbridge-param-sync.md) — host ↔ device state
    sync; what Overbridge does and does not expose.
  - [`audio-and-control-api.md`](designs/audio-and-control-api.md) — audio engine, command
    flow, HTTP/WS/MIDI API.
  - [`audio-routing-and-control-options.md`](designs/audio-routing-and-control-options.md) —
    keeping program audio in the box vs. ob-host's control-plane audio loop, and why hosting
    the VST/AU plugin (ob-host) is the only full programmatic-control option.
  - [`audio-cutout-and-duplex-fix.md`](designs/audio-cutout-and-duplex-fix.md) — why the
    Digitakt went silent ~5 s after the host connected (Engine latency-probe fault + USB-return
    monitoring), and the native single-AUHAL duplex + monitoring fix.
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
