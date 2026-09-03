// Renders the app with the fixture loaded and saves a screenshot, so the
// layout (panel widths, top bar, badge, overlay) can be eyeballed offline.
//   node shot.js [outfile]
const puppeteer = require("C:\\Users\\guozhiqiang\\.workbuddy\\binaries\\node\\workspace\\node_modules\\puppeteer-core");
const fs = require("fs");
const path = require("path");

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const BASE = "http://127.0.0.1:8000/";
const TOOLS = path.join(__dirname, "tools");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const out = process.argv[2] || path.join(__dirname, "shot.png");
  const browser = await puppeteer.launch({
    executablePath: EDGE,
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1600, height: 1000 });
  await page.goto(BASE, { waitUntil: "networkidle0", timeout: 20000 });
  await sleep(400);

  const pngB64 = fs.readFileSync(path.join(TOOLS, "fixture.png")).toString("base64");
  const xmlText = fs.readFileSync(path.join(TOOLS, "fixture.xml"), "utf8");
  await page.evaluate(async (b64, xml) => {
    const bin = atob(b64);
    const arr = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
    await doImport(new File([arr], "fixture.png", { type: "image/png" }),
                   new File([xml], "fixture.xml", { type: "text/xml" }));
  }, pngB64, xmlText);
  await sleep(500);

  // expand the tree and select the WLAN row so props + overlay are visible
  await page.evaluate(async () => {
    for (let i = 0; i < 300; i++) {
      const t = [...document.querySelectorAll("#treeBody .tree-toggle")]
        .find((e) => e.textContent === "▸");
      if (!t) break;
      t.click();
      await new Promise((r) => setTimeout(r, 15));
    }
  });
  const id = await page.evaluate(() => {
    for (const [i, n] of state.nodes) if (n.attrs && n.attrs["text"] === "WLAN") return i;
    return null;
  });
  if (id !== null) { await page.click(`.tree-row[data-id="${id}"]`); await sleep(300); }

  // report the computed column widths so the layout can be verified numerically
  const cols = await page.evaluate(() => {
    const cs = getComputedStyle(document.querySelector("#layout"));
    const r = (s) => { const e = document.querySelector(s); return e ? Math.round(e.getBoundingClientRect().width) : null; };
    return {
      grid: cs.gridTemplateColumns,
      left: r("#props"), center: r("#viewer"), right: r("#tree"),
      badgeVisible: !document.querySelector("#connBadge").hidden,
    };
  });
  console.log(JSON.stringify(cols, null, 2));

  await page.screenshot({ path: out });
  console.log("saved", out);
  await browser.close();
})().catch((e) => { console.error("FATAL:", e); process.exit(1); });
