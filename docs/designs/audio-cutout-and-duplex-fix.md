# Design: Digitakt audio cut-out and the native CoreAudio duplex fix

How ob-host went from "the Digitakt goes silent a few seconds after the host
connects" to "the device's audio plays continuously while the host is fully
connected and controlling it." This doc records the symptoms, the dead ends, the
two distinct root causes, and the fix that shipped.

> TL;DR. There were **two** separate failures hiding behind one symptom
> ("no audio"):
> 1. The Overbridge Engine **faulted** ("latency measurement failed") because
>    ob-host drove `process()` from a software timer / two unsynchronised cpal
>    AudioUnits, so the Engine's round-trip latency probe never settled. A single
>    duplex AUHAL (one device, one clock — what a DAW does) fixed this. An
>    Elektron **firmware update** independently fixed the fault as well.
> 2. Once the fault was gone the device was still **silent**, because in
>    Overbridge mode the Digitakt's analog Main Out plays the **USB return**
>    (host monitoring), and ob-host was writing silence / a dead plugin bus back
>    to it. **Monitoring** the device's own audio (which arrives on the CoreAudio
>    input channels) back to its output restored sound.

## Symptom

With the Overbridge Engine running and the Digitakt in Overbridge USB mode:

- A DAW (e.g. Ableton/Bitwig) could host the Overbridge VST and the Digitakt
  kept playing — **after** selecting the Digitakt as the DAW's audio device.
- ob-host would connect, the editor/IPC would come up, parameters worked for a
  few seconds, then the Digitakt's audio **cut out (~5 s)** and the Overbridge
  Engine showed a fault.

The "~5 s then silence" timing is the fingerprint of the Engine's round-trip
**latency measurement** giving up.

## How the Overbridge Engine measures the host

The Overbridge Engine talks to the hardware over USB privately and exposes the
device to the host as the VST plugin **plus** a CoreAudio device. To trust a
host it runs a continuous round-trip **latency probe**: it emits a probe on the
device output and times it returning on the device input. That measurement only
settles if the host presents a **single, sample-coherent duplex stream** — one
device, one clock, input and output locked together — exactly the topology a DAW
creates when you select the device as its audio interface.

If the host instead:

- drives `process()` from a **software timer** (`std::thread::sleep`), or
- opens **separate** input and output AudioUnits on independent IO procs/clocks
  (which is what `cpal` does on macOS),

…then the probe never settles, the Engine faults ("latency measurement failed"),
and it cuts the hardware audio after a few seconds.

## Root cause #1 — incoherent clock (the fault)

Earlier ob-host audio modes all failed the probe:

- **Software-timer loop** — jitter the Engine eventually rejects.
- **cpal "duplex"** — on macOS `cpal` opens input and output as *two* AudioUnits
  with independent clocks; they are not sample-locked, so the round trip is
  incoherent.
- Control-only / input-only / output-only / clock-on-another-device — none
  present the single coherent duplex stream the Engine needs on the Elektron
  device itself.

### Dead ends explored (and why they failed)

| Attempt | Why it didn't work |
|---|---|
| Drive `process()` from a `sleep` timer | Clock jitter → probe never settles → fault |
| Clock `process()` from the system default output device | Coherent clock, but not on the Elektron device; the device's own duplex probe still unsatisfied |
| cpal duplex on the Digitakt | Two unsynchronised AudioUnits; not sample-locked |
| Input-only / output-only on the Digitakt | A half-duplex stream can't satisfy a round-trip probe |
| Avoid opening the Elektron device at all (control-only) | Keeps the hardware audio, but gives up monitoring and still isn't a host the Engine measures as "audio-capable" |

### Fix #1 — a single duplex AUHAL (one device, one clock)

`cpal` cannot express a single sample-locked duplex AudioUnit on macOS, so
ob-host now uses CoreAudio directly (`coreaudio-sys`) to build one
`kAudioUnitSubType_HALOutput` (AUHAL) unit bound to the Elektron device with
**both** input and output IO enabled and **one** output render callback. Inside
that single callback we:

1. Pull the device input via `AudioUnitRender` on the input element, and
2. Drive the plugin's `process()` from that same coherent clock, and
3. Write the device output —

all on one clock, which is precisely the topology a DAW produces. This is the
only configuration whose round-trip latency the Engine can measure without
faulting.

See `src/host/coreaudio_duplex.rs` (module doc explains the AUHAL setup) and
`AudioEngine::run_coreaudio_duplex` in `src/host/audio.rs`.

> Note: an Elektron **firmware update** independently resolved the
> "latency measurement failed" fault during this work. The coherent-duplex
> design is still the correct host architecture (it is what makes the Engine
> measurement robust and matches DAW behaviour), and it is required for Fix #2.

## Root cause #2 — the device's audio is on the input, not the plugin bus (the silence)

After the fault stopped, the Digitakt was **still silent** through ob-host even
though the duplex stream was healthy (steady ~375 callbacks/s, zero render
errors). A level-metering diagnostic in the render callback was decisive:

```
duplex: 375 callbacks/s, input-render errors 0, device-in peak 0.1054, plugin-out peak 0.0000
```

- **device-in peak ≈ 0.10–0.14** → the Digitakt's audio **is** reaching the
  computer, on the CoreAudio **input** channels (device → computer over USB).
- **plugin-out peak = 0.0000** → the Overbridge VST's audio **output bus is dead
  silent** in this hosting context.

So the device's sound never flows *through* the plugin's output bus — routing
that bus to the device output (the earlier approach) was always going to be
silent. And while an Overbridge host is connected, the Digitakt's analog Main
Out plays the **USB return** (host monitoring), not its own internal mix. That is
exactly why the DAW only produced sound **after** the Digitakt was selected as
its **output** device: the DAW then monitors the device's tracks back to it.

### Fix #2 — monitor the device's audio back to its output

In the same duplex render callback, when monitoring is enabled we copy the
device's own input channels (default Main L/R = input ch 0–1) — with optional
gain — straight to the device output. This replicates DAW monitoring and the
analog Main Out becomes audible again. With monitoring off we present silence
but keep the output stream running, so the duplex clock and the Engine's
round-trip measurement stay coherent.

The transport context handed to `process()` is also made DAW-like (an advancing,
"playing" timeline with `continousTimeSamples` set), because the Overbridge
plugin only streams when it sees a valid, advancing transport.

## The shipped configuration

Native duplex + monitoring is now a first-class mode, not a diagnostic toggle.

- **CLI:** `ob-host --plugin "Digitakt" --duplex [DEVICE]`
  (DEVICE defaults to config, then the plugin name).
- **Config (`config/default.json`):**

```json
"duplex": {
  "enabled": true,
  "device": "Digitakt",
  "monitor": true,
  "monitor_source": 0,
  "monitor_gain": 1.0
}
```

- `monitor_source` is the first device **input** channel used as the monitor
  source (left); the next channel is right. `0` = Main L/R.
- `monitor_gain` is a linear gain on the monitored signal.

The previous diagnostic environment variables (`OB_CA_LOOPBACK`,
`OB_CA_SILENCE_OUT`) were removed; `OB_CA_DUPLEX=<device>` is still honored as a
convenience override that enables the mode with monitoring on.

### Real-time safety

The render callback never blocks the audio thread: it takes the plugin lock with
`try_lock()` (skipping a block rather than waiting), drains host commands and
pending events without blocking, and does only lock-free metering. Heavy
editor-thread work (state save, param scan) stays off the audio thread.

## How to verify

1. Start: `ob-host --plugin "Digitakt" --duplex Digitakt` (or just `--duplex`
   with the config block above).
2. Logs should show, then hold steady with no fault:

```
Duplex mode: single AUHAL on "Digitakt" (one device, one clock; DAW-equivalent), monitor on
CoreAudio duplex mode: single AUHAL on "Digitakt" — 12 in / 2 out @ 48000 Hz, block 128 (monitor on, source ch 0, gain 1.00; ...)
duplex: 375 callbacks/s (total N), input-render errors 0 (last status 0), device-in peak ~0.10
```

3. The Digitakt's analog Main Out plays continuously while parameters/MIDI are
   controlled and the Engine stays active (no "latency measurement failed").

## Commit references

History of the area, newest first:

- **`46e0fe4`** — *Fix Digitakt audio cut-out: native CoreAudio duplex + device
  monitoring.* Adds `src/host/coreaudio_duplex.rs` (single duplex AUHAL),
  `DuplexSettings`/`DuplexConfig`, the `--duplex` flag and config block, the
  advancing transport context and `read_first_output_bus` in the vendored VST3
  host, and `try_lock()` real-time safety. (This doc was committed separately on
  top of it.)
- **`02dc862`** — *Document audio routing and programmatic control options.*
  Established the control-only vs. monitor framing and why hosting the VST is the
  only full programmatic-control path. See
  [`audio-routing-and-control-options.md`](audio-routing-and-control-options.md).
- **`fdb37d6`** — *Fix Overbridge preset→UI sync and document architecture.*
- **`baa0d87`** — *Add Overbridge Host: local VST3 host with HTTP/WS control and
  hardware sync.* Introduced ob-host and the original (cpal-based) audio loop
  that this fix replaces on macOS.

## Related docs

- [`audio-routing-and-control-options.md`](audio-routing-and-control-options.md)
  — control-only vs. monitor, and why the VST host is the only full-control path.
- [`audio-and-control-api.md`](audio-and-control-api.md) — the audio engine,
  command flow, and HTTP/WS/MIDI API.
- [`vst3-hosting.md`](vst3-hosting.md) — how the plugin is loaded and driven.
- [`overbridge-param-sync.md`](overbridge-param-sync.md) — host ↔ device state
  sync over the VST3 surface.
- [`../active-issues/jitter-on-param-sync.md`](../active-issues/jitter-on-param-sync.md)
  — param-sync jitter (audio-thread lock contention), related real-time concern.
