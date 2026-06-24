# Design: audio routing and programmatic control options

Why ob-host runs its own audio loop, how that relates to keeping your musical signal
"in the box" (on the hardware), and why hosting the **VST/AU plugin in a host you
control (ob-host)** is the only viable option for full programmatic control given
Elektron's feature constraints.

## Two different "audio" paths — keep them separate

There are two distinct audio concerns, and conflating them causes confusion:

1. **Program audio** — your actual musical signal through the device.
2. **ob-host's control-plane audio loop** — the duplex `process()` graph ob-host runs to
   host the plugin.

### 1. Program audio can stay in the box

Overbridge USB mode does **not** require your musical signal to travel through the Mac.
You can keep the program path fully analog:

```
source → device analog IN → device analog OUT → mixer / interface
```

In Overbridge mode, USB is used for parameter/preset mirroring, optional multi-channel
digital I/O, and the Engine ↔ plugin link — not necessarily your mix. So the device keeps
processing sound on its own hardware path; you do **not** have to insert the plugin on a
DAW track or loop your mix through the host for the box to make sound.

Requirements remain: Overbridge Engine running, device in Overbridge USB mode, and (for
ob-host) the plugin loaded.

### 2. Opening the device is optional

The plugin must be **loaded** and its editor / device IPC must be **running**
(the hidden editor + main run-loop pump). How parameter writes reach the hardware
depends on the audio mode:

| Mode | How control runs | Effect on the device's audio |
|------|------------------|------------------------------|
| **Control-only** (default: `control_only: true`, `duplex.enabled: false` in `config/default.json`) | No audio device opened, no `process()`. Parameters delivered via the **edit controller** (same path as the plugin GUI), driven by the run-loop pump. | **Untouched** — the host never writes to the USB return; analog Main Out keeps the hardware's own mix. A DAW can use Overbridge USB audio in parallel. |
| **Duplex + monitor** (opt-in: `--duplex` or `duplex.enabled: true`) | A single CoreAudio duplex AUHAL on the Elektron device drives `process()` from one render callback; device input is monitored back to the USB return | Analog Main Out plays the **USB return**. Monitoring copies the device's own input (Main L/R by default) back so you still hear the internal mix |
| **Legacy monitor** (`--audio`) | A cpal stream on the Overbridge CoreAudio device drives `process()` | The plugin output is streamed to the device's USB return, which **overrides** the hardware's own audio (often silence) |

This is why streaming audio to the device made the Digitakt go silent in early
host builds: the host wrote the plugin's (silent) output bus to the USB-return
channels, and the device played that instead of its internal mix.

**Control-only is the default** — the host avoids opening the Elektron device
entirely, so the hardware keeps making sound on its own path while scenes,
crossfader, and API control still work. This is the right setup when a DAW
already owns Overbridge audio. It also sidesteps the audio-loop lock contention
behind [param-sync jitter](../active-issues/jitter-on-param-sync.md).

Use duplex only when you explicitly want ob-host to open the device audio path
*and* monitor through the USB return.

> **Mode precedence:** `--duplex` or `duplex.enabled: true` overrides
> control-only. Startup logs confirm the mode:
>
> - Control-only (default): `Control-only mode: audio engine not engaged` (no `CoreAudio duplex` line)
> - Duplex: `Duplex mode: single AUHAL on "Digitakt" … monitor on`

Use `--audio` only for the legacy cpal monitor path (not recommended).

## Programmatic control options (and why most are dead ends)

Goal: drive device parameters/presets from code. The candidate surfaces:

| Route | Programmatic control? | Notes |
|-------|----------------------|-------|
| **VST/AU plugin in a host you control (ob-host)** | ✅ **Yes — the only full option** | You own the host, so you read/write VST3 params, presets, and route MIDI. Requires the control-plane audio loop. |
| **Overbridge standalone app** | ❌ Effectively no | It is just another *client* of the Engine. No public API/CLI/SDK; UI automation (AppleScript/Accessibility) is fragile and can't address the 2000+ params. Gives *less* than the plugin route, not more. |
| **MIDI (CC/NRPN/SysEx) to the device** | ⚠️ Partial | No Overbridge needed and no audio loop, but limited to what the device maps to MIDI, and no bidirectional full-param/preset mirroring. |
| **Direct Engine IPC (`127.0.0.1:46000` …)** | ⚠️ Only via reverse engineering | Undocumented, proprietary (`ElektronIpcMessages`, `RemoteIpcMessageConnection`), brittle across firmware/Overbridge versions. Out of scope. |

### Why the plugin host is the only viable full-control option

- **Elektron publishes no Overbridge API.** There is no documented socket protocol, CLI,
  SDK, or scripting interface for the Engine or the standalone app.
- **The standalone app exposes no host boundary you own.** With the plugin, *you* are the
  host, which is the entire basis of ob-host: the VST3 host API lets you read/write every
  exposed parameter and observe `performEdit` / state changes. The standalone app is the
  host and offers nothing to hook into.
- **MIDI is a reduced surface.** It bypasses Overbridge entirely but can't mirror full
  device state or presets bidirectionally.
- **Direct IPC is undocumented.** Talking to the Engine on its localhost ports would mean
  reverse-engineering a proprietary protocol with no stability guarantees.

Net: given the feature constraints Elektron imposes, **hosting the VST/AU plugin in a host
you control (ob-host) is the only path to full programmatic, bidirectional control.** That
is precisely why ob-host exists and why it accepts the cost of running its own audio loop.

## Related docs

- [`overbridge-param-sync.md`](overbridge-param-sync.md) — how host ↔ device state sync
  works over the VST3 surface.
- [`audio-and-control-api.md`](audio-and-control-api.md) — the duplex audio engine,
  command flow, and HTTP/WS/MIDI API.
- [`vst3-hosting.md`](vst3-hosting.md) — how the plugin is loaded and driven.
- [`../active-issues/jitter-on-param-sync.md`](../active-issues/jitter-on-param-sync.md) —
  jitter partly attributable to the control-plane audio loop + scan contention.
