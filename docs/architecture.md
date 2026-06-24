# Architecture & operation

How the pieces fit together, how to run the host, and the reference tables that
used to live in the README. For the *why* behind each layer, follow the links
into [`designs/`](designs/).

## Layers

```
┌─────────────────────────────────────────────────────────────────┐
│                     Physical Controls Layer                      │
│  Scenes UI · Classic UI · HTTP/REST · WebSocket · MIDI · Web MIDI│
└────────────────────────────┬────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                    Control API (Axum, :7780)                     │
│  GET  /api/parameters          POST /api/parameters/{index}      │
│  POST /api/parameters/batch    POST /api/parameters/by-name      │
│  POST /api/midi/cc|note|raw    WS /api/ws    Static web/ surfaces │
└────────────────────────────┬────────────────────────────────────┘
                             │ parameter writes on the caller thread;
                             │ MIDI/macros via crossbeam command channel
┌────────────────────────────▼────────────────────────────────────┐
│                   Audio Host Thread (CoreAudio/cpal)             │
│  VST3 process() @ 48kHz · parameter delivery · MIDI injection    │
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

Design references:

- [`designs/vst3-hosting.md`](designs/vst3-hosting.md) — loading and driving the plugin.
- [`designs/overbridge-param-sync.md`](designs/overbridge-param-sync.md) — host ↔ device state sync.
- [`designs/audio-and-control-api.md`](designs/audio-and-control-api.md) — audio engine, command flow, API.
- [`designs/audio-cutout-and-duplex-fix.md`](designs/audio-cutout-and-duplex-fix.md) — the duplex + monitoring design.

## Running the host

With the device connected in **Overbridge USB mode** and the Overbridge Engine
running:

```bash
# Build once (or after code changes)
cargo build --release

# Default: control-only (no device audio path — safe alongside a DAW)
RUST_LOG=info ./target/release/ob-host --plugin "Digitakt"
```

`config/default.json` sets `control_only: true` and `duplex.enabled: false`.
To opt into the experimental duplex + monitoring path:

```bash
RUST_LOG=info ./target/release/ob-host --plugin "Digitakt" --duplex Digitakt
```

URLs (same server):

| Page | URL |
|------|-----|
| Scenes & A/B crossfader | http://127.0.0.1:7780/scenes.html |
| Classic parameter browser | http://127.0.0.1:7780/ |

Stop the server:

```bash
pkill -f 'target/release/ob-host'
```

> Static web assets are served with `Cache-Control: no-cache`, so a normal
> browser reload always picks up a rebuilt `scenes.js` / `scenes.html` (no hard
> refresh needed).

## Audio modes

| Mode | Command / config | Device audio |
|------|------------------|--------------|
| **Control-only** (default) | `control_only: true`, `duplex.enabled: false` in config (no extra flags) | Untouched — no device audio path; hardware keeps its own mix; DAW can use Overbridge audio in parallel |
| **Duplex + monitor** (opt-in) | `--duplex` or `duplex.enabled: true` | Analog Main Out plays USB return; host monitors device input back so the internal mix stays audible |
| Legacy monitor | `--audio` | Plugin output routed to device (usually silent; not recommended) |
| Passthru | `--passthru` | Loops device input back to output via cpal |

**Precedence:** `--duplex` or `duplex.enabled: true` overrides control-only.
The default config uses control-only only.

Duplex config keys: `duplex.device`, `duplex.monitor`, `duplex.monitor_source`,
`duplex.monitor_gain`. Full background:
[`designs/audio-cutout-and-duplex-fix.md`](designs/audio-cutout-and-duplex-fix.md)
and [`designs/audio-routing-and-control-options.md`](designs/audio-routing-and-control-options.md).

## CLI options

| Flag | Description |
|------|-------------|
| `--plugin NAME` | Plugin name substring (Digitakt, Syntakt, …) |
| `--duplex [DEVICE]` | Opt-in: native CoreAudio duplex on the Elektron device + monitor audio back to analog out |
| `--control-only` | Control without opening device audio (default via config; overridden by `--duplex`) |
| `--audio` | Legacy cpal monitor mode (plugin output to device) |
| `--passthru` | Loop device input back to output via cpal |
| `--list-plugins` | Scan and list available plugins |
| `--list-devices` | List cpal output devices and exit |
| `--port 7780` | API listen port |
| `--plugin-dir PATH` | VST3 scan directory |
| `--mappings PATH` | MIDI CC → parameter mapping file |
| `--no-engine` | Don't auto-launch Overbridge Engine |
| `--config PATH` | Host config JSON |
| `--gui` | Open the Overbridge plugin editor window (`OB_GUI=1`) |

Environment variables: `OB_PLUGIN`, `OB_PORT`, `OB_PLUGIN_DIR`.

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

After `setup.sh` you may also have `vendor/Overbridge Engine.app` locally
(gitignored).

## Development notes

### Always run the binary cargo actually built (CARGO_TARGET_DIR gotcha)

`cargo build` does **not** always write to `overbridge-scenes/target/`. If
`CARGO_TARGET_DIR` is set in the environment (e.g. some sandboxed shells point it
at a temp cache), cargo writes the fresh binary there instead, and running
`./target/release/ob-host` then silently launches a **stale** binary.

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
