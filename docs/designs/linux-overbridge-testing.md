# Design: testing Overbridge + VSTs on Linux

How to develop and validate ob-host on Linux when Elektron only ships Overbridge for
**macOS and Windows**. This doc compares realistic paths — native fake-plugin CI, Wine,
VMs, and remote hardware — and proposes a phased test strategy.

## Problem

Today:

| Layer | macOS (production) | Linux (today) |
|-------|-------------------|---------------|
| `cargo test` + scenes/morph tests | ✓ | ✓ (CI on `ubuntu-latest`) |
| HTTP / WS / scenes API | ✓ | ✓ via `--fake-plugin` |
| Load real Elektron VST3 | ✓ | ✗ not attempted |
| Overbridge Engine + USB device | ✓ | ✗ not supported natively |
| Hardware param sync | ✓ (hidden editor + run-loop pump) | N/A without real plugin |

The product goal on Linux is **not** “replace macOS for live use” — it is **developer
confidence**: catch regressions in the host, API, and morph engine without a Mac on every
PR, and optionally exercise a **real** Overbridge plugin load when someone has the stack
available.

## Constraints (Elektron + ob-host)

### What Elektron officially supports

- **Overbridge Engine** and **Overbridge VST3** are distributed for **macOS and Windows
  only**. There is no native Linux Engine or USB driver from Elektron.
- Hardware must be in **Overbridge USB mode** for full parameter mirroring (not MIDI-only).
- The VST3 plugin is a **Windows PE** or **macOS bundle**, not a Linux `.so`.

### What ob-host assumes

- Control-only hosting: no CoreAudio path; parameters via **edit controller** + sync pump.
- **macOS:** `editor_macos.rs` opens a hidden plugin editor and pumps `NSRunLoop` — this
  is what makes Overbridge `RemoteDeviceClient` / hardware IPC reliable on a Mac.
- **Linux / other:** `ParamSyncPump` runs on the tokio run-loop tick instead; there is no
  editor window yet. Whether a Wine-hosted Overbridge plugin needs an equivalent UI pump
  is **unknown** and must be validated experimentally.
- VST3 loading on Linux is supported by vendored `truce-rack-vst3` (`Contents/x86_64-linux/*.so`
  layout, `~/.vst3`, `/usr/lib/vst3`).

### Legal / licensing

- Overbridge binaries are **proprietary**; CI must not redistribute them. Docs and scripts
  should assume the developer installs Overbridge locally (same as macOS `plugins/`).
- Running macOS in a VM for CI may violate Apple license terms unless on Apple hardware;
  treat **Linux + Wine** or a **physical Windows/macOS test machine** as the practical options.

## Where to run what (Mac vs Linux VM vs EC2)

You have a Mac today — use it for **real Overbridge + hardware** (Tier 4). Use **Linux**
only for automated tests that do not need Elektron binaries or USB.

| Environment | Best for | Overbridge VST | USB hardware | Cost |
|-------------|----------|----------------|--------------|------|
| **Your Mac (native)** | Daily dev, scenes UI, Digitakt | ✓ | ✓ | — |
| **Linux VM on Mac** (Multipass, Lima, UTM) | Tier 0 parity with CI | ✗ | ✗ | Free |
| **EC2 Ubuntu** | Tier 0 CI parity, SSH from anywhere | ✗ | ✗ | ~$0.05/hr |
| **Docker on Mac** | Quick one-off `cargo test` | ✗ | ✗ | Free |
| **Wine on Linux** (future Tier 2) | Spike Windows VST load | Maybe | ✗ | Lab time |
| **macOS VM on Mac** | Not useful for *Linux* testing | ✓ | ✓ (passthrough) | Tier 4 only |

**Recommendation:** keep the Digitakt on your Mac for real testing. Spin up **Multipass**
or **EC2** when you want to confirm Linux/CI without rebooting — both run the same
`./scripts/test.sh` as GitHub Actions.

**EC2 is not a substitute for hardware testing** — no USB to the Digitakt, no Overbridge
Engine in a useful way. **A Linux VM on your Mac is equivalent to EC2** for Tier 0; pick
whichever is easier (local VM = faster iteration, EC2 = matches CI exactly, shareable URL).

**Do not run macOS in a VM to test Linux** — that tests macOS, not Linux. Apple Silicon
Linux VMs are **aarch64**; GitHub Actions uses **x86_64** — usually fine for Tier 0 Rust
tests, but note the arch difference if you later try Wine/x86 VST spikes.

---

## Practical guide: run tests today

### On your Mac (no Linux required)

**Tier 0 — full suite** (same as CI):

```bash
cd overbridge-scenes
./scripts/test.sh
```

Expect ~30–40 Rust tests + Node morph tests. No Overbridge install needed.

**Tier 0 — live HTTP smoke** (fake plugin server):

```bash
./scripts/test-params.sh --live
# → starts ob-host on port 3848, curls /api/status and POST /api/parameters/0
```

**Tier 4 — real Digitakt** (what you use for scenes/crossfader):

```bash
./scripts/setup.sh                    # once: copy VST3s, build
./scripts/start-engine.sh
unset CARGO_TARGET_DIR                # avoid stale binary (see architecture.md)
cargo build --release
RUST_LOG=info ./target/release/ob-host --plugin Digitakt
open http://127.0.0.1:7780/scenes.html
```

---

### Option A — Linux VM on your Mac (Multipass)

[Multipass](https://multipass.run/) is the simplest Ubuntu VM on macOS (works on Apple
Silicon and Intel).

**1. Install and launch VM**

```bash
brew install multipass
multipass launch 24.04 --name ob-scenes --cpus 4 --memory 8G --disk 40G
```

**2. Mount the repo and install deps** (on the Mac host):

```bash
multipass mount "$(pwd)" ob-scenes:/home/ubuntu/overbridge-scenes
multipass exec ob-scenes -- bash -lc '
  sudo apt-get update
  sudo apt-get install -y build-essential pkg-config libasound2-dev curl jq nodejs npm
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.90.0
  source "$HOME/.cargo/env"
  cd /home/ubuntu/overbridge-scenes
  ./scripts/test.sh
'
```

**3. Optional live smoke inside the VM**

```bash
multipass exec ob-scenes -- bash -lc '
  source "$HOME/.cargo/env"
  cd /home/ubuntu/overbridge-scenes
  ./scripts/test-params.sh --live
'
```

**4. Shell into the VM for interactive work**

```bash
multipass shell ob-scenes
cd /home/ubuntu/overbridge-scenes
source ~/.cargo/env
```

**Alternatives:** [Lima](https://github.com/lima-vm/lima) (`limactl start template://ubuntu`)
for a Docker-friendly Linux VM; **UTM** if you want a full desktop. Same test commands
inside the guest.

---

### Option B — EC2 Ubuntu (matches GitHub Actions)

Good when you want a clean machine identical to CI, or to test from another network.

**1. Launch instance**

- AMI: **Ubuntu 24.04 LTS**
- Type: `t3.medium` (2 vCPU, 4 GiB) or larger for faster `cargo` builds
- Storage: 20 GiB gp3
- Security group: SSH (22) from your IP only — **no need to open 7780** for Tier 0

**2. SSH and one-shot test** (replace host and key path):

```bash
ssh -i ~/.ssh/your-key.pem ubuntu@EC2_PUBLIC_IP '
  sudo apt-get update &&
  sudo apt-get install -y build-essential pkg-config libasound2-dev curl jq git nodejs npm &&
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.90.0 &&
  source "$HOME/.cargo/env" &&
  git clone https://github.com/MartinNeifert/overbridge-scenes.git &&
  cd overbridge-scenes &&
  ./scripts/test.sh &&
  ./scripts/test-params.sh --live
'
```

**3. Persistent dev box** — clone once, pull before each run:

```bash
# on EC2 after initial clone
cd ~/overbridge-scenes && git pull && ./scripts/test.sh
```

**Cost tip:** stop the instance when idle; Tier 0 tests only need a few minutes of CPU.

---

### Option C — Docker on Mac (quick sanity check)

No VM UI; good for “does it compile and test on Linux?” in one command.

```bash
docker run --rm -v "$(pwd)":/work -w /work rust:1.90-bookworm bash -lc '
  apt-get update && apt-get install -y pkg-config libasound2-dev nodejs npm &&
  ./scripts/test.sh
'
```

Caveats: slower first run (image pull); no `multipass mount` convenience; Apple Silicon
runs aarch64 Linux inside the container (same as ARM Multipass).

---

### Option D — GitHub Actions (no local Linux)

Push a branch — workflow `.github/workflows/ci.yml` runs `./scripts/test.sh` and
`./scripts/test-params.sh --live` on `ubuntu-latest`. Use this when you only need CI
confirmation without local setup.

---

## What each test command does

| Command | Builds? | Starts server? | Needs Elektron? |
|---------|---------|----------------|-----------------|
| `./scripts/test.sh` | debug via `cargo test` | no | no |
| `./scripts/test-params.sh --live` | debug `ob-host` if missing | yes (fake, :3848) | no |
| `cargo test -- --test-threads=1` | same as test.sh Rust part | no | no |
| `node --test web/scenes-morph.test.mjs` | no | no | no |
| `ob-host --plugin Digitakt` | release (you build) | yes (:7780) | yes |

**CI deps** (already in workflow): `libasound2-dev pkg-config curl jq` + Rust **1.90.0**
(see `rust-toolchain.toml`).

---

## Future: Wine / real VST on Linux (not ready yet)

Only pursue after Tier 0 is green on your chosen Linux environment.

**Spike host:** use the Multipass VM or a dedicated x86_64 Linux machine (Wine + Windows
VST3 is painful on aarch64). On Apple Silicon Macs, prefer an **x86_64 EC2** instance
(e.g. `t3.medium`) for Wine experiments, not an ARM VM.

Rough sequence (document results in `docs/machines/wine-overbridge-spike.md` when tried):

```bash
# Ubuntu x86_64 only for Tier 2 spike
sudo dpkg --add-architecture i386
sudo apt-get update
sudo apt-get install -y wine64 wine32 winetricks

export WINEPREFIX=$HOME/.wine-overbridge
winetricks -q vcrun2019   # or whatever Overbridge installer needs

# Install Windows Overbridge into the prefix (manual GUI or silent installer)
# Symlink VST3 into repo plugins/ — see Tier 2 section below

cargo build --release
./target/release/ob-host --plugin Digitakt --no-engine
curl http://127.0.0.1:7780/api/parameters | head
```

USB + Engine (Tier 3) is out of scope for EC2 and for Mac Linux VMs — keep that on native
Mac or a physical Windows/Linux box with passthrough.

---

## Testing tiers

Think in tiers. Higher tiers cost more setup; lower tiers should stay fast and headless.

```text
Tier 0  Fake plugin + unit/integration tests     ← CI today
Tier 1  Native Linux VST3 (non-Elektron)        ← host smoke
Tier 2  Wine: load Windows Overbridge VST3      ← plugin load / param surface
Tier 3  Wine: Engine + USB + hardware           ← full stack (lab only)
Tier 4  Remote real OS (Mac/Win + device)       ← manual / scheduled QA
```

### Tier 0 — Fake plugin (implemented)

**Command:**

```bash
OB_FAKE_PLUGIN=1 ./target/release/ob-host --fake-plugin --port 3848
# or: ./scripts/test-params.sh --live
```

**Covers:** parameter round-trips, MIDI mapping, scenes HTTP API, morph math, preset-load
fingerprint behaviour via `FakePlugin` + `test_params` table.

**CI:** `.github/workflows/ci.yml` → `./scripts/test.sh` + `./scripts/test-params.sh --live`.

**Gaps:** no COM/VST3, no Overbridge IPC, no `performEdit` timing realism, no 2000+ param
scale.

**Keep investing here:** expand contract tests when macOS finds new Overbridge quirks.

### Tier 1 — Native Linux VST3 (any vendor)

**Goal:** prove `truce-rack-vst3` scan/load on Linux independently of Elektron.

**Setup:**

```bash
# Example: ship a tiny open-source VST3 in vendor/ or use a free Linux .vst3
mkdir -p ~/.vst3
# copy SomePlugin.vst3 → ~/.vst3/

cargo build --release
./target/release/ob-host --plugin SomePlugin --plugin-dir ~/.vst3 --no-engine
```

**Success criteria:**

- `--list-plugins` finds the bundle.
- Plugin loads without segfault.
- `GET /api/parameters` returns a non-empty list.
- `POST /api/parameters/{index}` updates cached values.

**Value:** isolates host/API bugs from Wine/Overbridge. Recommended before any Wine work.

**CI option:** add a **single** open-source Linux VST3 (e.g. small GPL-friendly effect) as
a gitignored or submodule test fixture; gate behind `OB_LINUX_VST_SMOKE=1` so default CI
stays fast.

### Tier 2 — Wine + Windows Overbridge VST3 (no hardware)

**Goal:** load Elektron’s Windows VST3 inside ob-host on Linux and exercise the parameter
API **without** USB.

**Prerequisites (lab machine):**

- Wine **staging** (8.x+) or distro equivalent; winetricks VC++ runtimes the plugin needs.
- Windows Overbridge installed **inside the prefix** (or copied VST3 + minimal Engine
  files — exact layout TBD by experiment).
- VST3 path visible to the host, e.g. symlink into `plugins/`:

  ```text
  plugins/Digitakt.vst3/   → wine drive_c/.../Common Files/VST3/Digitakt.vst3/
  ```

**Launch pattern:**

```bash
export WINEPREFIX=~/.wine-overbridge
# Optional: WINEDEBUG=-all to reduce noise

cargo build --release
./target/release/ob-host \
  --plugin Digitakt \
  --plugin-dir plugins \
  --no-engine    # until Engine strategy is clear
```

**Open questions (spike checklist):**

| # | Question | Pass signal |
|---|----------|-------------|
| 1 | Does `LoadedModule::open` work on the Windows PE inside Wine? | Plugin scan lists Digitakt |
| 2 | Does `createInstance` / factory init crash or hang? | Host reaches “Plugin exposes N parameters” |
| 3 | Does `setParamNormalized` work without Engine? | API set/get round-trip on a few params |
| 4 | Is a hidden editor / message loop required on Wine? | Compare with/without future Linux editor pump |
| 5 | Does `getState` / preset fingerprint behave like macOS? | Toggle preset in standalone Overbridge app |

**Tooling alternatives:**

| Approach | Pros | Cons |
|----------|------|------|
| **Plain Wine + ob-host** | Direct; matches how we host on Mac | No bridge; PE load may fail; no JUCE/UI assumptions tested |
| **yabridge** | Mature VST2/VST3 bridge for Linux DAWs | Extra layer; ob-host would host **yabridge’s** plugin, not Elektron’s directly — good for DAW parity, awkward for *our* host |
| **Wine + standalone Overbridge app** | Validates Engine install | Does not test ob-host integration |

**Recommendation:** spike **plain Wine first** (simplest alignment with ob-host architecture).
Treat yabridge as a **reference** for what Wine fixes, not as the primary integration, unless
we decide to host through a bridge DLL.

### Tier 3 — Wine + Overbridge Engine + USB hardware

**Goal:** full host ↔ device sync on Linux — the same end state as macOS control-only mode.

**Why this is hard:**

```text
  ob-host (Linux, ELF)
       → loads Digitakt.vst3 (Windows PE via Wine)
       → plugin talks to Overbridge Engine (Windows exe via Wine?)
       → Engine ↔ USB (WinUSB / proprietary driver)
       → Digitakt hardware
```

Each arrow is a separate risk:

1. **Engine in same prefix** — may need to `wine "Overbridge Engine.exe"` before starting
   ob-host; port `scripts/start-engine.sh` to a Wine-aware launcher.
2. **USB passthrough** — Wine does not magically forward USB. Options:
   - **USB/IP** or **qemu usb-host** if the machine is a VM.
   - **Physical Linux box + usbip** to a Windows VM running Engine (split brain — avoid if possible).
   - **Dual-boot Windows** on the same machine (Tier 4 variant).
3. **Single prefix vs two** — plugin and Engine must agree on IPC sockets (`127.0.0.1:46000` …).
4. **Editor / JUCE timers** — macOS relies on `NSRunLoop`; Wine may need a visible editor or
   a different pump (spike: `OB_OPEN_EDITOR=1` equivalent on Wine).

**Success criteria (manual test script):**

1. Engine reports device connected in Wine Overbridge UI.
2. ob-host loads plugin, `GET /api/selector` shows linked device.
3. UI slider moves a parameter; hardware knob reflects change.
4. Hardware knob moves; WebSocket pushes update to `/scenes.html`.

**Non-goals for v1:** low-latency audio, multi-client Engine, hot-unplug reliability.

### Tier 4 — Real OS + device (Mac or Windows)

When Wine fails or for release QA, use a **dedicated test host**:

| Setup | Notes |
|-------|--------|
| **Mac + Digitakt** | Current reference platform; control-only ob-host + DAW audio in parallel |
| **Windows PC + device** | Native Overbridge; ob-host port would need Windows build (not in repo today) |
| **Self-hosted GitHub runner** | Mac mini with device attached; nightly workflow, not per-PR |
| **Remote desktop to lab Mac** | Developers without hardware |

Document manual checklist in `docs/machines/` per device once a Linux/Wine path is understood.

## Proposed roadmap

### Phase A — Document and stabilize Tier 0 (done)

- Linux CI: `cargo test`, morph tests, fake-plugin live smoke.
- No Elektron binaries in repo.

### Phase B — Tier 1 native VST3 smoke (low effort)

- [x] Design doc: `linux-overbridge-testing.md` (this file).
- [ ] Optional `scripts/linux-vst-smoke.sh` + env gate `OB_LINUX_VST_SMOKE`.
- [ ] Pick one tiny Linux VST3 for repeatable load test.

### Phase C — Tier 2 Wine spike (time-boxed)

- [ ] `docs/machines/wine-overbridge-spike.md` with results table (pass/fail per checklist row).
- [ ] `scripts/wine-overbridge-setup.sh` (local only, gitignored prefix path).
- [ ] Note Wine version, Overbridge version, plugin param count if load succeeds.

### Phase D — Editor pump on non-macOS (if Tier 2 loads)

- [ ] If hardware sync fails without UI: add minimal editor open on Linux (X11/Wayland via
  `baseview`, already a dependency on macOS) or document Wine-only editor requirement.
- [ ] Reuse `ParamSyncPump` for fingerprint/preset detection either way.

### Phase E — Tier 3 USB (optional, lab)

- Only after Tier 2 param API works.
- Single maintainer machine; results feed back into `docs/machines/`.

## CI recommendations

| Job | Runner | What |
|-----|--------|------|
| `test-linux` (existing) | `ubuntu-latest` | Tier 0 only |
| `linux-vst-smoke` (future, optional) | `ubuntu-latest` | Tier 1 with OSS plugin; `OB_LINUX_VST_SMOKE=1` |
| `overbridge-wine` (future, manual) | `workflow_dispatch` | Tier 2/3 on self-hosted runner with Wine + license-installed Overbridge |

Do **not** add Wine to default PR CI until Tier 2 is reproducible in under ~10 minutes.

## Developer quick reference

```bash
# Mac — full automated suite (no Elektron)
./scripts/test.sh
./scripts/test-params.sh --live

# Mac — real hardware
./scripts/start-engine.sh
cargo build --release && ./target/release/ob-host --plugin Digitakt

# Linux VM / EC2 — same as CI
sudo apt-get install -y build-essential pkg-config libasound2-dev curl jq nodejs npm
# install rust 1.90.0, then:
./scripts/test.sh && ./scripts/test-params.sh --live
```

## Related docs

- [`../architecture.md`](../architecture.md) — CLI, control-only hosting.
- [`overbridge-param-sync.md`](overbridge-param-sync.md) — what must work for hardware sync.
- [`vst3-hosting.md`](vst3-hosting.md) — how plugins are loaded.
- [`../hardware-sync.md`](../hardware-sync.md) — IPC overview.
- [`../../.github/workflows/ci.yml`](../../.github/workflows/ci.yml) — current Linux CI.

## Open decisions

1. **Windows ob-host** — native Windows build may be simpler than Wine for Elektron QA;
   out of scope unless someone needs it.
2. **yabridge vs direct Wine** — decide after Phase C spike.
3. **Whether Linux is a supported *runtime* platform** — recommend **dev/CI only** until Tier 3
   passes on more than one machine.
