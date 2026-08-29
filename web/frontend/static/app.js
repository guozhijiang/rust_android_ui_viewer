"use strict";

// --------------------------------------------------------------------------- //
// Global state
// --------------------------------------------------------------------------- //
const state = {
  tree: null,
  nodes: new Map(),
  parents: new Map(),
  selectedId:  null,
  image: null, // data URL (capture mode)
  rawXml: null, // original UI hierarchy XML text (for accurate export)
  naturalW: 0,
  naturalH: 0,
  scale: 1,
  tx: 0,
  ty: 0,
  search: "",
  treeFrom: "capture",
  treeScaleX: 1,
  treeScaleY: 1,
  zoom: 1, // user zoom multiplier (inspect mode)
  panning: false,
};

const live = {
  active: false,
  connected: false,
  ws: null,
  decoder: null,
  codec: null,
  videoW: 0,
  videoH: 0,
  treePhysicalW: 0,
  treePhysicalH: 0,
  treeScale: 1,
  kbd: false,
  recoverTimer: null,
  frameW: 0,
  frameH: 0,
  frameSeen: false,
  autoRetried: false,
  frameTimer: null,
  // Per-mode view transform. Inspect and live used to share a single
  // scale/tx/ty, so switching tabs carried the other mode's pan/zoom over and
  // the picture appeared to jump. Keeping them separate lets each mode restore
  // its own correct geometry on switch.
  viewScale: 1,
  viewTx: 0,
  viewTy: 0,
};

// Recording state
const rec = {
  recording: false,
  steps: [],
  lastTapSelector: null,
  replaying: false,
  replayIdx: null,
  replayFailed: [],
  speed: 1.0,
  loops: 0,
  replayAbort: false,
};

const $ = (sel) => document.querySelector(sel);

// --------------------------------------------------------------------------- //
// Small helpers
// --------------------------------------------------------------------------- //
function setStatus(msg, isErr) {
  const s = $("#status");
  s.textContent = msg;
  s.style.color = isErr ? "#ff6b6b" : "var(--muted)";
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

async function refreshDevices(silent) {
  try {
    const res = await fetch("/api/devices");
    const data = await res.json();
    const devs = data.devices || [];
    const sel = $("#device");
    const cur = sel.value;
    const current = [...sel.options].map((o) => o.value).filter((v) => v !== "");
    if (JSON.stringify(devs) !== JSON.stringify(current)) {
      sel.innerHTML = '<option value="">（默认设备）</option>';
      for (const d of devs) {
        const o = document.createElement("option");
        o.value = d;
        o.textContent = d;
        sel.appendChild(o);
      }
      if (cur && devs.includes(cur)) sel.value = cur;
    }
    if (!silent) {
      setStatus(devs.length ? `已刷新 · ${devs.length} 台设备` : "未检测到设备");
    }
    return devs;
  } catch (e) {
    if (!silent) setStatus("无法获取设备列表: " + e.message, true);
    return [];
  }
}

function currentSerial() {
  return $("#device").value || "";
}

// --------------------------------------------------------------------------- //
// Capture mode
// --------------------------------------------------------------------------- //
async function doCapture() {
  const btn = $("#capture");
  if (btn.disabled) return;
  btn.disabled = true;
  setStatus("抓取中…");
  try {
    const res = await fetch("/api/capture?serial=" + encodeURIComponent(currentSerial()), {
      method: "POST",
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || res.statusText);
    }
    const data = await res.json();
    loadCapture(data);
    setStatus(`已抓取 ${data.width}×${data.height} · ${state.nodes.size} 节点`);
  } catch (e) {
    setStatus("抓取失败: " + e.message, true);
  } finally {
    btn.disabled = false;
  }
}

async function doImport(imgFile, xmlFile) {
  setStatus("导入中…");
  try {
    const form = new FormData();
    form.append("screenshot", imgFile);
    form.append("ui_xml", xmlFile);
    const res = await fetch("/api/import", { method: "POST", body: form });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || res.statusText);
    }
    const data = await res.json();
    loadCapture(data);
    setStatus(`已导入 ${data.width}×${data.height} · ${state.nodes.size} 节点`);
  } catch (e) {
    setStatus("导入失败: " + e.message, true);
  }
}

function triggerDownload(href, filename) {
  const a = document.createElement("a");
  a.href = href;
  a.download = filename;
  a.style.display = "none";
  // The anchor must live in the document or some browsers ignore the click.
  document.body.appendChild(a);
  a.click();
  setTimeout(() => a.remove(), 1000);
}

async function saveDump() {
  if (!state.image) {
    setStatus("没有可保存的截图/XML", true);
    return;
  }
  const stamp = Date.now();
  triggerDownload(state.image, `screenshot_${stamp}.png`);
  // Download XML — prefer the original dump so attributes/order are preserved;
  // fall back to rebuilding from the parsed tree if not available.
  let xml = state.rawXml;
  if (!xml && state.tree) xml = treeToXml(state.tree);
  if (xml) {
    try {
      // Serve the hierarchy over HTTP instead of a blob:/data: URL: a second
      // in-page download is often blocked silently by the browser.
      const res = await fetch("/api/save-xml", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ xml }),
      });
      if (!res.ok) throw new Error((await res.json().catch(() => ({}))).detail || res.statusText);
      const d = await res.json();
      triggerDownload(d.url, `hierarchy_${stamp}.xml`);
      setStatus("已保存截图与 XML");
    } catch (e) {
      setStatus("XML 保存失败: " + e.message, true);
    }
  } else {
    setStatus("已保存截图（无 XML）");
  }
}

function treeToXml(node) {
  // Minimal reconstruction for round-trip save (attrs only, no bounds reparse)
  function escAttr(v) { return String(v == null ? "" : v).replace(/&/g, "&amp;").replace(/"/g, "&quot;"); }
  function rec(n) {
    const cls = (n.attrs && n.attrs["class"]) || "android.view.View";
    let attrs = '';
    if (n.attrs) {
      for (const [k, v] of Object.entries(n.attrs)) {
        if (k === "class") continue;
        attrs += ` ${k}="${escAttr(v)}"`;
      }
    }
    const b = n.bounds;
    if (b) attrs += ` bounds="[${b.left},${b.top}][${b.right},${b.bottom}]"`;
    let inner = "";
    for (const c of (n.children || [])) inner += rec(c);
    return `<node${attrs}>${inner}</node>`;
  }
  return `<?xml version='1.0' encoding='UTF-8'?>\n<hierarchy>` + rec(state.tree) + `</hierarchy>`;
}

function loadCapture(data) {
  state.tree = data.tree;
  state.treeFrom = "capture";
  state.treeScaleX = 1;
  state.treeScaleY = 1;
  state.image = data.image;
  state.rawXml = data.raw_xml || null;
  state.naturalW = data.width;
  state.naturalH = data.height;
  state.selectedId = null;

  state.nodes.clear();
  state.parents.clear();
  indexTree(data.tree, null);

  $("#screen").src = data.image;
  applyDisplayMode();
  setOverlaySize(data.width, data.height);
  $("#nodeCount").textContent = state.nodes.size + " 节点";

  renderProps(null);
  renderTree();
  drawOverlay();
  fitView();
}

function indexTree(node, parentId) {
  state.nodes.set(node.id, node);
  if (parentId !== null) state.parents.set(node.id, parentId);
  for (const c of node.children) indexTree(c, node.id);
}

// --------------------------------------------------------------------------- //
// Properties panel
// --------------------------------------------------------------------------- //
const HILITE_KEYS = new Set([
  "class", "package", "resource-id", "text", "content-desc",
  "bounds", "clickable", "focusable", "enabled", "checked", "selected",
  "scrollable", "password", "index",
]);

function renderProps(node) {
  const body = $("#propsBody");
  if (!node) {
    body.innerHTML = '<div class="empty">未选择节点</div>';
    return;
  }
  const attrs = node.attrs || {};
  const keys = Object.keys(attrs).sort((a, b) => {
    const ai = HILITE_KEYS.has(a) ? 0 : 1;
    const bi = HILITE_KEYS.has(b) ? 0 : 1;
    return ai - bi;
  });
  let html = "";
  for (const k of keys) {
    const v = attrs[k] == null ? "" : attrs[k];
    const hl = HILITE_KEYS.has(k) ? " hl" : "";
    html += `<div class="prop-row"><div class="prop-key">${esc(k)}</div>` +
            `<div class="prop-val${hl}">${esc(v)}</div></div>`;
  }
  if (node.bounds) {
    const b = node.bounds;
    html += `<div class="prop-row"><div class="prop-key">尺寸</div>` +
            `<div class="prop-val">${b.right - b.left} x ${b.bottom - b.top} px</div></div>`;
  }
  body.innerHTML = html;
}

// --------------------------------------------------------------------------- //
// Tree rendering + search/filter
// --------------------------------------------------------------------------- //
function shortClass(cls) {
  if (!cls) return "?";
  const i = cls.lastIndexOf(".");
  return i >= 0 ? cls.slice(i + 1) : cls;
}

function nodeLabel(node) {
  const a = node.attrs || {};
  const cls = shortClass(a["class"]);
  let extra = a["text"] || a["resource-id"] || a["content-desc"] || "";
  if (extra.length > 40) extra = extra.slice(0, 40) + "…";
  let label = `<span class="cls">${esc(cls)}</span>`;
  if (extra) label += ` <span class="txt">·</span> <span class="txt">${esc(extra)}</span>`;
  return label;
}

function subtreeMatches(node, q) {
  const lc = q.toLowerCase();
  for (const v of Object.values(node.attrs || {})) {
    if (v && v.toLowerCase().includes(lc)) return true;
  }
  for (const c of node.children) if (subtreeMatches(c, q)) return true;
  return false;
}

function descendantMatches(node, q) {
  for (const c of node.children) {
    if (subtreeMatches(c, q)) return true;
  }
  return false;
}

function childVisible(node, q) {
  return q ? (subtreeMatches(node, q) || descendantMatches(node, q)) : true;
}

function shouldOpen(node, q, depth) {
  if (q) return true;
  if (node.id === state.selectedId) return true;
  let cur = state.parents.get(state.selectedId);
  while (cur !== undefined) {
    if (cur === node.id) return true;
    cur = state.parents.get(cur);
  }
  return depth < 2;
}

function renderTree() {
  const body = $("#treeBody");
  body.innerHTML = "";
  if (!state.tree) return;
  const q = state.search.trim();
  const root = document.createElement("ul");
  root.appendChild(buildTreeNode(state.tree, q, 0));
  body.appendChild(root);
  markTreeSelection();
}

function buildTreeNode(node, q, depth) {
  const li = document.createElement("li");
  li.className = "tree-node";

  const row = document.createElement("div");
  row.className = "tree-row";
  row.dataset.id = node.id;
  if (node.id === state.selectedId) row.classList.add("selected");

  const hasChildren = node.children.length > 0;
  const open = shouldOpen(node, q, depth);
  if (q && subtreeMatches(node, q)) row.classList.add("match");

  const toggle = document.createElement("span");
  toggle.className = "tree-toggle";
  if (hasChildren) toggle.textContent = open ? "▾" : "▸";

  const label = document.createElement("span");
  label.className = "tree-label";
  label.innerHTML = nodeLabel(node);

  row.appendChild(toggle);
  row.appendChild(label);
  li.appendChild(row);

  if (hasChildren && open) {
    const ul = document.createElement("ul");
    for (const c of node.children) {
      if (childVisible(c, q)) ul.appendChild(buildTreeNode(c, q, depth + 1));
    }
    li.appendChild(ul);
  }

  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!hasChildren) return;
    const ul = li.querySelector(":scope > ul");
    if (ul) { ul.remove(); toggle.textContent = "▸"; }
    else {
      const newUl = document.createElement("ul");
      for (const c of node.children) newUl.appendChild(buildTreeNode(c, q, depth + 1));
      li.appendChild(newUl);
      toggle.textContent = "▾";
    }
  });
  row.addEventListener("click", () => selectNode(node.id, true));
  return li;
}

function markTreeSelection() {
  document.querySelectorAll(".tree-row").forEach((r) => r.classList.remove("selected"));
  if (state.selectedId === null) return;
  const el = document.querySelector(`.tree-row[data-id="${state.selectedId}"]`);
  if (el) el.classList.add("selected");
}

function expandAncestors(id) {
  const q = state.search.trim();
  const chain = [];
  let cur = state.parents.get(id);
  while (cur !== undefined) { chain.push(cur); cur = state.parents.get(cur); }
  for (const aid of chain) {
    const row = document.querySelector(`.tree-row[data-id="${aid}"]`);
    if (!row) continue;
    const li = row.parentElement;
    if (li.querySelector(":scope > ul")) continue;
    const node = state.nodes.get(aid);
    if (!node || !node.children.length) continue;
    const ul = document.createElement("ul");
    for (const c of node.children) {
      if (childVisible(c, q)) ul.appendChild(buildTreeNode(c, q, 1));
    }
    li.appendChild(ul);
    const tog = row.querySelector(".tree-toggle");
    if (tog) tog.textContent = "▾";
  }
}

// --------------------------------------------------------------------------- //
// Selection + overlay highlight
// --------------------------------------------------------------------------- //
function selectNode(id, fromTree) {
  if (id === 0) return;
  state.selectedId = id;
  renderProps(state.nodes.get(id));
  drawOverlay();
  expandAncestors(id);
  markTreeSelection();
  if (fromTree) {
    scrollTreeTo(id);
    ensureNodeVisible(id);
  }
  // Never recenter on click: it made the picture jump and the highlight land
  // away from the spot the user actually clicked.
}

function scrollTreeTo(id) {
  const el = document.querySelector(`.tree-row[data-id="${id}"]`);
  if (el) el.scrollIntoView({ block: "nearest", behavior: "auto" });
}

// Pan the minimum amount needed to bring a node into view. Used only when the
// selection comes from the tree panel: clicking the image must never move the
// view, otherwise the picture jumps on every click.
function ensureNodeVisible(id) {
  const node = state.nodes.get(id);
  if (!node || !node.bounds) return;
  const { w, h } = currentViewSize();
  if (!w || !h) return;
  const vp = $("#viewport");
  const aw = vp.clientWidth, ah = vp.clientHeight;
  if (!aw || !ah) return;
  const b = node.bounds;
  const sx = state.treeScaleX || 1;
  const sy = state.treeScaleY || 1;
  const s = live.active ? live.viewScale : state.scale;
  const panX = live.active ? live.viewTx : state.tx;
  const panY = live.active ? live.viewTy : state.ty;
  const x0 = b.left * sx * s + panX;
  const y0 = b.top * sy * s + panY;
  const x1 = b.right * sx * s + panX;
  const y1 = b.bottom * sy * s + panY;
  const M = 16;
  const nw = x1 - x0, nh = y1 - y0;
  let dx = 0, dy = 0;
  if (nw >= aw - 2 * M) dx = (aw - nw) / 2 - x0;
  else if (x0 < M) dx = M - x0;
  else if (x1 > aw - M) dx = aw - M - x1;
  if (nh >= ah - 2 * M) dy = (ah - nh) / 2 - y0;
  else if (y0 < M) dy = M - y0;
  else if (y1 > ah - M) dy = ah - M - y1;
  if (!dx && !dy) return;
  if (live.active) { live.viewTx += dx; live.viewTy += dy; }
  else { state.tx += dx; state.ty += dy; }
  clampPan();
  applyTransform();
}

// Upper guard so very deep hierarchies don't paint thousands of SVG rects on
// every selection. A few hundred is still cheap and gives immediate visual
// feedback that a tree was captured.
const FAINT_NODE_LIMIT = 800;

function drawOverlay() {
  const svg = $("#overlay");
  if (live.active) {
    if (!$("#liveOverlayChk").checked) {
      while (svg.firstChild) svg.removeChild(svg.firstChild);
      return;
    }
    const w = live.frameW || live.videoW;
    const h = live.frameH || live.videoH;
    if (w && h) setOverlaySize(w, h);
  } else if (state.naturalW && state.naturalH) {
    // Inspect mode: the overlay must follow the screenshot size, not whatever
    // the live feed last used (otherwise highlights are scaled wrong).
    setOverlaySize(state.naturalW, state.naturalH);
  }
  while (svg.firstChild) svg.removeChild(svg.firstChild);
  if (state.selectedId === null && state.nodes.size === 0) return;

  const ns = "http://www.w3.org/2000/svg";
  const total = state.nodes.size;
  const drawFaint = total > 0 && total < FAINT_NODE_LIMIT;

  if (drawFaint) {
    for (const node of state.nodes.values()) {
      if (!node.bounds) continue;
      if (node.id === state.selectedId) continue;
      const r = document.createElementNS(ns, "rect");
      setRect(r, node.bounds);
      r.setAttribute("fill", "none");
      r.setAttribute("stroke", "rgba(120,200,255,0.10)");
      r.setAttribute("stroke-width", "1");
      svg.appendChild(r);
    }
  }

  if (state.selectedId !== null) {
    let cur = state.parents.get(state.selectedId);
    while (cur !== undefined) {
      const node = state.nodes.get(cur);
      if (node && node.bounds) {
        const r = document.createElementNS(ns, "rect");
        setRect(r, node.bounds);
        r.setAttribute("fill", "none");
        r.setAttribute("stroke", "rgba(255,255,255,0.50)");
        r.setAttribute("stroke-width", "1.5");
        svg.appendChild(r);
      }
      cur = state.parents.get(cur);
    }
  }

  const sel = state.selectedId !== null ? state.nodes.get(state.selectedId) : null;
  if (sel && sel.bounds) {
    const b = sel.bounds;
    const r = document.createElementNS(ns, "rect");
    setRect(r, b);
    r.setAttribute("fill", "rgba(0, 200, 255, 0.35)");
    r.setAttribute("stroke", "rgb(0, 230, 255)");
    r.setAttribute("stroke-width", "3");
    svg.appendChild(r);
    // Corner markers so small nodes are still easy to spot.
    const m = Math.min(b.right - b.left, b.bottom - b.top, 24) / 2;
    const corners = [
      [b.left, b.top, 1, 1],
      [b.right, b.top, -1, 1],
      [b.left, b.bottom, 1, -1],
      [b.right, b.bottom, -1, -1],
    ];
    for (const [cx, cy, sx, sy] of corners) {
      const p = document.createElementNS(ns, "path");
      const d = `M ${cx + m * sx} ${cy} L ${cx} ${cy} L ${cx} ${cy + m * sy}`;
      p.setAttribute("d", d);
      p.setAttribute("stroke", "rgb(0, 230, 255)");
      p.setAttribute("stroke-width", "3");
      p.setAttribute("fill", "none");
      svg.appendChild(p);
    }
  }
}

function setRect(r, b) {
  const sx = state.treeScaleX || 1;
  const sy = state.treeScaleY || 1;
  r.setAttribute("x", Math.round(b.left * sx));
  r.setAttribute("y", Math.round(b.top * sy));
  r.setAttribute("width", Math.max(1, Math.round((b.right - b.left) * sx)));
  r.setAttribute("height", Math.max(1, Math.round((b.bottom - b.top) * sy)));
}

// Pin the overlay SVG to the exact image/frame pixel size so its coordinate
// system maps 1:1 onto the screenshot/video. The stage transform then scales
// both together, guaranteeing the highlight aligns with what is shown.
function setOverlaySize(w, h) {
  const ov = $("#overlay");
  ov.setAttribute("viewBox", `0 0 ${w} ${h}`);
  ov.style.width = w + "px";
  ov.style.height = h + "px";
}

function hitTest(x, y) {
  let best = null, bestArea = Infinity;
  function rec(node) {
    const b = node.bounds;
    if (b && x >= b.left && x <= b.right && y >= b.top && y <= b.bottom) {
      const area = (b.right - b.left) * (b.bottom - b.top);
      if (area < bestArea) { bestArea = area; best = node.id; }
    }
    for (const c of node.children) rec(c);
  }
  rec(state.tree);
  return best;
}

// --------------------------------------------------------------------------- //
// Viewport: zoom + pan (inspect) + coordinate mapping
// --------------------------------------------------------------------------- //
function applyTransform() {
  const s = live.active ? live.viewScale : state.scale;
  const tx = live.active ? live.viewTx : state.tx;
  const ty = live.active ? live.viewTy : state.ty;
  $("#stage").style.transform = `translate(${tx}px, ${ty}px) scale(${s})`;
}

function currentViewSize() {
  if (live.active) return { w: live.videoW, h: live.videoH };
  return { w: state.naturalW, h: state.naturalH };
}

// Room taken by the on-screen device controls in live mode. The picture is fit
// into what is left so the controls sit in the letterbox instead of covering
// the frame — same rule as the Rust build.
function liveChromeInsets() {
  if (!live.active) return { bottom: 0, right: 0 };
  const nav = $("#liveNav");
  const side = $("#liveSide");
  return {
    bottom: nav && !nav.hidden ? nav.offsetHeight + 14 : 0,
    right: side && !side.hidden ? side.offsetWidth + 14 : 0,
  };
}

function fitView() {
  const vp = $("#viewport");
  const aw = vp.clientWidth, ah = vp.clientHeight;
  const { w, h } = currentViewSize();
  if (!w || !h) return;
  const inset = liveChromeInsets();
  const availW = Math.max(50, aw - inset.right);
  const availH = Math.max(50, ah - inset.bottom);
  const s = Math.min(availW / w, availH / h) * 0.98 * (live.active ? 1 : state.zoom);
  const tx = (availW - w * s) / 2;
  const ty = (availH - h * s) / 2;
  if (live.active) { live.viewScale = s; live.viewTx = tx; live.viewTy = ty; }
  else { state.scale = s; state.tx = tx; state.ty = ty; }
  applyTransform();
}

function clampPan() {
  const vp = $("#viewport");
  const { w, h } = currentViewSize();
  if (!w) return;
  const s = live.active ? live.viewScale : state.scale;
  const contentW = w * s;
  const contentH = h * s;
  const aw = vp.clientWidth, ah = vp.clientHeight;
  // Only allow panning beyond edges when zoomed in.
  const minX = Math.min(0, aw - contentW);
  const minY = Math.min(0, ah - contentH);
  if (live.active) {
    live.viewTx = Math.max(minX, Math.min(0, live.viewTx));
    live.viewTy = Math.max(minY, Math.min(0, live.viewTy));
  } else {
    state.tx = Math.max(minX, Math.min(0, state.tx));
    state.ty = Math.max(minY, Math.min(0, state.ty));
  }
}

function toNatural(e) {
  const el = live.active ? $("#liveCanvas") : $("#screen");
  if (!el || el.hidden) return null;
  const rect = el.getBoundingClientRect();
  if (!rect.width || !rect.height) return null;
  const { w, h } = currentViewSize();
  if (!w || !h) return null;
  const x = Math.round((e.clientX - rect.left) * (w / rect.width));
  const y = Math.round((e.clientY - rect.top) * (h / rect.height));
  if (x < 0 || y < 0 || x > w || y > h) return null;
  return { x, y };
}

function liveToTree(x, y) {
  const sx = state.treeScaleX || 1;
  const sy = state.treeScaleY || 1;
  if (sx !== 1 || sy !== 1) {
    return { x: Math.round(x / sx), y: Math.round(y / sy) };
  }
  return { x, y };
}

function updateZoomLabel() {
  $("#zoomLabel").textContent = state.zoom.toFixed(2) + "x";
}

// --------------------------------------------------------------------------- //
// Mouse interaction
// --------------------------------------------------------------------------- //
const vp = $("#viewport");
let drag = null;
let touch = null;
let touch2 = null;

vp.addEventListener("mousedown", (e) => {
  if (live.active) {
    const nat = toNatural(e);
    if (e.button === 0) {
      if (!nat) return;
      touch = { startX: nat.x, startY: nat.y, moved: false, pointer: 0, startTime: Date.now() };
      sendControl({ type: "touch", action: 0, x: nat.x, y: nat.y, pointerId: 0 });
    } else if (e.button === 2) {
      if (!nat) return;
      touch2 = { startX: nat.x, startY: nat.y, moved: false, pointer: 1 };
      sendControl({ type: "touch", action: 0, x: nat.x, y: nat.y, pointerId: 1 });
    }
    return;
  }
  // Inspect mode: pan when zoomed, otherwise pending click for hit-test.
  if (e.button !== 0) return;
  drag = { x: e.clientX, y: e.clientY, tx: state.tx, ty: state.ty, moved: false };
  if (state.zoom > 1.001) {
    state.panning = true;
    $("#viewport").classList.add("grabbing");
  }
});

window.addEventListener("mousemove", (e) => {
  if (state.naturalW || live.videoW) {
    const nat = toNatural(e);
    if (nat) $("#coord").textContent = `x: ${nat.x}  y: ${nat.y}`;
  }
  if (live.active) {
    const nat = toNatural(e);
    if (touch) {
      if (nat) {
        const dx = Math.abs(nat.x - touch.startX);
        const dy = Math.abs(nat.y - touch.startY);
        if (dx + dy > 6) touch.moved = true;
        sendControl({ type: "touch", action: 2, x: nat.x, y: nat.y, pointerId: touch.pointer });
      }
    }
    if (touch2) {
      const dx = nat ? Math.abs(nat.x - touch2.startX) : 0;
      const dy = nat ? Math.abs(nat.y - touch2.startY) : 0;
      if (dx + dy > 6) touch2.moved = true;
      if (nat && touch2.moved) {
        sendControl({ type: "touch", action: 2, x: nat.x, y: nat.y, pointerId: touch2.pointer });
      }
    }
    return;
  }
  // Inspect mode: pan
  if (drag && state.panning) {
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    state.tx = drag.tx + dx;
    state.ty = drag.ty + dy;
    clampPan();
    applyTransform();
  }
});

window.addEventListener("mouseup", (e) => {
  if (live.active) {
    const nat = toNatural(e);
    if (e.button === 0 && touch) {
      const t = touch;
      touch = null;
      const elapsed = Date.now() - (t.startTime || 0);
      if (nat) sendControl({ type: "touch", action: 1, x: nat.x, y: nat.y, pointerId: t.pointer });
      // Recording: tap / long_tap / swipe
      if (rec.recording) {
        if (t.moved && nat) {
          recordStep({ action: "swipe", from_fx: t.startX, from_fy: t.startY, to_fx: nat.x, to_fy: nat.y });
        } else {
          recordStep({ action: elapsed > 500 ? "long_tap" : "tap", fx: t.startX, fy: t.startY });
        }
      }
      if (!t.moved && nat && state.tree && $("#liveOverlayChk").checked) {
        const treePt = liveToTree(nat.x, nat.y);
        const id = hitTest(treePt.x, treePt.y);
        if (id !== null) {
          selectNode(id, false);
          scrollTreeTo(id);
        }
      }
    } else if (e.button === 2 && touch2) {
      const t2 = touch2;
      touch2 = null;
      if (nat) sendControl({ type: "touch", action: 1, x: nat.x, y: nat.y, pointerId: t2.pointer });
      if (!t2.moved) {
        sendControl({ type: "key", action: 0, keycode: 4, meta: 0 });
        sendControl({ type: "key", action: 1, keycode: 4, meta: 0 });
        if (rec.recording) recordStep({ action: "key", keycode: 4, key: "返回" });
      }
    }
    return;
  }
  // Inspect mode: click selects node, drag was for pan
  if (!drag) return;
  $("#viewport").classList.remove("grabbing");
  const wasMoved = drag.moved || Math.abs(e.clientX - drag.x) > 5 || Math.abs(e.clientY - drag.y) > 5;
  const wasPanning = state.panning;
  state.panning = false;
  drag = null;
  if (!wasMoved || !wasPanning) {
    const nat = toNatural(e);
    if (nat) {
      const id = hitTest(nat.x, nat.y);
      if (id !== null) {
        selectNode(id, false);
        scrollTreeTo(id);
      }
    }
  }
});

vp.addEventListener("wheel", (e) => {
  e.preventDefault();
  if (live.active) {
    const nat = toNatural(e);
    if (nat) {
      const SCROLL_SCALE = 0.04;
      sendControl({
        type: "scroll", x: nat.x, y: nat.y,
        h: e.deltaX * SCROLL_SCALE,
        v: -e.deltaY * SCROLL_SCALE,
      });
      if (rec.recording) recordStep({ action: "scroll", fx: nat.x, fy: nat.y, h: e.deltaX * SCROLL_SCALE, v: -e.deltaY * SCROLL_SCALE });
    }
    return;
  }
  // Inspect mode: ctrl+wheel = zoom (trackpad pinch), plain wheel = nothing
  if (e.ctrlKey) {
    const delta = -e.deltaY * 0.0025;
    state.zoom = Math.max(0.5, Math.min(4, state.zoom + delta));
    updateZoomLabel();
    fitView();
  }
}, { passive: false });

vp.addEventListener("contextmenu", (e) => {
  if (live.active) e.preventDefault();
});

// --------------------------------------------------------------------------- //
// Live mode: WebCodecs H.264 decode
// --------------------------------------------------------------------------- //
let drawnW = 0, drawnH = 0;
let decErrCount = 0;
let rebuildTimer = null;

function isKeyFrame(data) {
  const n = data.length;
  for (let i = 0; i < n - 5; i++) {
    if (data[i] !== 0 || data[i + 1] !== 0) continue;
    let nalAt;
    if (data[i + 2] === 1) nalAt = i + 3;
    else if (data[i + 2] === 0 && data[i + 3] === 1) nalAt = i + 4;
    else continue;
    if ((data[nalAt] & 0x1f) === 5) return true;
  }
  return false;
}

function destroyDecoder() {
  if (rebuildTimer) { clearTimeout(rebuildTimer); rebuildTimer = null; }
  if (live.decoder) {
    try { live.decoder.close(); } catch (e) { /* noop */ }
    live.decoder = null;
  }
}

function onDecoderError(e) {
  decErrCount++;
  console.error("[live] VideoDecoder error:", e && e.message, {
    codec: live.codecUsed, size: live.videoW + "x" + live.videoH,
  });
  if (decErrCount > 4) {
    destroyDecoder();
    live.codecUsed = null;
    setStatus(`视频解码持续失败。请尝试降低分辨率，或断开后重新连接。`, true);
    return;
  }
  setStatus(`视频解码错误，自动恢复（${decErrCount}/4）…`);
  live.codec = null;
  live.codecUsed = null;
  destroyDecoder();
  rebuildTimer = setTimeout(() => ensureDecoder(), 300);
}

function boostCodecLevel(codec) {
  const m = /^(avc1\.[0-9a-f]{4})([0-9a-f]{2})$/i.exec(codec);
  return m ? m[1] + "33" : codec;
}

function ensureDecoder() {
  if (live.decoder && live.decoder.state === "configured") return;
  destroyDecoder();
  live.decoder = new VideoDecoder({
    output: (frame) => {
      const cv = $("#liveCanvas");
      const w = frame.displayWidth || live.videoW;
      const h = frame.displayHeight || live.videoH;
      if (w !== drawnW || h !== drawnH) {
        cv.width = w; cv.height = h; drawnW = w; drawnH = h;
      }
      cv.getContext("2d").drawImage(frame, 0, 0);
      frame.close();
      live.frameSeen = true;
      clearFrameWatchdog();
      if (Math.abs(w - live.frameW) > 2 || Math.abs(h - live.frameH) > 2) {
        const wasFirst = live.frameW === 0;
        live.frameW = w; live.frameH = h;
        setOverlaySize(w, h);
        updateTreeScale();
        // First real frame: fit the view to the actual canvas size. Always
        // repaint the overlay so a tree captured before the first frame is
        // scaled correctly instead of staying invisible.
        if (wasFirst) fitView();
        drawOverlay();
        updateLiveDebug();
      }
    },
    error: onDecoderError,
  });
  let candidates;
  if (live.codec) {
    const boosted = boostCodecLevel(live.codec);
    candidates = [boosted, live.codec, "avc1.640033", "avc1.640028", "avc1.42001f"];
  } else {
    const rotated = ["avc1.640033", "avc1.640028", "avc1.4d0028", "avc1.42001f"];
    candidates = rotated.concat(rotated).slice(
      decErrCount % rotated.length, decErrCount % rotated.length + rotated.length);
  }
  for (const c of candidates) {
    try {
      live.decoder.configure({ codec: c, codedWidth: live.videoW || 1280, codedHeight: live.videoH || 720 });
      live.codecUsed = c;
      return;
    } catch (e) { /* try next */ }
  }
  setStatus("无法初始化视频解码器（当前浏览器不支持 H.264）", true);
}

function handleSize(w, h) {
  const changed = w !== live.videoW || h !== live.videoH;
  live.videoW = w; live.videoH = h;
  updateTreeScale();
  if (changed) {
    decErrCount = 0;
    destroyDecoder();
    ensureDecoder();
    fitView();
    setStatus(`已连接 · ${w}×${h}`);
  }
}

function updateTreeScale() {
  const vw = live.frameW || live.videoW;
  const vh = live.frameH || live.videoH;
  // Only scale tree coords to video size while actually showing the live feed.
  // In inspect mode the screenshot is already at physical resolution, so the
  // overlay must use 1:1 coords (otherwise highlights land in the wrong place).
  if (live.active && state.treeFrom === "live" && live.treePhysicalW && vw) {
    state.treeScaleX = vw / live.treePhysicalW;
    state.treeScaleY = live.treePhysicalH && vh ? vh / live.treePhysicalH : 1;
  } else {
    state.treeScaleX = 1; state.treeScaleY = 1;
  }
}

function updateLiveDebug() {
  const el = $("#liveDebug");
  if (!el) return;
  const live_active = live.active || live.connected || live.frameW > 0;
  if (!live_active) { el.style.display = "none"; return; }
  const vw = live.frameW || live.videoW;
  const vh = live.frameH || live.videoH;
  const pw = live.treePhysicalW;
  const ph = live.treePhysicalH;
  if (state.treeFrom === "live" && pw && ph) {
    el.textContent = `scale ${state.treeScaleX.toFixed(3)}×${state.treeScaleY.toFixed(3)}  ${vw}×${vh}→${pw}×${ph}`;
  } else {
    el.textContent = `frame ${vw}×${vh}`;
  }
  el.style.display = "";
}

function onWsMessage(ev) {
  if (typeof ev.data === "string") {
    let j;
    try { j = JSON.parse(ev.data); } catch (e) { return; }
    if (j.type === "codec") live.codec = j.codec;
    else if (j.type === "size") handleSize(j.width, j.height);
    else if (j.type === "recovering") {
      setStatus("连接中断，正在自动重连…");
      if (live.recoverTimer) clearTimeout(live.recoverTimer);
      live.recoverTimer = setTimeout(() => {
        setStatus("自动重连超时，请重新连接", true);
        liveDisconnected();
      }, 8000);
    } else if (j.type === "closed") {
      setStatus("会话结束" + (j.error ? ": " + j.error : ""), !!j.error);
      liveDisconnected();
    }
    return;
  }
  if (live.recoverTimer) { clearTimeout(live.recoverTimer); live.recoverTimer = null; }
  if (!live.videoW) return;
  ensureDecoder();
  if (!live.decoder || live.decoder.state !== "configured") return;
  const data = new Uint8Array(ev.data);
  const chunk = new EncodedVideoChunk({
    type: isKeyFrame(data) ? "key" : "delta",
    timestamp: (performance.now() * 1000) | 0,
    data,
  });
  try { live.decoder.decode(chunk); }
  catch (e) { onDecoderError(e); }
}

function sendControl(msg) {
  if (!live.ws || live.ws.readyState !== WebSocket.OPEN) return;
  live.ws.send(JSON.stringify(msg));
}

async function liveConnect() {
  if (live.connected) { await liveDisconnect(); return; }
  await refreshDevices(true);
  setStatus("正在连接…");
  // Apply quality preset -> (max_size, bitrate)
  const preset = parseInt($("#livePreset").value, 10) || 1;
  const presets = [
    { max_size: 0, bitrate: 0 },     // 清晰
    { max_size: 1024, bitrate: 4_000_000 }, // 流畅
    { max_size: 720, bitrate: 2_000_000 },  // 极速
  ];
  const p = presets[preset] || presets[1];
  try {
    const res = await fetch("/api/scrcpy/start", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        serial: currentSerial(),
        max_size: p.max_size,
        bitrate: p.bitrate,
      }),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || res.statusText);
    }
    const info = await res.json();
    live.videoW = info.width;
    live.videoH = info.height;
    updateTreeScale();
    openWs(info);
  } catch (e) {
    setStatus("连接失败: " + e.message, true);
  }
}

function openWs(info) {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const myWs = new WebSocket(`${proto}://${location.host}/ws/scrcpy`);
  live.ws = myWs;
  myWs.binaryType = "arraybuffer";
  myWs.onopen = () => {
    if (live.ws !== myWs) return;
    live.connected = true;
    live.frameSeen = false;
    updateConnectBtn();
    updateLiveChrome();
    applyDisplayMode();
    $("#viewerTitle").textContent = "实时画面";
    setStatus("已连接 " + (info.deviceName || info.serial || "") + ` · ${info.width}×${info.height}`);
    fitView();
    updateLiveDebug();
    armFrameWatchdog();
  };
  myWs.onmessage = (ev) => { if (live.ws === myWs) onWsMessage(ev); };
  myWs.onclose = () => { if (live.ws === myWs) liveDisconnected(); };
  myWs.onerror = () => { if (live.ws === myWs) setStatus("WebSocket 错误", true); };
}

function liveDisconnected() {
  live.connected = false;
  destroyDecoder();
  drawnW = 0; drawnH = 0; decErrCount = 0;
  live.codecUsed = null; live.frameW = 0; live.frameH = 0;
  // videoW/H are assigned before the socket opens, so a failed connect would
  // leave stale dimensions that fitView() would happily fit against.
  live.videoW = 0; live.videoH = 0;
  clearFrameWatchdog();
  if (live.recoverTimer) { clearTimeout(live.recoverTimer); live.recoverTimer = null; }
  if (live.ws) { try { live.ws.close(); } catch (e) {} live.ws = null; }
  updateLiveDebug();
  updateConnectBtn();
  updateLiveChrome();
  applyDisplayMode();
  if (!live.active && state.image) $("#viewerTitle").textContent = "截图";
  setStatus("已断开");
}

async function liveDisconnect() {
  try { await fetch("/api/scrcpy/stop", { method: "POST" }); } catch (e) {}
  liveDisconnected();
}

function armFrameWatchdog() {
  clearFrameWatchdog();
  live.frameTimer = setTimeout(() => {
    live.frameTimer = null;
    if (!live.connected || live.frameSeen) return;
    if (live.autoRetried) { setStatus("画面未就绪，请重新连接", true); return; }
    live.autoRetried = true;
    setStatus("画面未就绪，正在自动重连…");
    liveDisconnect().then(() => liveConnect());
  }, 6000);
}

function clearFrameWatchdog() {
  if (live.frameTimer) { clearTimeout(live.frameTimer); live.frameTimer = null; }
}

function updateConnectBtn() {
  const b = $("#liveConnect");
  b.textContent = live.connected ? "断开" : "连接";
  b.classList.toggle("live-on", live.connected);
}

function loadLiveTree(data) {
  state.tree = data.tree;
  state.treeFrom = "live";
  // Do NOT overwrite state.naturalW/H: those describe the *screenshot* and are
  // what fitView()/drawOverlay() use to size the inspect view. Overwriting them
  // with the tree's physical size made the screenshot render at the wrong size
  // (and overflow the viewport) whenever the two differed, e.g. after rotation.
  // The live tree's own physical size is tracked in live.treePhysicalW/H.
  state.selectedId = null;
  live.treePhysicalW = data.width;
  live.treePhysicalH = data.height;
  updateTreeScale();
  state.nodes.clear();
  state.parents.clear();
  indexTree(data.tree, null);
  // Match the actually-decoded frame size (same precedence as drawOverlay) so
  // the tree overlay lines up with what is painted on the canvas.
  const ow = live.frameW || live.videoW || data.width;
  const oh = live.frameH || live.videoH || data.height;
  setOverlaySize(ow, oh);
  $("#nodeCount").textContent = state.nodes.size + " 节点";
  renderProps(null);
  renderTree();
  drawOverlay();
  updateLiveDebug();
  setStatus(`已抓取 UI 树 · ${state.nodes.size} 节点`);
}

async function liveDump() {
  if (!live.connected) { setStatus("请先连接设备", true); return; }
  setStatus("抓取 UI 树…");
  try {
    const res = await fetch("/api/capture?serial=" + encodeURIComponent(currentSerial()), { method: "POST" });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || res.statusText);
    }
    loadLiveTree(await res.json());
    // Grabbing the tree is pointless without showing it, so turn the overlay
    // on automatically (otherwise the button looks like it did nothing).
    $("#liveOverlayChk").checked = true;
    drawOverlay();
  } catch (e) {
    setStatus("抓取失败: " + e.message, true);
  }
}

// --------------------------------------------------------------------------- //
// Keyboard mapping
// --------------------------------------------------------------------------- //
const KEYCODES = {
  Backspace: 67, Tab: 61, Enter: 66, CapsLock: 115, Insert: 124,
  ShiftLeft: 59, ShiftRight: 60, ControlLeft: 113, ControlRight: 114,
  AltLeft: 57, AltRight: 58, Escape: 111, Delete: 112,
  ArrowUp: 19, ArrowDown: 20, ArrowLeft: 21, ArrowRight: 22,
  Home: 122, End: 123, PageUp: 92, PageDown: 93,
  Minus: 69, Equal: 70, BracketLeft: 71, BracketRight: 72, Backslash: 73,
  Semicolon: 74, Quote: 75, Slash: 76, Comma: 77, Period: 78, GraveAccent: 68,
  F1: 131, F2: 132, F3: 133, F4: 134, F5: 135, F6: 136,
  F7: 137, F8: 138, F9: 139, F10: 140, F11: 141, F12: 142,
};

const META_SHIFT = 0x1, META_ALT = 0x2, META_CTRL = 0x1000, META_META = 0x10000;

function eventToKeycode(e) {
  const c = e.code;
  if (c.startsWith("Key")) return 29 + (c.charCodeAt(3) - 65);
  if (c.startsWith("Digit")) return 7 + (c.charCodeAt(5) - 48);
  if (c.startsWith("Numpad")) {
    const d = parseInt(c.slice(6), 10);
    if (!isNaN(d)) return d === 0 ? 144 : 145 + (d - 1);
  }
  return KEYCODES[c] ?? null;
}

function metaFlags(e) {
  let m = 0;
  if (e.shiftKey) m |= META_SHIFT;
  if (e.altKey) m |= META_ALT;
  if (e.ctrlKey) m |= META_CTRL;
  if (e.metaKey) m |= META_META;
  return m;
}

function isFormField(el) {
  return el && (el.tagName === "INPUT" || el.tagName === "SELECT" || el.tagName === "TEXTAREA");
}

function sendKeyTap(keycode, meta) {
  sendControl({ type: "key", action: 0, keycode, meta: meta || 0 });
  sendControl({ type: "key", action: 1, keycode, meta: meta || 0 });
}

window.addEventListener("keydown", (e) => {
  if (!live.active) return;
  if (isFormField(e.target)) return;
  if (e.key === "Escape" && !live.kbd) { sendKeyTap(4); return; }
  if (!live.kbd) return;
  const isShortcut = e.ctrlKey || e.metaKey;
  if (isShortcut) {
    const kc = eventToKeycode(e);
    if (kc === null) return;
    e.preventDefault();
    sendControl({ type: "key", action: 0, keycode: kc, meta: metaFlags(e) });
    if (rec.recording) recordStep({ action: "key", keycode: kc, key: e.code });
    return;
  }
  if (e.key && e.key.length === 1) {
    sendControl({ type: "text", text: e.key });
    if (rec.recording) recordStep({ action: "text", text: e.key });
    return;
  }
  const kc = KEYCODES[e.code] ?? null;
  if (kc === null) return;
  e.preventDefault();
  sendControl({ type: "key", action: 0, keycode: kc, meta: metaFlags(e) });
  if (rec.recording) recordStep({ action: "key", keycode: kc, key: e.code });
});

window.addEventListener("keyup", (e) => {
  if (!live.active || !live.kbd) return;
  if (isFormField(e.target)) return;
  const kc = e.ctrlKey || e.metaKey
    ? eventToKeycode(e)
    : (KEYCODES[e.code] ?? null);
  if (kc === null) return;
  e.preventDefault();
  sendControl({ type: "key", action: 1, keycode: kc, meta: metaFlags(e) });
});

window.addEventListener("paste", (e) => {
  if (!live.active || !live.kbd || !live.connected) return;
  const text = e.clipboardData && e.clipboardData.getData("text");
  if (text) {
    e.preventDefault();
    sendControl({ type: "text", text });
    if (rec.recording) recordStep({ action: "text", text });
  }
});

// --------------------------------------------------------------------------- //
// Recording / replay
// --------------------------------------------------------------------------- //
function recordStep(step) {
  if (!rec.recording) return;
  step.ts = Date.now() / 1000;
  // Try to resolve a selector from current tree
  if ((step.fx != null) && (step.fy != null) && state.tree) {
    const id = hitTest(Math.round(step.fx), Math.round(step.fy));
    if (id !== null) step.selector = buildSelector(id);
  }
  if (step.action === "swipe" && state.tree) {
    const fid = hitTest(Math.round(step.from_fx), Math.round(step.from_fy));
    const tid = hitTest(Math.round(step.to_fx), Math.round(step.to_fy));
    if (fid !== null) step.from_selector = buildSelector(fid);
    if (tid !== null) step.to_selector = buildSelector(tid);
  }
  rec.steps.push(step);
  renderRecSteps();
}

function buildSelector(id) {
  const n = state.nodes.get(id);
  if (!n) return null;
  const a = n.attrs || {};
  const sel = {};
  if (a["resource-id"]) sel.resource_id = a["resource-id"];
  if (a["text"]) sel.text = a["text"];
  if (a["content-desc"]) sel.content_desc = a["content-desc"];
  if (a["class"]) sel.class = a["class"];
  return Object.keys(sel).length ? sel : null;
}

function renderRecSteps() {
  const el = $("#recSteps");
  if (!el) return;
  if (!rec.steps.length) {
    el.innerHTML = '<div class="rec-status">（暂无步骤）</div>';
    $("#recStatus").textContent = rec.recording ? "● 录制中（0 步）" : (rec.replaying ? "回放中…" : "未录制");
    $("#recStatus").classList.toggle("recording", rec.recording);
    return;
  }
  let html = "";
  const active = rec.replaying ? rec.replayIdx : (rec.recording ? rec.steps.length - 1 : -1);
  for (let i = 0; i < rec.steps.length; i++) {
    const s = rec.steps[i];
    const cls = rec.replayFailed.includes(i) ? "step failed" : (i === active ? "step active" : "step");
    html += `<div class="${cls}">${i + 1}. ${describeStep(s)}</div>`;
  }
  el.innerHTML = html;
  el.scrollTop = el.scrollHeight;
  $("#recStatus").textContent = rec.recording
    ? `● 录制中（${rec.steps.length} 步）`
    : (rec.replaying ? `回放中 ${rec.replayIdx + 1}/${rec.steps.length}` : `共 ${rec.steps.length} 步`);
  $("#recStatus").classList.toggle("recording", rec.recording);
  $("#recSave").disabled = rec.steps.length === 0;
}

function describeStep(s) {
  switch (s.action) {
    case "tap": return `tap (${Math.round(s.fx)}, ${Math.round(s.fy)})`;
    case "long_tap": return `long_tap (${Math.round(s.fx)}, ${Math.round(s.fy)})`;
    case "swipe": return `swipe (${Math.round(s.from_fx)},${Math.round(s.from_fy)}) → (${Math.round(s.to_fx)},${Math.round(s.to_fy)})`;
    case "text": return `text "${(s.text || "").slice(0, 30)}"`;
    case "key": return `key ${s.key || s.keycode || ""}`;
    case "scroll": return `scroll (${Math.round(s.fx)}, ${Math.round(s.fy)})`;
    default: return s.action;
  }
}

function toggleRecording() {
  if (rec.replaying) { setStatus("回放进行中，不能录制", true); return; }
  rec.recording = !rec.recording;
  if (rec.recording) {
    rec.steps = [];
    rec.replayFailed = [];
    rec.replayIdx = null;
    $("#recToggle").textContent = "■ 停止";
    $("#recToggle").classList.add("rec-on");
    setStatus("● 录制已开始");
  } else {
    $("#recToggle").textContent = "● 录制";
    $("#recToggle").classList.remove("rec-on");
    setStatus(`■ 录制已停止（共 ${rec.steps.length} 步）`);
  }
  renderRecSteps();
  $("#recPanel").hidden = !live.active;
}

function saveRecording() {
  if (!rec.steps.length) return;
  const blob = new Blob([JSON.stringify(rec.steps, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "recording_" + Date.now() + ".json";
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  setStatus(`已保存 ${rec.steps.length} 步录制`);
}

function loadRecording(file) {
  const reader = new FileReader();
  reader.onload = () => {
    try {
      const steps = JSON.parse(reader.result);
      if (!Array.isArray(steps) || !steps.length) {
        setStatus("录制文件为空或格式错误", true);
        return;
      }
      rec.steps = steps;
      renderRecSteps();
      startReplay();
    } catch (e) {
      setStatus("加载录制失败: " + e.message, true);
    }
  };
  reader.readAsText(file);
}

async function startReplay() {
  if (rec.replaying) { rec.replayAbort = true; return; }
  if (!live.connected) { setStatus("回放需先连接设备", true); return; }
  rec.replaying = true;
  rec.replayAbort = false;
  rec.replayFailed = [];
  rec.replayIdx = null;
  rec.speed = parseFloat($("#recSpeed").value) || 1;
  rec.loops = parseInt($("#recLoops").value, 10) || 0;
  $("#recToggle").disabled = true;
  const totalLoops = rec.loops + 1;
  for (let loop = 0; loop < totalLoops && !rec.replayAbort; loop++) {
    for (let i = 0; i < rec.steps.length && !rec.replayAbort; i++) {
      rec.replayIdx = i;
      renderRecSteps();
      const s = rec.steps[i];
      try {
        await replayStep(s);
      } catch (e) {
        rec.replayFailed.push(i);
        renderRecSteps();
      }
      // Inter-step delay (scaled by speed)
      const delay = (s.ts && rec.steps[i + 1] && rec.steps[i + 1].ts)
        ? Math.max(50, (rec.steps[i + 1].ts - s.ts) * 1000 / rec.speed)
        : 200;
      await sleep(delay);
    }
  }
  rec.replaying = false;
  rec.replayIdx = null;
  $("#recToggle").disabled = false;
  renderRecSteps();
  const failed = rec.replayFailed.length;
  setStatus(failed === 0 ? "回放完成" : `回放完成，${failed} 步失败`);
}

// Replay coordinates are video-frame pixels — that is what scrcpy expects, and
// it is exactly what recordStep stores (fx/fy come straight from toNatural()).
// Node bounds live in *physical* device pixels, so a point resolved through a
// selector must be scaled down by the tree scale before being sent.
function selectorToVideo(sel) {
  const pt = resolveSelector(sel);
  if (!pt) return null;
  return {
    x: Math.round(pt.x * (state.treeScaleX || 1)),
    y: Math.round(pt.y * (state.treeScaleY || 1)),
  };
}

function stepPoint(s, xKey, yKey, selKey) {
  const pt = s[selKey] ? selectorToVideo(s[selKey]) : null;
  if (pt) return pt;
  if (s[xKey] != null && s[yKey] != null) {
    return { x: Math.round(s[xKey]), y: Math.round(s[yKey]) };
  }
  return null;
}

async function replayStep(s) {
  switch (s.action) {
    case "tap": {
      const pt = stepPoint(s, "fx", "fy", "selector");
      if (!pt) break;
      sendControl({ type: "touch", action: 0, x: pt.x, y: pt.y, pointerId: 0 });
      await sleep(50);
      sendControl({ type: "touch", action: 1, x: pt.x, y: pt.y, pointerId: 0 });
      break;
    }
    case "long_tap": {
      const pt = stepPoint(s, "fx", "fy", "selector");
      if (!pt) break;
      sendControl({ type: "touch", action: 0, x: pt.x, y: pt.y, pointerId: 0 });
      await sleep(600);
      sendControl({ type: "touch", action: 1, x: pt.x, y: pt.y, pointerId: 0 });
      break;
    }
    case "swipe": {
      const from = stepPoint(s, "from_fx", "from_fy", "from_selector");
      const to = stepPoint(s, "to_fx", "to_fy", "to_selector");
      if (!from || !to) break;
      const sx = from.x, sy = from.y, ex = to.x, ey = to.y;
      sendControl({ type: "touch", action: 0, x: sx, y: sy, pointerId: 0 });
      // Intermediate moves for smooth swipe
      const steps = 10;
      for (let k = 1; k <= steps; k++) {
        const x = Math.round(sx + (ex - sx) * k / steps);
        const y = Math.round(sy + (ey - sy) * k / steps);
        sendControl({ type: "touch", action: 2, x, y, pointerId: 0 });
        await sleep(20);
      }
      sendControl({ type: "touch", action: 1, x: ex, y: ey, pointerId: 0 });
      break;
    }
    case "text": {
      sendControl({ type: "text", text: s.text || "" });
      break;
    }
    case "scroll": {
      const pt = stepPoint(s, "fx", "fy", "selector");
      if (!pt) break;
      sendControl({ type: "scroll", x: pt.x, y: pt.y, h: s.h || 0, v: s.v || 0 });
      break;
    }
    case "key": {
      sendControl({ type: "key", action: 0, keycode: s.keycode, meta: 0 });
      await sleep(30);
      sendControl({ type: "key", action: 1, keycode: s.keycode, meta: 0 });
      break;
    }
  }
}

function resolveSelector(sel) {
  // Try to find a node matching the selector; return its center.
  if (!state.tree) return null;
  let best = null;
  function rec(node) {
    const a = node.attrs || {};
    let match = true;
    if (sel.resource_id && a["resource-id"] !== sel.resource_id) match = false;
    if (match && sel.text && a["text"] !== sel.text) match = false;
    if (match && sel.content_desc && a["content-desc"] !== sel.content_desc) match = false;
    if (match && sel.class && a["class"] !== sel.class) match = false;
    if (match && node.bounds) {
      best = {
        x: Math.round((node.bounds.left + node.bounds.right) / 2),
        y: Math.round((node.bounds.top + node.bounds.bottom) / 2),
      };
      return;
    }
    for (const c of node.children) { rec(c); if (best) return; }
  }
  rec(state.tree);
  return best;
}

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

// --------------------------------------------------------------------------- //
// Left panel: device / apps / settings
// --------------------------------------------------------------------------- //
let _appFilter = "third";
let _appSearch = "";
let _appsCache = [];

async function loadDevicePanel() {
  const body = $("#deviceBody");
  body.textContent = "加载中…";
  try {
    const res = await fetch("/api/device-info-full?serial=" + encodeURIComponent(currentSerial()));
    const d = await res.json();
    const rows = [
      ["机型", d.model], ["品牌", d.brand],
      ["系统", d.android ? `Android ${d.android} (API ${d.sdk})` : "?"],
      ["分辨率", d.resolution], ["DPI", d.density],
      ["电量", d.battery], ["存储", d.storage || ""],
      ["软件版本", d.build], ["序列号", d.serial],
    ];
    body.innerHTML = rows.map(([k, v]) =>
      `<div class="prop-row"><div class="prop-key">${esc(k)}</div><div class="prop-val">${esc(v || "")}</div></div>`
    ).join("");
  } catch (e) {
    body.textContent = "加载失败: " + e.message;
  }
}

async function loadAppsPanel() {
  try {
    const res = await fetch(`/api/apps?serial=${encodeURIComponent(currentSerial())}&filter=all`);
    const data = await res.json();
    _appsCache = data.apps || [];
    renderAppList();
  } catch (e) {
    $("#appList").innerHTML = '<div class="rec-status">加载失败</div>';
  }
}

function renderAppList() {
  const q = _appSearch.toLowerCase();
  const list = $("#appList");
  list.innerHTML = "";
  let shown = 0;
  for (const a of _appsCache) {
    let ok = true;
    if (_appFilter === "third" && !a.thirdParty) ok = false;
    else if (_appFilter === "system" && a.thirdParty) ok = false;
    else if (_appFilter === "running" && !a.running) ok = false;
    if (!ok) continue;
    if (q && !a.package.toLowerCase().includes(q)) continue;
    shown++;
    const row = document.createElement("div");
    row.className = "app-row";
    row.innerHTML = `${esc(a.package)}${a.running ? '<span class="running">●</span>' : ''}`;
    row.addEventListener("click", () => selectApp(a));
    list.appendChild(row);
  }
  if (!shown) list.innerHTML = '<div class="rec-status">没有匹配应用</div>';
}

async function selectApp(a) {
  document.querySelectorAll(".app-row").forEach((r) => r.classList.remove("selected"));
  const idx = _appsCache.indexOf(a);
  if (idx >= 0) {
    const rows = $("#appList").children;
    if (rows[idx]) rows[idx].classList.add("selected");
  }
  const detail = $("#appDetail");
  detail.hidden = false;
  detail.innerHTML = `<div class="pkg-name">${esc(a.package)}</div>` +
    `<div class="app-actions">` +
    `<button data-act="start">▶ 启动</button>` +
    `<button data-act="stop">■ 停止</button>` +
    `<button data-act="clear">清数据</button>` +
    `<button data-act="settings">⚙ 详情</button>` +
    `${a.thirdParty ? '<button data-act="uninstall">🗑 卸载</button>' : ''}` +
    `</div><div class="app-props"></div>`;
  detail.querySelectorAll("button[data-act]").forEach((b) => {
    b.addEventListener("click", () => appAction(b.dataset.act, a.package));
  });
  // Load properties
  try {
    const res = await fetch(`/api/app-props?serial=${encodeURIComponent(currentSerial())}&pkg=${encodeURIComponent(a.package)}`);
    const data = await res.json();
    detail.querySelector(".app-props").textContent = data.text;
  } catch (e) { /* ignore */ }
}

async function appAction(act, pkg) {
  const serial = currentSerial();
  const endpoints = {
    start: ["/api/app/start", { serial, pkg }],
    stop: ["/api/app/stop", { serial, pkg }],
    clear: ["/api/app/clear", { serial, pkg }],
    settings: ["/api/app/settings", { serial, pkg }],
    uninstall: ["/api/app/uninstall", { serial, pkg }],
  };
  const [url, body] = endpoints[act] || [];
  if (!url) return;
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const data = await res.json();
    $("#installResult").textContent = data.text || "";
    if (act === "uninstall") loadAppsPanel();
  } catch (e) {
    $("#installResult").textContent = "操作失败: " + e.message;
  }
}

async function installApk(file) {
  $("#installResult").textContent = "安装中…";
  try {
    const buf = await file.arrayBuffer();
    const res = await fetch(`/api/app/install?serial=${encodeURIComponent(currentSerial())}`, {
      method: "POST",
      headers: { "Content-Type": "application/octet-stream" },
      body: buf,
    });
    const data = await res.json();
    $("#installResult").textContent = data.text || "";
    loadAppsPanel();
  } catch (e) {
    $("#installResult").textContent = "安装失败: " + e.message;
  }
}

async function loadSettingsPanel() {
  const grid = $("#settingsGrid");
  try {
    const res = await fetch("/api/system-settings");
    const data = await res.json();
    grid.innerHTML = "";
    for (const item of data.items) {
      const b = document.createElement("button");
      b.textContent = item.name;
      b.addEventListener("click", () => openSettingsAction(item.action));
      grid.appendChild(b);
    }
  } catch (e) { /* ignore */ }
  loadU2Status();
}

async function openSettingsAction(action) {
  try {
    await fetch("/api/settings-action", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial(), action }),
    });
  } catch (e) { /* ignore */ }
}

async function sendQuickKey(code) {
  try {
    await fetch("/api/input-key", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial(), code: String(code) }),
    });
  } catch (e) { /* ignore */ }
}

async function setBrightness(value) {
  try {
    const res = await fetch("/api/brightness", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial(), value }),
    });
    const data = await res.json();
    $("#settingsResult").textContent = data.text || "";
  } catch (e) { /* ignore */ }
}

async function setAutoBrightness(on) {
  try {
    const res = await fetch("/api/auto-brightness", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial(), on }),
    });
    const data = await res.json();
    $("#settingsResult").textContent = data.text || "";
  } catch (e) { /* ignore */ }
}

function switchLeftTab(tab) {
  document.querySelectorAll(".ltab").forEach((b) =>
    b.classList.toggle("active", b.dataset.ltab === tab));
  $("#propsBody").hidden = tab !== "props";
  $("#tabDevice").hidden = tab !== "device";
  $("#tabApps").hidden = tab !== "apps";
  $("#tabSettings").hidden = tab !== "settings";
  // Hide the left-tabs nav itself until live mode (inspect mode = pure element panel).
  $("#leftTabs").hidden = !live.active;
  if (tab === "device") loadDevicePanel();
  if (tab === "apps" && !_appsCache.length) loadAppsPanel();
  if (tab === "settings") loadSettingsPanel();
}

// --------------------------------------------------------------------------- //
// Tabs
// --------------------------------------------------------------------------- //
// Exactly one display surface is visible at a time: the screenshot in inspect
// mode, the video canvas in live mode. Switching tabs must restore the other
// one, otherwise a stale canvas covers the screenshot and clicks stop mapping.
function applyDisplayMode() {
  const showVideo = live.active && live.connected;
  $("#liveCanvas").hidden = !showVideo;
  $("#screen").hidden = live.active ? true : !state.image;
  const hint = $("#emptyHint");
  if (hint) {
    if (showVideo || state.image) {
      hint.style.display = "none";
    } else if (live.active && !live.connected) {
      hint.textContent = "点击「连接」启动实时会话";
      hint.style.display = "flex";
    } else {
      hint.textContent = "点击「Capture」抓取设备屏幕，或切换到「实时」标签连接设备。";
      hint.style.display = "flex";
    }
  }
}

function switchTab(tab) {
  live.active = tab === "live";
  $("#inspectBar").hidden = live.active;
  $("#liveBar").hidden = !live.active;
  document.querySelectorAll("#tabs .tab").forEach((b) =>
    b.classList.toggle("active", b.dataset.tab === tab));
  if (live.active) {
    $("#viewerTitle").textContent = "实时画面";
    $("#leftTabs").hidden = false;
    $("#propsTitle").textContent = "设备 / 应用";
    $("#recPanel").hidden = false;
    updateLiveChrome();
    applyDisplayMode();
    updateTreeScale();
    if (live.videoW) fitView();
    drawOverlay();
  } else {
    $("#viewerTitle").textContent = "截图";
    $("#leftTabs").hidden = true;
    $("#propsTitle").textContent = "属性";
    $("#recPanel").hidden = true;
    // The on-screen device buttons are operation controls and belong only to
    // the live mode — hide them the moment we leave it (UI查看 shows none).
    updateLiveChrome();
    switchLeftTab("props");
    applyDisplayMode();
    updateTreeScale();
    if (state.image) fitView();
    drawOverlay();
    // A tree grabbed during the live session describes a screen that is no
    // longer the one on display — re-capture so highlights stay accurate.
    if (state.treeFrom === "live" && state.image) doCapture();
  }
}

// --------------------------------------------------------------------------- //
// UI wiring
// --------------------------------------------------------------------------- //
document.querySelectorAll("#tabs .tab").forEach((b) =>
  b.addEventListener("click", () => switchTab(b.dataset.tab)));

document.querySelectorAll(".ltab").forEach((b) =>
  b.addEventListener("click", () => switchLeftTab(b.dataset.ltab)));

$("#refreshDevices").addEventListener("click", () => refreshDevices(false));

$("#device").addEventListener("change", async () => {
  if (live.connected) {
    await liveDisconnect();
    setStatus("设备已切换，请重新点击「连接」");
  }
});

$("#capture").addEventListener("click", doCapture);
$("#saveBtn").addEventListener("click", saveDump);
$("#importBtn").addEventListener("click", () => $("#importImg").click());
$("#importImg").addEventListener("change", () => $("#importXml").click());
$("#importXml").addEventListener("change", () => {
  const img = $("#importImg").files[0];
  const xml = $("#importXml").files[0];
  if (img && xml) doImport(img, xml);
  $("#importImg").value = "";
  $("#importXml").value = "";
});

$("#search").addEventListener("input", (e) => {
  state.search = e.target.value;
  renderTree();
});

// Zoom controls (inspect mode)
$("#zoomIn").addEventListener("click", () => {
  state.zoom = Math.min(4, state.zoom + 0.25);
  updateZoomLabel();
  fitView();
});
$("#zoomOut").addEventListener("click", () => {
  state.zoom = Math.max(0.5, state.zoom - 0.25);
  updateZoomLabel();
  fitView();
});
$("#zoomFit").addEventListener("click", () => {
  state.zoom = 1;
  updateZoomLabel();
  fitView();
});

// Live controls
$("#liveConnect").addEventListener("click", liveConnect);
$("#liveDump").addEventListener("click", liveDump);
$("#liveOverlayChk").addEventListener("change", () => {
  // drawOverlay() clears everything when the box is unchecked, and paints the
  // faint outlines + selection when it is checked.
  drawOverlay();
});
$("#liveKbdChk").addEventListener("change", (e) => {
  live.kbd = e.target.checked;
  setStatus(live.kbd ? "键盘输入已开启" : "键盘输入已关闭");
});
$("#liveText").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  const v = e.target.value.trim();
  if (v && live.connected) {
    sendControl({ type: "text", text: v });
    if (rec.recording) recordStep({ action: "text", text: v });
    e.target.value = "";
  }
});

// Device-style buttons (live mode)
// The controls live inside #viewport, so a click would otherwise bubble up to
// the viewport handler and be forwarded to the device as a tap. Swallow the
// pointer events at the container level (same guarantee as the Rust build,
// where taps only start inside the picture rect).
for (const id of ["#liveSide", "#liveNav"]) {
  const el = $(id);
  if (!el) continue;
  for (const ev of ["mousedown", "mouseup", "wheel", "contextmenu"]) {
    el.addEventListener(ev, (e) => e.stopPropagation());
  }
}

$("#liveEnd").addEventListener("click", () => {
  if (!live.connected) return;
  liveDisconnect();
  setStatus("已结束实时会话");
});

document.querySelectorAll("#liveSide [data-key], #liveNav [data-key]").forEach((b) => {
  b.addEventListener("click", () => {
    if (!live.connected) return;
    const kc = parseInt(b.dataset.key, 10);
    if (!isNaN(kc)) {
      sendKeyTap(kc);
      if (rec.recording) recordStep({ action: "key", keycode: kc, key: b.title });
    }
  });
});

// Settings tab: quick keys + brightness
document.querySelectorAll("#tabSettings [data-key]").forEach((b) => {
  b.addEventListener("click", () => sendQuickKey(b.dataset.key));
});
document.querySelectorAll("#tabSettings [data-brightness]").forEach((b) => {
  b.addEventListener("click", () => setBrightness(parseInt(b.dataset.brightness, 10)));
});
document.querySelectorAll("#tabSettings [data-auto]").forEach((  b) => {
  b.addEventListener("click", () => setAutoBrightness(b.dataset.auto === "1"));
});

// u2 (uiautomator2) acceleration panel
async function loadU2Status() {
  try {
    const res = await fetch("/api/u2/status");
    const d = await res.json();
    const st = $("#u2Status");
    const who = d.serial || "默认设备";
    st.textContent = "状态: " + (d.started ? "已启动 (" + who + ")" : "未启动");
    st.className = "u2-status " + (d.started ? "ok" : "warn");
    const dot = $("#u2Dot");
    if (dot) dot.classList.toggle("on", !!d.started);
    if (d.error) $("#u2Msg").textContent = d.error;
  } catch (e) { /* ignore */ }
}
$("#u2Start").addEventListener("click", async () => {
  const jar = $("#u2Jar").value.trim();
  $("#u2Msg").textContent = "正在推送并启动…";
  try {
    const res = await fetch("/api/u2/start", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial(), jar }),
    });
    const d = await res.json();
    $("#u2Msg").textContent = d.message || (res.ok ? "已启动" : "启动失败");
    await loadU2Status();
  } catch (e) {
    $("#u2Msg").textContent = "启动失败: " + e.message;
  }
});
$("#u2Stop").addEventListener("click", async () => {
  try {
    await fetch("/api/u2/stop", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ serial: currentSerial() }),
    });
    $("#u2Msg").textContent = "已停止";
    await loadU2Status();
  } catch (e) {
    $("#u2Msg").textContent = "停止失败: " + e.message;
  }
});

// Top-bar u2 popover: click the "u2" pill to expand the config, click anywhere
// else to dismiss it.
$("#u2Toggle").addEventListener("click", (e) => {
  e.stopPropagation();
  const p = $("#u2Panel");
  if (!p) return;
  p.hidden = !p.hidden;
  if (!p.hidden) loadU2Status();
});
document.addEventListener("click", (e) => {
  const p = $("#u2Panel");
  const w = $("#u2Widget");
  if (p && !p.hidden && w && !w.contains(e.target)) p.hidden = true;
});

// Apps tab: filter + search + install
document.querySelectorAll(".af").forEach((b) => {
  b.addEventListener("click", () => {
    _appFilter = b.dataset.filter;
    document.querySelectorAll(".af").forEach((x) =>
      x.classList.toggle("active", x.dataset.filter === _appFilter));
    renderAppList();
  });
});
$("#appSearch").addEventListener("input", (e) => {
  _appSearch = e.target.value;
  renderAppList();
});
$("#installBtn").addEventListener("click", () => $("#installFile").click());
$("#installFile").addEventListener("change", () => {
  const f = $("#installFile").files[0];
  if (f) installApk(f);
  $("#installFile").value = "";
});

// Recording controls
$("#recToggle").addEventListener("click", toggleRecording);
$("#recSave").addEventListener("click", saveRecording);
$("#recLoad").addEventListener("click", () => $("#recFile").click());
$("#recFile").addEventListener("change", () => {
  const f = $("#recFile").files[0];
  if (f) loadRecording(f);
  $("#recFile").value = "";
});

function updateLiveChrome() {
  // The side/nav buttons are live-mode operation controls. They must be hidden
  // in UI查看 even if a session is still connected, otherwise they leak into
  // the inspect view and overlay the screenshot.
  const on = live.active && live.connected;
  $("#liveSide").hidden = !on;
  $("#liveNav").hidden = !on;
}

window.addEventListener("resize", () => {
  const { w } = currentViewSize();
  if (w) fitView();
});

// Ensure the initial UI state matches the "UI查看" tab declared in HTML.
switchTab("inspect");
switchLeftTab("props");

setInterval(() => refreshDevices(true), 10000);

refreshDevices();
updateZoomLabel();
loadU2Status();
