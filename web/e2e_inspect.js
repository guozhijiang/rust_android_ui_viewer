// Offline inspect-mode E2E for the web UI viewer.
// Imports the synthetic fixture (tools/fixture.png + tools/fixture.xml) through
// the real doImport() -> /api/import -> loadCapture() path, then exercises the
// features that would otherwise need a phone: tree render, node selection,
// properties, overlay highlight, search filtering and zoom.
//
//   node e2e_inspect.js
//
const puppeteer = require("C:\\Users\\guozhiqiang\\.workbuddy\\binaries\\node\\workspace\\node_modules\\puppeteer-core");
const fs = require("fs");
const path = require("path");

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const BASE = "http://127.0.0.1:8000/";
const TOOLS = path.join(__dirname, "tools");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const errors = [];
  const browser = await puppeteer.launch({
    executablePath: EDGE,
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox", "--window-size=1600,1000"],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1600, height: 1000 });
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));

  await page.goto(BASE, { waitUntil: "networkidle0", timeout: 20000 });
  await sleep(400);

  const R = {};
  const pngB64 = fs.readFileSync(path.join(TOOLS, "fixture.png")).toString("base64");
  const xmlText = fs.readFileSync(path.join(TOOLS, "fixture.xml"), "utf8");

  // 1) import through the real user path
  R.import = await page.evaluate(async (b64, xml) => {
    const bin = atob(b64);
    const arr = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
    const imgFile = new File([arr], "fixture.png", { type: "image/png" });
    const xmlFile = new File([xml], "fixture.xml", { type: "text/xml" });
    await doImport(imgFile, xmlFile);
    return {
      nodes: state.nodes.size, w: state.naturalW, h: state.naturalH,
      hasImage: !!state.image, status: document.querySelector("#status").textContent,
    };
  }, pngB64, xmlText);

  // 2) tree rendered
  R.treeRows = await page.$$eval("#treeBody .tree-row", (els) => els.length);

  // 3) select the WLAN text node -> properties + overlay
  const wlanId = await page.evaluate(() => {
    for (const [id, n] of state.nodes) {
      if (n.attrs && n.attrs["text"] === "WLAN") return id;
    }
    return null;
  });
  R.wlanId = wlanId;
  if (wlanId !== null) {
    // The tree only auto-expands to depth 2, so open every collapsed toggle
    // until none remain (this also exercises the expand/collapse toggles).
    // Done in-page: each click re-renders the tree, invalidating handles.
    await page.evaluate(async () => {
      for (let i = 0; i < 300; i++) {
        const t = [...document.querySelectorAll("#treeBody .tree-toggle")]
          .find((e) => e.textContent === "▸");
        if (!t) break;
        t.click();
        await new Promise((r) => setTimeout(r, 25));
      }
    });
    await sleep(200);
    R.rowsAfterExpand = await page.$$eval("#treeBody .tree-row", (els) => els.length);
    await page.click(`.tree-row[data-id="${wlanId}"]`);
    await sleep(200);
    R.propsLen = await page.$eval("#propsBody", (el) => el.textContent.trim().length);
    R.propsHasId = await page.$eval("#propsBody", (el) =>
      el.textContent.includes("com.android.settings:id/wlan"));
    R.overlayShapes = await page.$$eval("#overlay *", (els) => els.length);
    R.selected = await page.evaluate(() => state.selectedId);
  }

  // 4) search filtering
  await page.type("#search", "蓝牙");
  await sleep(250);
  R.searchRows = await page.$$eval("#treeBody .tree-row", (els) => els.length);
  R.searchMatches = await page.$$eval("#treeBody .tree-row.match", (els) => els.length);
  await page.$eval("#search", (el) => { el.value = ""; el.dispatchEvent(new Event("input")); });
  await sleep(250);
  R.searchClearedRows = await page.$$eval("#treeBody .tree-row", (els) => els.length);

  // 5) zoom
  const s0 = await page.evaluate(() => state.scale);
  await page.click("#zoomIn");
  await sleep(150);
  const s1 = await page.evaluate(() => state.scale);
  await page.click("#zoomOut");
  await sleep(150);
  const s2 = await page.evaluate(() => state.scale);
  await page.click("#zoomFit");
  await sleep(200);
  const s3 = await page.evaluate(() => state.scale);
  R.zoom = { s0, s1, s2, s3, scaledUp: s1 > s0, backDown: s2 < s1 };

  // 6) save-xml round trip (token endpoint)
  R.saveXml = await page.evaluate(async () => {
    const res = await fetch("/api/save-xml", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ xml: state.rawXml || "" }),
    });
    if (!res.ok) return { ok: false, status: res.status };
    const d = await res.json();
    return { ok: !!(d.token || d.url), keys: Object.keys(d) };
  });

  // 7) right-click copy menu on a tree row and on a property row
  R.ctxMenu = await page.evaluate(async () => {
    const out = {};
    // Tree row: find the WLAN text node row (like step 2) and right-click it.
    const row = [...document.querySelectorAll("#treeBody .tree-row")]
      .find((r) => (r.textContent || "").includes("WLAN"));
    if (!row) return { error: "no WLAN row" };
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true,
      clientX: 200, clientY: 200 }));
    const menu = document.querySelector("#ctxMenu");
    await new Promise((r) => setTimeout(r, 50));
    out.opened = !menu.hidden;
    out.items = [...menu.querySelectorAll(".ctx-item")].map((e) => e.textContent);
    // Click the bounds item and confirm the menu closes.
    const boundsItem = [...menu.querySelectorAll(".ctx-item")]
      .find((e) => e.textContent === "复制 bounds");
    if (boundsItem) boundsItem.click();
    await new Promise((r) => setTimeout(r, 50));
    out.closedAfterClick = menu.hidden;
    out.copiedStatus = document.querySelector("#status").textContent;
    // Property row menu (a node is still selected → prop rows exist).
    const prow = document.querySelector("#propsBody .prop-row");
    if (prow) {
      prow.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true,
        clientX: 220, clientY: 260 }));
      await new Promise((r) => setTimeout(r, 50));
      out.propItems = [...document.querySelectorAll("#ctxMenu .ctx-item")]
        .map((e) => e.textContent);
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    }
    return out;
  });

  await browser.close();

  // NOTE: after clearing the search the row count does NOT return to the
  // initial `treeRows` — the selected node keeps its ancestors expanded. That
  // is correct behaviour, so only assert the filter shrank and clearing grew.
  const pass =
    R.import.nodes === 40 && R.import.w === 470 && R.import.h === 1024 &&
    R.treeRows > 5 && R.rowsAfterExpand === 40 &&
    R.propsHasId === true && R.overlayShapes > 0 &&
    R.searchRows < R.rowsAfterExpand && R.searchMatches > 0 &&
    R.searchClearedRows > R.searchRows &&
    R.zoom.scaledUp && R.zoom.backDown && R.saveXml.ok &&
    R.ctxMenu.opened && R.ctxMenu.items.length >= 5 &&
    R.ctxMenu.closedAfterClick && R.ctxMenu.copiedStatus === "已复制到剪贴板" &&
    (R.ctxMenu.propItems || []).length >= 2 &&
    errors.length === 0;

  console.log(JSON.stringify({ errors, results: R, pass }, null, 2));
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error("E2E FATAL:", e); process.exit(2); });
