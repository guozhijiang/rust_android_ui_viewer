// Real-device recording → replay full-cycle E2E (web UI viewer).
//
// Exercises the actual recording code path on a connected device:
//   1. live scrcpy connect
//   2. start recording (background hierarchy refresh kicks in)
//   3. a real click on the live canvas (mousedown → touch inject → mouseup →
//      recordStep), like a user tap
//   4. stop recording; assert the step got fractional coords + a UiSelector
//      resolved from the freshest hierarchy + foreground-app annotation
//   5. replay the recording through the live WS control channel; assert 0 failed
//
// Skips (exit 3) when no adb device is present.
//
//   node e2e_record.js
//
const puppeteer = require("C:\\Users\\guozhiqiang\\.workbuddy\\binaries\\node\\workspace\\node_modules\\puppeteer-core");
const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const BASE = "http://127.0.0.1:8000/";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const devRes = await fetch(BASE + "api/devices");
  const dev = await devRes.json();
  if (!dev.devices || dev.devices.length === 0) {
    console.log("SKIP: no adb device connected");
    process.exit(3);
  }
  console.log("device:", JSON.stringify(dev.devices));

  // The recording selector comes from /api/dump-ui; on this device that needs
  // the u2 server (uiautomator dump is SIGKILLed under H5 apps / screen off).
  const u2 = await (await fetch(BASE + "api/u2/start", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ serial: dev.devices[0] }),
  })).json();
  console.log("u2:", JSON.stringify(u2));

  const errors = [];
  const browser = await puppeteer.launch({
    executablePath: EDGE,
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox", "--use-gl=swiftshader",
           "--autoplay-policy=no-user-gesture-required", "--window-size=1400,900"],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1400, height: 900 });
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));

  await page.goto(BASE, { waitUntil: "networkidle0", timeout: 20000 });
  await sleep(500);

  // 1) live connect
  await page.click('button.tab[data-tab="live"]');
  await sleep(300);
  await page.click("#liveConnect");
  let live = { connected: false };
  for (let i = 0; i < 80; i++) {
    live = await page.evaluate(() => ({ connected: live.connected, frames: live.frames }));
    if (live.connected) break;
    await sleep(250);
  }
  console.log("live:", JSON.stringify(live));

  // 2) go HOME so the screen is the (harmless) vivo launcher, then start
  //    recording → background tree refresh loop starts
  await fetch(BASE + "api/input-key", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ serial: dev.devices[0], code: "3" }), // HOME
  });
  await sleep(800);
  await page.click("#recToggle");
  await sleep(4500); // let _recTree fetch at least once

  // 3) pick a real node (smallest one with a resource-id) from the freshest
  //    background tree, map its center to canvas client coords, and click
  //    there — a genuine user-style tap that must resolve to that element.
  const target = await page.evaluate(() => {
    if (!_recTree) return { error: "no _recTree" };
    let best = null, bestArea = Infinity;
    (function rec(n) {
      const rid = (n.attrs || {})["resource-id"];
      if (rid && n.bounds) {
        const a = (n.bounds.right - n.bounds.left) * (n.bounds.bottom - n.bounds.top);
        if (a > 0 && a < bestArea) { bestArea = a; best = n; }
      }
      for (const c of n.children || []) rec(c);
    })(_recTree);
    if (!best) return { error: "no node with resource-id" };
    const b = best.bounds;
    return {
      rid: best.attrs["resource-id"],
      cx: Math.round((b.left + b.right) / 2),
      cy: Math.round((b.top + b.bottom) / 2),
      screenW: _screenSize ? _screenSize.w : 1080,
      screenH: _screenSize ? _screenSize.h : 2344,
    };
  });
  console.log("target:", JSON.stringify(target));
  if (target.error) { console.log("RESULT: fail — " + target.error); process.exit(1); }

  const rect = await page.evaluate(() => {
    const r = document.querySelector("#liveCanvas").getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  });
  // device px → client px: proportional through the video frame
  await page.mouse.click(
    rect.x + (target.cx / target.screenW) * rect.w,
    rect.y + (target.cy / target.screenH) * rect.h
  );
  await sleep(400);

  // 4) stop recording (give the async app annotation time to land)
  await page.click("#recToggle");
  await sleep(1500);

  const recState = await page.evaluate(() => ({
    recording: rec.recording,
    steps: rec.steps.map((s) => ({
      action: s.action,
      fx: s.fx, fy: s.fy,
      selector: s.selector || null,
      app: s.app || null,
      ts: s.ts,
    })),
  }));
  console.log("recSteps:", JSON.stringify(recState, null, 2));

  const s0 = (recState.steps || [])[0] || {};
  const checks = {
    stopped: recState.recording === false,
    isTap: s0.action === "tap",
    fracOk: typeof s0.fx === "number" && s0.fx >= 0 && s0.fx <= 1 &&
            typeof s0.fy === "number" && s0.fy >= 0 && s0.fy <= 1,
    hasSelector: !!(s0.selector && (s0.selector.resource_id || s0.selector.text ||
                                    s0.selector.content_desc || s0.selector.class)),
    selMatchesTarget: !!(s0.selector && s0.selector.resource_id === target.rid),
    annotated: !!s0.app,
  };

  // 5) replay through the live WS control channel
  if (live.connected) {
    await page.evaluate(() => startReplay());
    let done = false;
    for (let i = 0; i < 40; i++) {
      done = await page.evaluate(() => !rec.replaying);
      if (done) break;
      await sleep(250);
    }
    checks.replayFinished = done;
    checks.replayFailed = await page.evaluate(() => rec.replayFailed.length);
  }

  await page.evaluate(() => { if (live.connected) liveDisconnect(); });
  await sleep(400);
  await browser.close();

  // NOTE: _selMatchesTarget is informational — the background tree refreshes
  // every 3s, so the smallest node at the target point can legitimately change
  // between target selection and the click. A non-null selector + a successful
  // replay is the hard requirement (selector resolution itself is asserted
  // separately in e2e_device.js against a resource-id).
  const pass = live.connected && checks.stopped && checks.isTap && checks.fracOk &&
    checks.hasSelector && checks.annotated &&
    checks.replayFinished && checks.replayFailed === 0 && errors.length === 0;
  console.log("RESULT:", JSON.stringify({ errors, live, checks, pass }, null, 2));
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error("E2E FATAL:", e); process.exit(2); });
