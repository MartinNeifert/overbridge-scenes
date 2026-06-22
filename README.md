# Overbridge Host

A local VST3 host for Elektron Overbridge devices with a programmatic control API and web-based control surface. Load Digitakt, Syntakt, Analog Rytm, and other Overbridge plugins without a DAW, then drive every exposed parameter via HTTP, WebSocket, or MIDI.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Physical Controls Layer                      │
│  Web UI · HTTP/REST · WebSocket · MIDI CC · Virtual MIDI Port   │
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                    Control API (Axum, :7780)                     │
│  GET  /api/parameters          POST /api/parameters/{index}      │
│  POST /api/parameters/by-name  POST /api/midi/cc|note|raw        │
│  WS   /api/ws                  Static web/ control surface       │
└────────────────────────────┬────────────────────────────────────┘
                             │ crossbeam command channel
┌────────────────────────────▼────────────────────────────────────┐
│                   Audio Host Thread (cpal)                       │
│  VST3 process() @ 48kHz · parameter set · MIDI event injection   │
└────────────────────────────┬────────────────────────────────────┘
                             │ VST3 parameter + MIDI bridge
┌────────────────────────────▼────────────────────────────────────┐
│              Elektron Overbridge VST3 (local copy)               │
│  Digitakt · Syntakt · Analog Rytm · Digitone · etc.             │
└────────────────────────────┬────────────────────────────────────┘
                             │ Overbridge protocol
┌────────────────────────────▼────────────────────────────────────┐
│                   Overbridge Engine (system)                     │
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

This host wraps the VST parameter interface and MIDI input so your code never touches undocumented internals.

## Quick start

```bash
cd overbridge-host

# Copy VST3 plugins from system install + build
./scripts/setup.sh

# Ensure device is USB-connected in Overbridge mode
./scripts/start-engine.sh

# Launch host (pick your device plugin)
./target/release/ob-host --plugin Digitakt

# Open control surface
open http://127.0.0.1:7780/
```

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

## Scenes & Crossfader (Octatrack-style)

A standalone control surface for snapshotting parameters into **4 scenes** and
morphing between two of them with an Octatrack-style **A/B crossfader**.

Open it at:

```
http://127.0.0.1:7780/scenes.html
```

It is fully separate from the classic control surface (`/`) — no shared files,
no changes to the existing UI.

### Concepts

- **Scene** — a snapshot of *specific* parameters and the value each should take
  (just like an Octatrack scene, which only stores the parameter locks you set).
- **Crossfader** — assign one scene to **A** (left) and one to **B** (right),
  then drag to linearly morph every parameter across the two scenes.
- **Baseline** — the neutral value used for a parameter when one crossfader side
  is empty (`— None —`) or a parameter is only locked in one of the two scenes.
  It is captured from the live device when you assign A/B (or via *Capture
  baseline*). This is what makes "fade the current values → a scene" work: assign
  a scene to B, leave A as None, and slide right.

### Building a scene (the "Map" workflow)

1. Pick the scene to edit in **Add parameters to**.
2. Search a parameter and click **＋** — it captures the current live value.
3. Turn the knob on the hardware, then click **⤓ Map** on that row to re-snapshot
   the live value as the scene value. (Per-row, exactly the "turn knob → map"
   flow.) **Snapshot live** does this for every parameter already in the scene.
4. Fine-tune any captured value with its slider, or remove it with **✕**.
5. **Recall** applies a whole scene instantly, independent of the crossfader.

### Morphing

Assign **Scene A** and **Scene B**, then drag the crossfader (or use `⟵ A`, `·`,
`B ⟶` to snap). The union of parameters across the two scenes is interpolated:

- locked in both scenes → morphs A-value ↔ B-value;
- locked in only one scene → morphs that lock ↔ the baseline;
- one side set to `— None —` → morphs the other scene ↔ the baseline.

Scenes are stored in the browser (`localStorage`), namespaced per loaded plugin,
so Digitakt and Analog Heat keep independent scene sets. Writes go over the same
WebSocket the classic surface uses, so changes are reflected everywhere live.

See [docs/designs/scenes-crossfader.md](docs/designs/scenes-crossfader.md) for the
design rationale and morph semantics.

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

The host creates a virtual MIDI input port named **Overbridge Host Control**. Route your controller (or Max, TouchOSC, etc.) to that port.

Run with custom mappings:

```bash
./target/release/ob-host --plugin Syntakt --mappings config/mappings.local.json
```

## Project layout

```
overbridge-host/
├── plugins/           # Copied Elektron VST3 bundles (gitignored)
├── vendor/            # Overbridge Engine.app reference copy
├── config/            # Host + MIDI mapping configuration
├── web/               # Browser control surface
├── scripts/           # setup.sh, copy-plugins.sh, start-engine.sh
├── src/               # Rust VST host + API
└── docs/              # Design decisions, machine notes, active issues
```

## Documentation

Project docs live in [`docs/`](docs/), organized by purpose:

| Folder | Purpose |
|--------|---------|
| [`docs/designs/`](docs/designs/) | Architecture decisions and the reasoning behind them — VST3 hosting, Overbridge param/preset sync, audio + control API, [audio routing & control options](docs/designs/audio-routing-and-control-options.md) |
| [`docs/machines/`](docs/machines/) | Device-specific implementation notes and observed behavior (e.g. [Analog Heat MKII](docs/machines/analog-heat-mk2.md)) |
| [`docs/active-issues/`](docs/active-issues/) | Open problems with root-cause analysis and next steps (e.g. [param-sync jitter](docs/active-issues/jitter-on-param-sync.md)) |

See [`docs/README.md`](docs/README.md) for the full index and the conventions each doc type follows.

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

1. **Web UI (included)** — Pin parameters, drag sliders, send MIDI CC. Extend `web/app.js` for custom layouts.
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

`cargo build` does **not** always write to `overbridge-host/target/`. If
`CARGO_TARGET_DIR` is set in the environment (e.g. some sandboxed/agent shells
point it at a temp cache like
`/var/folders/.../cursor-sandbox-cache/.../cargo-target`), cargo writes the
fresh binary there instead. Running `./target/release/ob-host` then silently
launches a **stale** binary, so code changes appear to have no effect.

Symptoms: you edit + rebuild, logs still show old strings/behavior, and
`ls -la target/release/ob-host` shows an old mtime.

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

Quick check that you're running fresh code:

```bash
strings "$(cargo metadata --no-deps --format-version 1 | python3 -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')/release/ob-host" | grep "some-new-log-string"
```

## Requirements

- macOS (Apple Silicon or Intel)
- Elektron Overbridge installed (`/Applications/Elektron/`)
- Rust toolchain (installed via `brew install rust` or rustup)
- Hardware in **Overbridge USB mode** (not MIDI-only)

## License

MIT. Elektron VST plugins and Overbridge Engine remain proprietary Elektron software — the bundled copies in `plugins/` and `vendor/` are not redistributable.
