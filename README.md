# Overbridge Scenes

Octatrack-style **scene snapshots** and an **A/B crossfader** for Elektron Overbridge devices (Digitakt, Syntakt, Analog Heat, Analog Rytm, and others). Includes a lightweight local VST3 host and web control surfaces so you can morph parameters without a DAW.

Open the scenes UI after starting the host:

```
http://127.0.0.1:7780/scenes.html
```

A classic parameter browser lives at `/` on the same server.

## What this repo contains

This repository ships **source code only**. It does **not** include Elektron's proprietary software:

| Component | In this repo? | How you get it |
|-----------|---------------|----------------|
| Overbridge Scenes host + web UI | Yes | Clone and build |
| [`truce-rack-vst3`](vendor/truce-rack-vst3/) (open-source VST3 host crate) | Yes | Vendored (MIT or Apache-2.0) |
| Elektron Overbridge VST3 plugins | **No** | Install [Overbridge](https://www.elektron.se/support-downloads/overbridge), then run `./scripts/copy-plugins.sh` |
| Overbridge Engine | **No** | Installed with Overbridge; `setup.sh` may copy a local reference into `vendor/` (gitignored) |

Local copies created by the setup scripts live in `plugins/` and `vendor/Overbridge Engine.app`. Both paths are **gitignored** and stay on your machine.

## Quick start

```bash
git clone https://github.com/MartinNeifert/overbridge-scenes.git
cd overbridge-scenes

# Copy VST3 plugins from your system install + build
./scripts/setup.sh

# Ensure device is USB-connected in Overbridge mode
./scripts/start-engine.sh

# Launch host (pick your device plugin)
./target/release/ob-host --plugin Digitakt

# Open scenes control surface
open http://127.0.0.1:7780/scenes.html
```

## Scenes & crossfader

A standalone control surface for snapshotting parameters into **4 scenes** and morphing between two of them with an Octatrack-style **A/B crossfader**.

### Concepts

- **Scene** — a snapshot of *specific* parameters and the value each should take (like an Octatrack scene: only the parameter locks you set).
- **Crossfader** — assign one scene to **A** (left) and one to **B** (right), then drag to linearly morph every parameter across the two scenes.
- **Baseline** — the neutral value used when one crossfader side is empty (`— None —`) or a parameter is only locked in one scene. Captured from the live device when you assign A/B (or via *Capture baseline*). Assign a scene to B, leave A as None, and slide right to fade current values into that scene.
- **Crossfader takeover** — optional soft-takeover modes (**Jump**, **Pickup**, **Scale**) so morphing respects live knob changes instead of snapping.
- **MIDI crossfader** — map an absolute fader (0–127) or infinite encoder to the crossfader from the browser (Web MIDI).
- **Per-pattern scenes** — scenes are stored per pattern (bank A–P × 1–16), so each Digitakt pattern keeps its own 4 scenes. Pick the pattern manually, or enable **Follow Program Change** to auto-switch from the device's MIDI Program Change. The Digitakt VST exposes no pattern parameter, so Program Change is the live source for the current pattern (see [docs/designs/scenes-crossfader.md](docs/designs/scenes-crossfader.md)).

### Building a scene

1. Pick the scene to edit in **Add parameters to**.
2. Search a parameter and click **＋** — it captures the current live value.
3. Turn the knob on the hardware, then click **⤓ Map** on that row to re-snapshot the live value. **Snapshot live** does this for every parameter already in the scene.
4. Fine-tune with the row slider, or remove with **✕**.
5. **Recall** applies a whole scene instantly, independent of the crossfader.

### Morphing

Assign **Scene A** and **Scene B**, then drag the crossfader (or use `⟵ A`, `·`, `B ⟶` to snap). The union of parameters across the two scenes is interpolated:

- locked in both scenes → morphs A-value ↔ B-value;
- locked in only one scene → morphs that lock ↔ the baseline;
- one side set to `— None —` → morphs the other scene ↔ the baseline.

Scenes are stored in the browser (`localStorage`), namespaced per loaded plugin, so Digitakt and Analog Heat keep independent scene sets. Writes go over WebSocket, so changes are reflected on the classic surface too.

See [docs/designs/scenes-crossfader.md](docs/designs/scenes-crossfader.md) for design rationale and morph semantics.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Physical Controls Layer                      │
│  Scenes UI · Classic UI · HTTP/REST · WebSocket · MIDI · Web MIDI│
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                    Control API (Axum, :7780)                     │
│  GET  /api/parameters          POST /api/parameters/{index}      │
│  POST /api/parameters/by-name  POST /api/midi/cc|note|raw        │
│  WS   /api/ws                  Static web/ control surfaces        │
└────────────────────────────┬────────────────────────────────────┘
                             │ crossbeam command channel
┌────────────────────────────▼────────────────────────────────────┐
│                   Audio Host Thread (cpal)                       │
│  VST3 process() @ 48kHz · parameter set · MIDI event injection   │
└────────────────────────────┬────────────────────────────────────┘
                             │ VST3 parameter + MIDI bridge
┌────────────────────────────▼────────────────────────────────────┐
│         Elektron Overbridge VST3 (your local install)            │
│  Digitakt · Syntakt · Analog Rytm · Digitone · etc.             │
└────────────────────────────┬────────────────────────────────────┘
                             │ Overbridge protocol
┌────────────────────────────▼────────────────────────────────────┐
│              Overbridge Engine (your local install)              │
│  USB driver · device sync · multi-channel audio routing          │
└────────────────────────────┬────────────────────────────────────┘
                             │ USB
┌────────────────────────────▼────────────────────────────────────┐
│                   Elektron Hardware Device                       │
└───────────────────────────────────────────────────────────────────┘
```

See [docs/](docs/) for design decisions, device-specific notes, and active issues. Start with [docs/designs/overbridge-param-sync.md](docs/designs/overbridge-param-sync.md) for how host ↔ device state stays in sync.

## What Elektron exposes (and what this host uses)

Elektron does **not** publish a standalone Overbridge API. Programmatic control flows through:

| Path | Capability |
|------|------------|
| **VST parameters** | Full sound-shaping, FX, macros — same knobs as the plugin UI |
| **MIDI to plugin** | Note sequencing, CC modulators, transport |
| **Physical device** | Knobs remain live; Overbridge mirrors state bidirectionally |

Overbridge Scenes wraps the VST parameter interface and MIDI input so your code never touches undocumented internals.

## API reference

### Status

```bash
curl http://127.0.0.1:7780/api/status
```

### List parameters

```bash
curl http://127.0.0.1:7780/api/parameters | jq '.[0:5]'
```

### Set parameter by index

```bash
curl -X POST http://127.0.0.1:7780/api/parameters/42 \
  -H 'Content-Type: application/json' \
  -d '{"value": 0.75}'
```

### Set parameter by name

```bash
curl -X POST http://127.0.0.1:7780/api/parameters/by-name \
  -H 'Content-Type: application/json' \
  -d '{"name": "Filter Cutoff", "value": 0.5}'
```

### Send MIDI

```bash
# Note on
curl -X POST http://127.0.0.1:7780/api/midi/note \
  -H 'Content-Type: application/json' \
  -d '{"channel": 0, "note": 60, "velocity": 100, "on": true}'

# Control change
curl -X POST http://127.0.0.1:7780/api/midi/cc \
  -H 'Content-Type: application/json' \
  -d '{"channel": 0, "controller": 1, "value": 64}'
```

### WebSocket (real-time)

Connect to `ws://127.0.0.1:7780/api/ws` — receives parameter snapshots at 10 Hz.

Send commands:

```json
{"action": "set_parameter", "index": 12, "value": 0.8}
{"action": "set_parameter_by_name", "name": "Filter Cutoff", "value": 0.6}
```

## Physical controller mapping

Edit `config/mappings.example.json` (or copy to `config/mappings.local.json`):

```json
{
  "mappings": [
    {
      "source": { "type": "cc", "channel": 0, "controller": 1 },
      "target": { "parameter": "Filter Cutoff", "curve": "linear" }
    }
  ]
}
```

The host creates a virtual MIDI input port (default name: **Overbridge Host Control**). Route your controller (or Max, TouchOSC, etc.) to that port.

Run with custom mappings:

```bash
./target/release/ob-host --plugin Syntakt --mappings config/mappings.local.json
```

## Project layout

```
overbridge-scenes/
├── config/            # Host + MIDI mapping configuration
├── docs/              # Design decisions, machine notes, active issues
├── scripts/           # setup.sh, copy-plugins.sh, start-engine.sh
├── src/               # Rust VST host + API (binary: ob-host)
├── vendor/
│   └── truce-rack-vst3/   # Vendored open-source VST3 host (in repo)
├── web/               # Scenes UI, classic UI, shared device header
├── plugins/           # Local Elektron VST3 copies (gitignored, not in repo)
└── target/            # Build output (gitignored)
```

After `setup.sh`, you may also have `vendor/Overbridge Engine.app` locally (gitignored).

## Documentation

Project docs live in [`docs/`](docs/), organized by purpose:

| Folder | Purpose |
|--------|---------|
| [`docs/designs/`](docs/designs/) | Architecture decisions — VST3 hosting, Overbridge param/preset sync, audio + control API, [audio routing & control options](docs/designs/audio-routing-and-control-options.md), [scenes & crossfader](docs/designs/scenes-crossfader.md) |
| [`docs/machines/`](docs/machines/) | Device-specific notes (e.g. [Analog Heat MKII](docs/machines/analog-heat-mk2.md)) |
| [`docs/active-issues/`](docs/active-issues/) | Open problems (e.g. [param-sync jitter](docs/active-issues/jitter-on-param-sync.md)) |

See [`docs/README.md`](docs/README.md) for the full index.

## CLI options

| Flag | Description |
|------|-------------|
| `--plugin NAME` | Plugin name substring (Digitakt, Syntakt, …) |
| `--list-plugins` | Scan and list available plugins |
| `--port 7780` | API listen port |
| `--plugin-dir PATH` | VST3 scan directory |
| `--mappings PATH` | MIDI CC → parameter mapping file |
| `--no-engine` | Don't auto-launch Overbridge Engine |
| `--config PATH` | Host config JSON |

Environment variables: `OB_PLUGIN`, `OB_PORT`, `OB_PLUGIN_DIR`.

## Building physical controls

Recommended patterns:

1. **Web UI (included)** — Scenes crossfader at `/scenes.html`; classic parameter browser at `/`. Extend `web/scenes.js` or `web/app.js` for custom layouts.
2. **Python/Node client** — Poll `/api/parameters`, POST changes. Good for scripting macros.
3. **MIDI hardware** — Map knobs to CC in `config/mappings.json`; host translates CC → VST parameter.
4. **WebSocket client** — Lowest-latency bidirectional control for custom UIs (TouchOSC bridge, etc.).

Example Python client:

```python
import requests

BASE = "http://127.0.0.1:7780"

params = requests.get(f"{BASE}/api/parameters").json()
print(f"Loaded {len(params)} parameters")

requests.post(f"{BASE}/api/parameters/by-name", json={
    "name": "Filter Cutoff",
    "value": 0.42,
})
```

## Development notes

### Always run the binary cargo actually built (CARGO_TARGET_DIR gotcha)

`cargo build` does **not** always write to `overbridge-scenes/target/`. If `CARGO_TARGET_DIR` is set in the environment (e.g. some sandboxed shells point it at a temp cache), cargo writes the fresh binary there instead. Running `./target/release/ob-host` then silently launches a **stale** binary.

Avoid it by always launching the binary cargo just produced:

```bash
# Best: let cargo resolve the path and run it
cargo run --release -- --plugin "Analog Heat"

# Or resolve the real target dir explicitly
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')
"$TARGET_DIR/release/ob-host" --plugin "Analog Heat"

# Or force a local target dir for the session
unset CARGO_TARGET_DIR   # or: export CARGO_TARGET_DIR="$PWD/target"
cargo build --release && ./target/release/ob-host --plugin "Analog Heat"
```

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
