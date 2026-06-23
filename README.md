# Overbridge Scenes

Octatrack-style **scene snapshots** and an **A/B crossfader** for Elektron
Overbridge devices (Digitakt, Syntakt, Analog Heat, Analog Rytm, and others).
A lightweight local VST3 host drives the Overbridge plugin and serves web
control surfaces, so you can snapshot and morph parameters **without a DAW**.

```
http://127.0.0.1:7780/scenes.html
```

## Features

- **4 scene snapshots per pattern** — each scene stores only the parameters you
  choose and the value each should take, exactly like an Octatrack scene.
- **A/B crossfader** — assign a scene to each side and drag to morph every
  parameter from A to B in real time. Snap buttons (`⟵ A`, `·`, `B ⟶`) too.
- **Crossfader takeover modes** — **Jump**, **Pickup**, and **Scale** so the
  morph reconciles gracefully with live hardware-knob moves instead of lurching.
- **Per-pattern scenes** — scenes are namespaced per pattern (bank A–P × 1–16).
  Switch patterns manually, or enable **Follow Program Change** to auto-switch
  from the device's MIDI Program Change.
- **Pattern baseline** — capture a neutral "home" snapshot per pattern, used as
  the morph target whenever a crossfader side is empty (`— None —`). Until you
  capture one, an empty side follows the live device value.
- **MIDI-controllable crossfader** — map an absolute fader (0–127) or an endless
  encoder to the crossfader from the browser (Web MIDI).
- **Live, bidirectional** — hardware knob moves stream into the UI; UI changes
  mirror to the device and the classic surface. Built-in **device monitoring**
  keeps the analog Main Out audible.
- **Two web surfaces + an API** — the scenes page, a classic parameter browser
  at `/`, and a full HTTP/WebSocket/MIDI control API for your own tools.

## What this repo contains

This repository ships **source code only**. It does **not** include Elektron's
proprietary software:

| Component | In this repo? | How you get it |
|-----------|---------------|----------------|
| Overbridge Scenes host + web UI | Yes | Clone and build |
| [`truce-rack-vst3`](vendor/truce-rack-vst3/) (open-source VST3 host crate) | Yes | Vendored (MIT or Apache-2.0) |
| Elektron Overbridge VST3 plugins | **No** | Install [Overbridge](https://www.elektron.se/support-downloads/overbridge), then run `./scripts/copy-plugins.sh` |
| Overbridge Engine | **No** | Installed with Overbridge; `setup.sh` may copy a local reference into `vendor/` (gitignored) |

Local copies created by the setup scripts live in `plugins/` and
`vendor/Overbridge Engine.app`. Both paths are **gitignored** and stay on your
machine.

## Quick start

```bash
git clone https://github.com/MartinNeifert/overbridge-scenes.git
cd overbridge-scenes

# Copy VST3 plugins from your system install + build
./scripts/setup.sh

# Ensure the device is USB-connected in Overbridge mode
./scripts/start-engine.sh

# Launch the host (duplex audio + monitoring is the default)
RUST_LOG=info ./target/release/ob-host --plugin Digitakt

# Open the scenes control surface
open http://127.0.0.1:7780/scenes.html
```

A classic parameter browser lives at `/` on the same server. For other run
modes, CLI flags, and the architecture overview, see
[`docs/architecture.md`](docs/architecture.md).

## Using scenes & the crossfader

### Build a scene

1. Pick the scene to edit under **Add parameters to**.
2. Search a parameter and click **＋** — it captures the current live value.
3. Turn a knob on the hardware, then click **⤓ Map** on that row to re-snapshot
   the live value. **Snapshot live** does this for every parameter in the scene.
4. Fine-tune a stored value with the row slider (this edits the *scene* only —
   it does not move the device). Remove a parameter with **✕**.
5. **Recall** applies a whole scene instantly, independent of the crossfader.

### Morph

Assign **Scene A** (left) and **Scene B** (right), then drag the crossfader. The
union of parameters across the two scenes is interpolated:

- locked in both scenes → morphs A-value ↔ B-value;
- locked in only one scene → morphs that lock ↔ the baseline;
- one side set to `— None —` → morphs the other scene ↔ the baseline.

Pick a takeover mode (**Jump** / **Pickup** / **Scale**) to control how the
morph meets live knob positions. Use **Capture baseline (pattern)** to set the
neutral home for empty sides; otherwise an empty side follows the live value.

Scenes persist in the browser (`localStorage`), namespaced per plugin **and**
per pattern. Full behaviour and rationale:
[`docs/designs/scenes-crossfader.md`](docs/designs/scenes-crossfader.md).

## Programmatic control

Everything the UI does is available over HTTP, WebSocket, and a virtual MIDI
port — poll/set parameters, send MIDI, drive a custom controller, or batch a
whole morph in one request. See [`docs/api-reference.md`](docs/api-reference.md).

```bash
curl http://127.0.0.1:7780/api/parameters | jq '.[0:5]'
```

## Documentation

| Doc | Purpose |
|-----|---------|
| [`docs/architecture.md`](docs/architecture.md) | Layered architecture, run modes, CLI flags, project layout, dev notes |
| [`docs/api-reference.md`](docs/api-reference.md) | HTTP / WebSocket / MIDI API, controller mapping, client examples |
| [`docs/designs/`](docs/designs/) | Design decisions — VST3 hosting, param/preset sync, audio + control API, [scenes & crossfader](docs/designs/scenes-crossfader.md) |
| [`docs/machines/`](docs/machines/) | Device-specific notes (e.g. [Analog Heat MKII](docs/machines/analog-heat-mk2.md)) |
| [`docs/active-issues/`](docs/active-issues/) | Open problems (e.g. [param-sync jitter](docs/active-issues/jitter-on-param-sync.md)) |

See [`docs/README.md`](docs/README.md) for the full index.

## Requirements

- macOS (Apple Silicon or Intel)
- [Elektron Overbridge](https://www.elektron.se/support-downloads/overbridge) installed (`/Applications/Elektron/`)
- Rust toolchain (`brew install rust` or rustup)
- Hardware in **Overbridge USB mode** (not MIDI-only)

## License

**Overbridge Scenes** (this repository) is licensed under the [MIT License](LICENSE).

**Third-party components included in this repo:**

- [`vendor/truce-rack-vst3`](vendor/truce-rack-vst3/) — MIT or Apache-2.0, at your option

**Not included — proprietary Elektron software you must install separately:**

- Elektron Overbridge VST3 plugins
- Overbridge Engine

Elektron is not affiliated with this project.
