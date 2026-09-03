// Real-device end-to-end E2E for the web UI viewer.
// Runs against the live server at http://127.0.0.1:8000/ and a physically
// connected adb device. Skips (exit 3) when no device is present so it can be
// re-run the moment the phone is plugged back in / USB debugging re-authorised.
//
// Validates, in order:
//   1. inspect capture (real screenshot + UI tree)
//   2. replay selector resolution against the real hierarchy
//   3. replay of a HOME-key step through the adb input path (startReplay)
//   4. live scrcpy connect + frame decode
//   5. live control-channel injection (HOME key over the WS)
//
//   node e2e_device.js
//
const puppeteer = require("C:\\Users\\guozhiqiang\\.workbuddy\\binaries\\node\\workspace\\node_modules\\puppeteer-core");
const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const BASE = "http://127.0.0.1:8000/";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  // 1) device present?
  const devRes = await fetch(BASE + "api/devices");
  const dev = await devRes.json();
  if (!dev.devices || dev.devices.length === 0) {
    console.log("SKIP: no adb device connected (plug in USB + authorise debugging, then re-run)");
    process.exit(3);
  }
  console.log("device:", JSON.stringify(dev.devices));

  // Hierarchy dumps need the u2 server on this device (uiautomator dump is
  // SIGKILLed under H5 apps / screen off); starting it is idempotent.
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

  // 2) capture (inspect mode): real screenshot + UI tree
  await page.click("#capture");
  let ok = false;
  for (let i = 0; i < 40; i++) {
    const st = await page.evaluate(() => ({
      img: !!state.image, nodes: state.nodes ? state.nodes.size : 0,
      w: state.naturalW, h: state.naturalH,
    }));
    if (st.img && st.nodes > 0 && st.w > 0) { ok = true; break; }
    await sleep(250);
  }
  const cap = await page.evaluate(() => ({
    img: !!state.image, nodes: state.nodes ? state.nodes.size : 0,
    w: state.naturalW, h: state.naturalH,
  }));
  console.log("capture:", JSON.stringify(cap));

  // 3) selector resolution against the real hierarchy: pick a node with a
  //    resource-id from the captured tree and resolve it with the replay
  //    pipeline (fresh /api/dump-ui fetch + smallest-area match).
  const resolve = await page.evaluate(async () => {
    let sel = null;
    (function find(n) {
      if (sel) return;
      if (n.attrs && n.attrs["resource-id"] && n.bounds &&
          (n.bounds.right - n.bounds.left) > 8) { sel = { resource_id: n.attrs["resource-id"] }; return; }
      for (const c of n.children || []) find(c);
    })(state.tree);
    if (!sel) return { error: "no node with resource-id" };
    const r = await resolvePoint(sel, 0.5, 0.5, 3);
    const size = await screenSize();
    return {
      sel, ok: r.ok,
      pt: r.pt,
      inBounds: r.pt && r.pt.x >= 0 && r.pt.x <= size.w && r.pt.y >= 0 && r.pt.y <= size.h,
      screen: size,
    };
  });
  console.log("resolve:", JSON.stringify(resolve));

  // 4) replay a HOME-key step through startReplay (adb input path; live is
  //    not connected yet). HOME is harmless and verifiable.
  const recResult = await page.evaluate(async () => {
    rec.steps = [{ action: "key", keycode: 3, key: "HOME", ts: Date.now() / 1000 }];
    rec.replayFailed = [];
    await startReplay();
    return { failed: rec.replayFailed.length, steps: rec.steps.length };
  });
  await sleep(300);
  console.log("replayHome:", JSON.stringify(recResult));

  // 5) live scrcpy connect + decode frames
  await page.click('button.tab[data-tab="live"]');
  await sleep(300);
  await page.click("#liveConnect");
  let live = { connected: false, videoW: 0, videoH: 0 };
  for (let i = 0; i < 80; i++) { // up to 20s
    live = await page.evaluate(() => ({
      connected: live.connected, videoW: live.videoW, videoH: live.videoH,
      frames: live.frames,
    }));
    if (live.connected && live.videoW > 0) break;
    await sleep(250);
  }
  console.log("live:", JSON.stringify(live));

  // 6) live control channel: HOME key injected over the WebSocket control
  //    connection. live stays connected and the device must still respond.
  let liveKey = { skipped: true };
  if (live.connected) {
    liveKey = await page.evaluate(() => {
      try {
        sendControl({ type: "key", action: 0, keycode: 3, meta: 0 });
        sendControl({ type: "key", action: 1, keycode: 3, meta: 0 });
        return { sent: true, wsState: live.ws && live.ws.readyState };
      } catch (e) {
        return { sent: false, error: e.message };
      }
    });
    await sleep(600);
    const still = await page.evaluate(() => ({ connected: live.connected, frames: live.frames }));
    liveKey.stillConnected = still.connected;
    liveKey.framesAfter = still.frames;
  }
  console.log("liveKey:", JSON.stringify(liveKey));

  await page.evaluate(() => { if (live.connected) liveDisconnect(); });
  await sleep(400);
  await browser.close();

  const result = {
    errors,
    capture: cap,
    captureOk: ok,
    resolve,
    resolveOk: !!resolve.ok && !!resolve.inBounds,
    replayHome: recResult,
    replayHomeOk: recResult.failed === 0,
    live,
    liveOk: live.connected && live.videoW > 0,
    liveKey,
    liveKeyOk: liveKey.sent && liveKey.stillConnected,
  };
  console.log("RESULT:", JSON.stringify(result, null, 2));
  const pass = ok && result.resolveOk && result.replayHomeOk && result.liveOk &&
    result.liveKeyOk && errors.length === 0;
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error("E2E FATAL:", e); process.exit(2); });
