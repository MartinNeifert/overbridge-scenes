# Design: audio engine, control API, and other decisions

Decisions not specific to VST3 or Overbridge sync.

## Duplex audio via cpal on the Overbridge device

`audio.rs` opens the Overbridge device as a **duplex** stream with cpal: an input stream
captures device channels into a shared buffer, and the output callback feeds those into the
plugin's `process()` and writes the result back.

- **Why duplex:** Overbridge devices are effect/IO endpoints (e.g. Analog Heat is 4-in/4-out
  @ 48 kHz). The plugin expects real input audio to process; output-only would starve it.
- Input and output configs are negotiated to the device sample rate (48 kHz) and a fixed
  block size (`block_size`, default 128). The input callback resizes its scratch buffers if
  the driver hands a larger frame count.
- Bus layout is chosen from channel count (`bus_layout_for_channels`): mono/stereo/discrete;
  the VST3 layer maps that to a speaker arrangement (`0x33` quad for 4 ch), falling back to
  stereo if the plugin rejects the wide layout.

## Commands to the audio thread, snapshots back

All mutations (`set_parameter`, MIDI, macros) are sent as `HostCommand`s over a crossbeam
channel and applied on the audio thread inside the `process()` lock (`apply_command`).

- **Why:** keeps all plugin COM calls on threads that already hold the plugin mutex in a
  predictable order, and keeps parameter delivery sample-accurate (applied right before the
  next `process()`).
- Readback is decoupled: a `RwLock<Vec<ParameterSnapshot>>` cache is updated by the sync
  passes and read by the HTTP/WS handlers, so API reads never touch the plugin mutex.

## Control API: Axum HTTP + WebSocket on :7780

- REST for discrete control (`GET/POST /api/parameters…`, `/api/midi/…`), WebSocket for
  low-latency bidirectional control and parameter snapshots.
- A `param_epoch` atomic is bumped on every change so the WS layer can push deltas
  (`take_pending_ws_updates`) rather than full snapshots.
- Static web control surface served from `web/`.

## MIDI bridge + virtual port

`midir` creates a virtual input port ("Overbridge Host Control") and maps incoming CC to
named parameters via `config/mappings*.json`. Lets hardware controllers / Max / TouchOSC
drive parameters without going through HTTP.

## Main run loop driven by tokio interval

`main.rs` runs a 4 ms tokio interval that calls `host.runloop_tick()` on the main thread,
which (a) drains `performEdit` values from `param_change_rx` into the cache and (b) pumps the
editor (`EditorPump::tick`).

- **Why the main thread:** AppKit / `NSRunLoop` and the plugin editor must be serviced on the
  process main thread; tokio's `current_thread` flavor keeps us there.

## Build / target-dir gotcha

If `CARGO_TARGET_DIR` is set (some sandboxed shells point it at a temp cache), `cargo build`
writes there, not `./target`, and running `./target/release/ob-host` launches a stale binary.
Prefer `cargo run`, or resolve the dir via `cargo metadata`. See the README "Development
notes".
