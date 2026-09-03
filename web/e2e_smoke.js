const puppeteer = require("C:\\Users\\guozhiqiang\\.workbuddy\\binaries\\node\\workspace\\node_modules\\puppeteer-core");

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const BASE = "http://127.0.0.1:8000/";

(async () => {
  const errors = [];
  const browser = await puppeteer.launch({
    executablePath: EDGE,
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox", "--window-size=1400,900"],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1400, height: 900 });
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));

  await page.goto(BASE, { waitUntil: "networkidle0", timeout: 20000 });
  // app.js v=5 should be served
  const appSrc = await page.$eval("#__ignore", () => null).catch(() => null);
  await new Promise((r) => setTimeout(r, 500));

  const checks = {};
  // Right-side tabs (data-tab) and left-side tabs (data-ltab)
  checks.hasLiveTab = (await page.$('button.tab[data-tab="live"]')) ? true : false;
  checks.hasInspectTab = (await page.$('button.tab[data-tab="inspect"]')) ? true : false;
  checks.hasAppsLtab = (await page.$('button.ltab[data-ltab="apps"]')) ? true : false;
  checks.hasSettingsLtab = (await page.$('button.ltab[data-ltab="settings"]')) ? true : false;
  // Recording controls present
  for (const id of ["recToggle", "recSave", "recLoad", "recSteps", "recPanel"]) {
    checks[id] = (await page.$(`#${id}`)) ? true : false;
  }
  // replayStep wired for capture mode: simulate a recording + replay with no device
  checks.replayNoCrash = await page.evaluate(async () => {
    try {
      if (typeof rec === "undefined") return "no-rec";
      // Fractional coords (0..1) — the Rust record.rs convention.
      rec.steps = [
        { action: "tap", fx: 0.3, fy: 0.2, ts: 0 },
        { action: "long_tap", fx: 0.4, fy: 0.2, ts: 1 },
        { action: "swipe", from_fx: 0.2, from_fy: 0.5, to_fx: 0.6, to_fy: 0.5, ts: 2 },
        { action: "text", text: "hi", ts: 3 },
        { action: "key", keycode: 3, key: "返回", ts: 4 },
        { action: "scroll", fx: 0.5, fy: 0.5, v: -200, ts: 5 },
      ];
      // inspect mode => live.connected false => must hit adb branch, not throw
      const results = [];
      for (const s of rec.steps) {
        results.push(await replayStep(s));
      }
      return results.every((r) => r !== undefined) ? "ok" : "bad";
    } catch (e) {
      return "throw: " + e.message;
    }
  });

  // Recording descriptions use the Rust-style Chinese format
  checks.describe = await page.evaluate(() => {
    return [
      describeStep({ action: "tap", selector: { resource_id: "com.x:id/y", text: "按钮" } }),
      describeStep({ action: "text", text: "hello" }),
      describeStep({ action: "key", keycode: 4, key: "返回" }),
    ].join("|");
  });

  // Live bar must NOT offer UI-tree grabbing anymore (aligned with Rust live)
  checks.liveTreeRemoved = await page.evaluate(() => {
    return !document.querySelector("#liveDump") && !document.querySelector("#liveOverlayChk");
  });

  // Right-click copy menu on the tree (needs an imported capture; without one
  // the menu just must not appear and not throw).
  checks.ctxMenuNoTree = await page.evaluate(() => {
    const row = document.querySelector("#treeBody .tree-row");
    if (!row) return "no-tree(ok)";
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    return document.querySelector("#ctxMenu").hidden ? "hidden(ok)" : "opened";
  });

  // Tab switching should not error
  for (const sel of [
    'button.tab[data-tab="live"]',
    'button.ltab[data-ltab="apps"]',
    'button.ltab[data-ltab="settings"]',
    'button.ltab[data-ltab="device"]',
    'button.ltab[data-ltab="props"]',
    'button.tab[data-tab="inspect"]',
  ]) {
    const el = await page.$(sel);
    if (el) { await el.click(); await new Promise((r) => setTimeout(r, 120)); }
  }
  checks.tabSwitchOk = errors.length === 0;

  await browser.close();
  console.log(JSON.stringify({ errors, checks }, null, 2));
  if (errors.length > 0) process.exit(1);
})().catch((e) => {
  console.error("E2E FATAL:", e);
  process.exit(2);
});
