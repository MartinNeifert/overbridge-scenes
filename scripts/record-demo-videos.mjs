#!/usr/bin/env node
/**
 * Record short feature demo clips for the README using the fake-plugin host.
 * Writes MP4 (source) and GIF (README embed — GitHub strips <video> tags).
 *
 * Prerequisites:
 *   OB_FAKE_PLUGIN=1 ./target/release/ob-host --fake-plugin --port 7780
 *
 * Usage:
 *   node scripts/record-demo-videos.mjs
 *   OB_DEMO_URL=http://127.0.0.1:7780 node scripts/record-demo-videos.mjs
 */
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdir, rename, unlink } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(__dirname, "..");
const OUT_DIR = path.join(ROOT, "docs", "videos");
const TMP_DIR = path.join(OUT_DIR, ".tmp");
const BASE_URL = process.env.OB_DEMO_URL || "http://127.0.0.1:7780";
const PLUGIN = "OB Test Host";
const PATTERN = "A01";

async function waitForServer() {
  for (let i = 0; i < 60; i++) {
    try {
      const res = await fetch(`${BASE_URL}/api/status`);
      if (res.ok) return;
    } catch {
      /* retry */
    }
    await sleep(200);
  }
  throw new Error(`Server not reachable at ${BASE_URL}`);
}

async function seedDemoData(crossfader) {
  const payload = {
    scenes: [
      {
        id: "1",
        name: "Dark",
        params: [
          { index: 0, id: 1, name: "Filter Cutoff", value: 0.12 },
          { index: 1, id: 2, name: "Filter Reso", value: 0.65 },
          { index: 2, id: 3, name: "Drive", value: 0.35 },
        ],
      },
      {
        id: "2",
        name: "Bright",
        params: [
          { index: 0, id: 1, name: "Filter Cutoff", value: 0.88 },
          { index: 1, id: 2, name: "Filter Reso", value: 0.18 },
          { index: 2, id: 3, name: "Drive", value: 0.08 },
        ],
      },
      {
        id: "3",
        name: "Crunch",
        params: [
          { index: 0, id: 1, name: "Filter Cutoff", value: 0.45 },
          { index: 2, id: 3, name: "Drive", value: 0.92 },
        ],
      },
      {
        id: "4",
        name: "Wide",
        params: [
          { index: 0, id: 1, name: "Filter Cutoff", value: 0.72 },
          { index: 1, id: 2, name: "Filter Reso", value: 0.05 },
        ],
      },
    ],
    crossfader,
    baseline: {
      explicit: true,
      values: [
        { index: 0, id: 1, value: 0.5 },
        { index: 1, id: 2, value: 0.25 },
        { index: 2, id: 3, value: 0.0 },
      ],
    },
  };

  const res = await fetch(
    `${BASE_URL}/api/scenes/${encodeURIComponent(PLUGIN)}/${PATTERN}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    },
  );
  if (!res.ok) {
    throw new Error(`Failed to seed scenes: ${res.status} ${await res.text()}`);
  }
}

const AB_CROSSFADER = { mode: "ab", a: "1", b: "2", pos: 0 };

const QUAD_CROSSFADER = {
  mode: "quad",
  corners: { tl: "1", tr: "2", bl: "3", br: "4" },
  x: 0.5,
  y: 0.5,
};

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

async function animateRange(page, selector, from, to, durationMs) {
  await page.evaluate(
    async ({ selector, from, to, durationMs }) => {
      const el = document.querySelector(selector);
      if (!el) throw new Error(`Missing ${selector}`);
      const steps = Math.max(30, Math.round(durationMs / 16));
      const delay = durationMs / steps;
      for (let i = 0; i <= steps; i++) {
        const t = i / steps;
        const eased = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
        const value = Math.round(from + (to - from) * eased);
        el.value = String(value);
        el.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((r) => setTimeout(r, delay));
      }
    },
    { selector, from, to, durationMs },
  );
}

/** Drag the 2D crossfader pad; x/y are normalized 0..1 within the pad. */
async function dragPad(page, padSelector, from, to, durationMs) {
  const pad = page.locator(padSelector);
  const box = await pad.boundingBox();
  if (!box) throw new Error(`Pad not visible: ${padSelector}`);

  const px = (x) => box.x + box.width * x;
  const py = (y) => box.y + box.height * y;

  await page.mouse.move(px(from.x), py(from.y));
  await page.mouse.down();

  const steps = Math.max(24, Math.round(durationMs / 16));
  const stepDelay = durationMs / steps;
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    const eased = t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
    const x = from.x + (to.x - from.x) * eased;
    const y = from.y + (to.y - from.y) * eased;
    await page.mouse.move(px(x), py(y));
    await sleep(stepDelay);
  }

  await page.mouse.up();
}

async function convertToMp4(webmPath, mp4Path) {
  await new Promise((resolve, reject) => {
    const proc = spawn(
      "ffmpeg",
      [
        "-y",
        "-i",
        webmPath,
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        "23",
        "-preset",
        "medium",
        "-movflags",
        "+faststart",
        "-an",
        mp4Path,
      ],
      { stdio: "inherit" },
    );
    proc.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}`)),
    );
  });
  await unlink(webmPath);
}

/** README-friendly GIF (GitHub renders these inline; <video> tags do not). */
async function convertToGif(mp4Path, gifPath, { width }) {
  const scale = `scale=${width}:-1:flags=lanczos`;
  await new Promise((resolve, reject) => {
    const proc = spawn(
      "ffmpeg",
      [
        "-y",
        "-i",
        mp4Path,
        "-vf",
        `fps=12,${scale},split[s0][s1];[s0]palettegen=max_colors=128:stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3`,
        gifPath,
      ],
      { stdio: "inherit" },
    );
    proc.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}`)),
    );
  });
}

async function recordVideo(name, { viewport, setup, action, gifWidth }) {
  await mkdir(TMP_DIR, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport,
    recordVideo: { dir: TMP_DIR, size: viewport },
    colorScheme: "dark",
    deviceScaleFactor: 1,
  });
  const page = await context.newPage();
  await setup(page);
  await sleep(600);
  await action(page);
  await sleep(700);
  const video = page.video();
  await context.close();
  await browser.close();

  const webmPath = await video.path();
  const mp4Path = path.join(OUT_DIR, `${name}.mp4`);
  const gifPath = path.join(OUT_DIR, `${name}.gif`);
  await convertToMp4(webmPath, mp4Path);
  await convertToGif(mp4Path, gifPath, { width: gifWidth ?? viewport.width });
  console.log(`✓ ${mp4Path}`);
  console.log(`✓ ${gifPath}`);
  return { mp4Path, gifPath };
}

async function recordScenesDemo() {
  return recordVideo("scenes-crossfader", {
    viewport: { width: 1280, height: 820 },
    gifWidth: 960,
    setup: async (page) => {
      await page.goto(`${BASE_URL}/scenes.html`, { waitUntil: "networkidle" });
      await page.waitForSelector("#sc-crossfader");
      await page.waitForFunction(
        () =>
          document.querySelector("#sc-assign-a")?.value === "1" &&
          document.querySelector("#sc-assign-b")?.value === "2",
        { timeout: 10000 },
      );
    },
    action: async (page) => {
      await animateRange(page, "#sc-crossfader", 0, 1000, 2800);
      await sleep(400);
      await animateRange(page, "#sc-crossfader", 1000, 0, 2400);
      await sleep(300);
      await page.click("#sc-jump-b");
      await sleep(500);
      await page.click("#sc-jump-a");
    },
  });
}

async function recordParametersDemo() {
  return recordVideo("classic-parameters", {
    viewport: { width: 1280, height: 820 },
    gifWidth: 960,
    setup: async (page) => {
      await page.goto(`${BASE_URL}/parameters.html`, {
        waitUntil: "networkidle",
      });
      await page.waitForSelector("#parameters .param-card", { timeout: 10000 });
    },
    action: async (page) => {
      const search = page.locator("#search");
      await search.click();
      await search.fill("Filter");
      await sleep(800);
      const pin = page.locator("#parameters .param-card .pin-btn").first();
      await pin.click();
      await sleep(500);
      const slider = page.locator("#pinned-controls input[type='range']").first();
      await slider.focus();
      await animateRange(page, "#pinned-controls input[type='range']", 0, 850, 1800);
      await sleep(400);
      await animateRange(page, "#pinned-controls input[type='range']", 850, 200, 1600);
    },
  });
}

async function recordQuadFaderDemo() {
  return recordVideo("scenes-quad-fader", {
    viewport: { width: 1280, height: 820 },
    gifWidth: 960,
    setup: async (page) => {
      await seedDemoData(QUAD_CROSSFADER);
      await page.goto(`${BASE_URL}/scenes.html`, { waitUntil: "networkidle" });
      await page.waitForSelector("#sc-xf-pad");
      await page.waitForFunction(
        () => !document.getElementById("sc-xf-quad")?.classList.contains("hidden"),
        { timeout: 10000 },
      );
      await page.waitForFunction(
        () => document.getElementById("sc-xf-mode")?.value === "quad",
        { timeout: 10000 },
      );
    },
    action: async (page) => {
      const center = { x: 0.5, y: 0.5 };
      await dragPad(page, "#sc-xf-pad", center, { x: 0.08, y: 0.08 }, 1400);
      await sleep(350);
      await dragPad(page, "#sc-xf-pad", { x: 0.08, y: 0.08 }, { x: 0.92, y: 0.08 }, 1200);
      await sleep(350);
      await dragPad(page, "#sc-xf-pad", { x: 0.92, y: 0.08 }, { x: 0.92, y: 0.92 }, 1200);
      await sleep(350);
      await dragPad(page, "#sc-xf-pad", { x: 0.92, y: 0.92 }, { x: 0.08, y: 0.92 }, 1200);
      await sleep(350);
      await dragPad(page, "#sc-xf-pad", { x: 0.08, y: 0.92 }, center, 1400);
    },
  });
}

async function recordRemoteDemo() {
  return recordVideo("remote-crossfader", {
    viewport: { width: 390, height: 844 },
    setup: async (page) => {
      await seedDemoData(AB_CROSSFADER);
      await page.goto(`${BASE_URL}/remote.html?pattern=${PATTERN}`, {
        waitUntil: "networkidle",
      });
      await page.waitForSelector("#remote-slider");
      await page.waitForSelector("#remote-name-a:not(:empty)", { timeout: 10000 });
    },
    action: async (page) => {
      await animateRange(page, "#remote-slider", 0, 1000, 2600);
      await sleep(400);
      await animateRange(page, "#remote-slider", 1000, 500, 1400);
      await sleep(300);
      await page.click("#remote-jump-a");
      await sleep(400);
      await page.click("#remote-jump-b");
    },
  });
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  await waitForServer();
  await seedDemoData(AB_CROSSFADER);
  console.log("Seeded demo scenes for", PLUGIN, PATTERN);
  await recordScenesDemo();
  await recordQuadFaderDemo();
  await recordParametersDemo();
  await recordRemoteDemo();
  console.log("Done — MP4 + GIF demos in docs/videos/");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
