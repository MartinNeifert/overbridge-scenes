# Design: audio routing and programmatic control options

Why ob-host is control-only, how that relates to keeping your musical signal
"in the box" (on the hardware), and why hosting the **VST/AU plugin in a host you
control (ob-host)** is the only viable option for full programmatic control given
Elektron's feature constraints.

## Two different "audio" paths — keep them separate

There are two distinct audio concerns, and conflating them causes confusion:

1. **Program audio** — your actual musical signal through the device.
2. **ob-host's role** — parameter/MIDI control only; it does not host audio I/O.

### 1. Program audio can stay in the box

Overbridge USB mode does **not** require your musical signal to travel through
ob-host. You can keep the program path fully analog:

```
source → device analog IN → device analog OUT → mixer / interface
```

In Overbridge mode, USB is used for parameter/preset mirroring, optional multi-channel
digital I/O, and the Engine ↔ plugin link — not necessarily your mix. So the device keeps
processing sound on its own hardware path; you do **not** have to insert the plugin on a
DAW track or loop your mix through ob-host for the box to make sound.

Requirements remain: Overbridge Engine running, device in Overbridge USB mode, and (for
ob-host) the plugin loaded.

### 2. ob-host never opens the device audio path

The plugin must be **loaded** and its editor / device IPC must be **running**
(the hidden editor + main run-loop pump). Parameter writes reach the hardware
through the **edit controller** (the same path as the plugin GUI) — no CoreAudio
device, no `process()`, no USB-return override.

| Concern | ob-host behavior |
|---------|------------------|
| **Control** | VST3 plugin + Engine carry parameter and MIDI changes to the box |
| **Device audio** | Untouched — analog Main Out keeps the hardware's own mix |
| **Alongside a DAW** | Ableton (etc.) can use Overbridge USB audio while ob-host handles scenes/crossfader |

This is the recommended setup: the DAW owns audio; ob-host owns scenes and the
crossfader without fighting over the USB return.

## Programmatic control options (and why most are dead ends)

Goal: drive device parameters/presets from code. The candidate surfaces:

| Route | Programmatic control? | Notes |
|-------|----------------------|-------|
| **VST/AU plugin in a host you control (ob-host)** | ✅ **Yes — the only full option** | You own the host, so you read/write VST3 params, presets, and route MIDI. Requires loading the plugin + Engine. |
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
you control (ob-host) is the only path to full programmatic, bidirectional control.**

## Related docs

- [`overbridge-param-sync.md`](overbridge-param-sync.md) — how host ↔ device state sync
  works over the VST3 surface.
- [`vst3-hosting.md`](vst3-hosting.md) — how the plugin is loaded and driven.
- [`../active-issues/jitter-on-param-sync.md`](../active-issues/jitter-on-param-sync.md) —
  jitter on the param-sync path.
