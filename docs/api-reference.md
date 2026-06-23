# API reference

The host exposes an HTTP + WebSocket control API on `http://127.0.0.1:7780`
(port configurable via `--port` / `OB_PORT` / config `api_port`), plus a virtual
MIDI input port. Everything the web surfaces do is available to your own code.

For the design rationale behind this surface see
[`designs/audio-and-control-api.md`](designs/audio-and-control-api.md).

## REST

### Status

```bash
curl http://127.0.0.1:7780/api/status
```

### Selector (devices + loaded plugin)

```bash
curl http://127.0.0.1:7780/api/selector
```

### List parameters

```bash
curl http://127.0.0.1:7780/api/parameters | jq '.[0:5]'
```

Each entry is a `ParameterSnapshot`:

```json
{
  "index": 0, "id": 46760448, "name": "T1 Sample Tune", "short_name": "T1 Sample Tune",
  "unit": "", "min": 0.0, "max": 1.0, "default": 0.714, "value": 0.696, "display": "0.70"
}
```

### Get one parameter

```bash
curl http://127.0.0.1:7780/api/parameters/42
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

### Set many parameters at once (batch)

Applies all updates under a single plugin lock and one `process()` pass, so a
multi-parameter change (e.g. a crossfader morph frame) reaches the hardware
together. This is what the scenes UI uses.

```bash
curl -X POST http://127.0.0.1:7780/api/parameters/batch \
  -H 'Content-Type: application/json' \
  -d '{"updates": [{"index": 8, "value": 0.25}, {"index": 12, "value": 0.6}]}'
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

# Raw bytes
curl -X POST http://127.0.0.1:7780/api/midi/raw \
  -H 'Content-Type: application/json' \
  -d '{"data": [144, 60, 100]}'
```

## WebSocket (real-time)

Connect to `ws://127.0.0.1:7780/api/ws`.

**Receive** — the server streams parameter state:

- `{"type": "parameters", "data": [ParameterSnapshot, …]}` — periodic full
  snapshot (also on epoch change / plugin switch).
- `{"type": "param_updates", "data": [{index, value, display}, …]}` — deltas,
  including hardware knob moves echoed back from the device.

**Send** — apply changes:

```json
{"action": "set_parameter", "index": 12, "value": 0.8}
{"action": "set_parameter_by_name", "name": "Filter Cutoff", "value": 0.6}
```

Both REST and WebSocket writes apply on the calling thread holding the plugin
lock and run one `process()` so the change reaches the hardware via
`IParameterChanges` (see [`designs/overbridge-param-sync.md`](designs/overbridge-param-sync.md)).

## Physical controller mapping (virtual MIDI port)

The host creates a virtual MIDI input port (default name **Overbridge Host
Control**). Route a hardware controller, Max, TouchOSC, etc. to that port; CC
messages are translated to named parameters via a mapping file.

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

Run with custom mappings:

```bash
./target/release/ob-host --plugin Syntakt --mappings config/mappings.local.json
```

> The scenes-page crossfader can also be driven by a MIDI controller (absolute
> fader or endless encoder) directly in the browser via Web MIDI — that path is
> independent of this virtual port and needs no mapping file.

## Building custom controllers

Recommended integration patterns:

1. **Web UI (included)** — scenes crossfader at `/scenes.html`; classic
   parameter browser at `/`. Extend `web/scenes.js` or `web/app.js`.
2. **Python / Node client** — poll `/api/parameters`, POST changes. Good for
   scripting macros and generative control.
3. **MIDI hardware** — map knobs to CC in a mappings file; the host translates
   CC → VST parameter.
4. **WebSocket client** — lowest-latency bidirectional control for custom UIs
   (TouchOSC bridge, etc.).

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

## What Elektron exposes (and what this host uses)

Elektron does **not** publish a standalone Overbridge API. Programmatic control
flows entirely through the VST3 plugin:

| Path | Capability |
|------|------------|
| **VST parameters** | Full sound-shaping, FX, macros — the same knobs as the plugin UI |
| **MIDI to plugin** | Note sequencing, CC modulators, transport |
| **Physical device** | Knobs remain live; Overbridge mirrors state bidirectionally |

Overbridge Scenes wraps the VST parameter interface and MIDI input so your code
never touches undocumented internals. See
[`designs/overbridge-param-sync.md`](designs/overbridge-param-sync.md) for the
limits of what the plugin surface reveals (e.g. there is no pattern-index
parameter).
