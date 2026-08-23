//! Recording & replay of device UI operations.
//!
//! A recording is a list of [`RecordStep`]s. Each step targets a UI element via
//! a [`UiSelector`] resolved against a live UI dump, with optional fractional
//! coordinates (0..1) as a fallback when the element cannot be found. Replay
//! injects the actions through `adb shell input`, which works regardless of
//! whether the live session used scrcpy or raw adb.

use std::os::windows::process::CommandExt as _;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::ui_tree::{parse, Node};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A loose locator for a UI node. Usually only one or two fields are set.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UiSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// One recorded action.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecordStep {
    /// "tap" | "long_tap" | "swipe" | "text" | "key"
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<UiSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_selector: Option<UiSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_selector: Option<UiSelector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fx: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_fx: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_fy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_fx: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_fy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keycode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub ts: f64,
}

impl RecordStep {
    pub fn new(action: &str) -> Self {
        RecordStep {
            action: action.to_string(),
            selector: None,
            from_selector: None,
            to_selector: None,
            fx: None,
            fy: None,
            from_fx: None,
            from_fy: None,
            to_fx: None,
            to_fy: None,
            text: None,
            keycode: None,
            key: None,
            app: None,
            activity: None,
            ts: 0.0,
        }
    }
}

impl RecordStep {
    /// Human-readable summary for the recorded-steps list in the UI.
    pub fn describe(&self) -> String {
        let sel = |s: &Option<UiSelector>| -> String {
            let s = match s {
                Some(s) => s,
                None => return String::new(),
            };
            let mut parts = Vec::new();
            if let Some(r) = &s.resource_id {
                parts.push(format!("id={r}"));
            }
            if let Some(t) = &s.text {
                parts.push(format!("text={t}"));
            }
            if let Some(d) = &s.content_desc {
                parts.push(format!("desc={d}"));
            }
            parts.join(",")
        };
        match self.action.as_str() {
            "tap" => format!("点击 [{}]", sel(&self.selector)),
            "long_tap" => format!("长按 [{}]", sel(&self.selector)),
            "swipe" => {
                format!("滑动 {}→{}", sel(&self.from_selector), sel(&self.to_selector))
            }
            "text" => format!("输入文本 \"{}\"", self.text.as_deref().unwrap_or("")),
            "key" => format!("按键 {}", self.key.as_deref().unwrap_or("")),
            other => other.to_string(),
        }
    }
}

/// Options controlling a replay pass.
pub struct ReplayOpts {
    /// Time scaling: 1.0 = replay at the same pace as recorded, 2.0 =
    /// twice as fast, 0.5 = half speed.
    pub speed: f32,
    /// Number of repetitions. 0 is treated as 1 (a single pass).
    pub loops: u32,
}

/// Build a selector from a node, keeping only the useful identifying attributes.
pub fn build_selector(node: &Node) -> UiSelector {
    let get = |k: &str| node.attrs.get(k).filter(|s| !s.is_empty()).cloned();
    UiSelector {
        resource_id: get("resource-id"),
        text: get("text"),
        content_desc: get("content-desc"),
        class: get("class"),
    }
}

pub fn node_matches(n: &Node, sel: &UiSelector) -> bool {
    if let Some(r) = &sel.resource_id {
        if n.attrs.get("resource-id") != Some(r) {
            return false;
        }
    }
    if let Some(t) = &sel.text {
        if n.attrs.get("text") != Some(t) {
            return false;
        }
    }
    if let Some(d) = &sel.content_desc {
        if n.attrs.get("content-desc") != Some(d) {
            return false;
        }
    }
    if let Some(c) = &sel.class {
        if n.attrs.get("class") != Some(c) {
            return false;
        }
    }
    true
}

/// Find the smallest node matching `sel` and return its centre in device px.
pub fn find_center(tree: &Node, sel: &UiSelector) -> Option<(i32, i32)> {
    fn rec(node: &Node, sel: &UiSelector, best: &mut Option<((i32, i32), i64)>) {
        if node_matches(node, sel) {
            if let Some(b) = node.bounds {
                let center = ((b.left + b.right) / 2, (b.top + b.bottom) / 2);
                let area = b.width() as i64 * b.height() as i64;
                if best.is_none() || area < best.unwrap().1 {
                    *best = Some((center, area));
                }
            }
        }
        for c in &node.children {
            rec(c, sel, best);
        }
    }
    let mut best = None;
    rec(tree, sel, &mut best);
    best.map(|(c, _)| c)
}

pub fn save_yaml(path: &std::path::Path, steps: &[RecordStep]) -> Result<()> {
    let s = serde_yaml::to_string(steps)?;
    std::fs::write(path, s)?;
    Ok(())
}

pub fn load_yaml(path: &std::path::Path) -> Result<Vec<RecordStep>> {
    let s = std::fs::read_to_string(path)?;
    let steps: Vec<RecordStep> = serde_yaml::from_str(&s)
        .map_err(|e| anyhow!("解析录制文件失败: {e}\n(请确认文件是此前本工具保存的 YAML)"))?;
    Ok(steps)
}

fn adb(adb_path: &str, serial: &str) -> Command {
    let mut c = Command::new(adb_path);
    c.creation_flags(CREATE_NO_WINDOW);
    if !serial.is_empty() {
        c.arg("-s").arg(serial);
    }
    c
}

fn adb_input(adb_path: &str, serial: &str, args: &[String]) {
    let mut c = adb(adb_path, serial);
    c.args(["shell", "input"]);
    c.args(args);
    let _ = c.spawn();
}

/// Best-effort device screen size; falls back to 1080x1920.
pub fn screen_size(adb_path: &str, serial: &str) -> (u32, u32) {
    let mut c = adb(adb_path, serial);
    c.args(["shell", "wm", "size"]);
    if let Ok(out) = c.output() {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(rest) = line.split("size:").nth(1) {
                if let Some((w, h)) = rest.trim().split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                        if w > 0 && h > 0 {
                            return (w, h);
                        }
                    }
                }
            }
        }
    }
    (1080, 1920)
}

/// Status messages emitted by [`replay`] so the UI can highlight the active
/// step (green) and any step that failed to resolve (red).
pub enum ReplayMsg {
    /// A step is about to execute. `index` is 0-based into the step list.
    Progress {
        index: usize,
        total: usize,
        loop_idx: usize,
        loops: usize,
        action: String,
    },
    /// A step had a selector but it could not be found on the device, so it was
    /// executed using fractional-coordinate fallback (or skipped).
    Failed { index: usize, text: String },
    /// Non-step status text (e.g. between loops).
    Info(String),
    /// The whole replay finished.
    Done,
}

/// Resolve a selector to device coordinates, re-fetching the UI tree a few times
/// so the action waits for the expected screen. Falls back to fractional coords.
/// The bool is `true` when the selector matched (so callers can flag a failure
/// when an expected element was not found).
///
/// To keep replay fast, once a hierarchy is successfully fetched but the element
/// is still absent we only wait a couple more short intervals (the screen may be
/// mid-transition) and then stop, instead of hammering `tries` full retries.
pub fn resolve(
    adb_path: &str,
    serial: &str,
    u2: Option<&crate::u2::U2>,
    sel: &Option<UiSelector>,
    fx: f32,
    fy: f32,
    size: (u32, u32),
    tries: usize,
) -> (Option<(i32, i32)>, bool) {
    if let Some(sel) = sel {
        for attempt in 0..tries {
            let fetched = crate::u2::fetch_hierarchy(adb_path, serial, u2, 1200)
                .ok()
                .and_then(|xml| parse(&xml).ok());
            match fetched {
                Some(tree) => {
                    if let Some(c) = find_center(&tree, sel) {
                        return (Some(c), true);
                    }
                    // Screen loaded but element not present: a few short retries
                    // cover animations/transitions, then give up.
                    if attempt >= 3 {
                        break;
                    }
                }
                None => {}
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        // Selector existed but never matched: report the fallback coords.
        (Some(((fx * size.0 as f32) as i32, (fy * size.1 as f32) as i32)), false)
    } else {
        (Some(((fx * size.0 as f32) as i32, (fy * size.1 as f32) as i32)), true)
    }
}

/// Replay recorded steps on the device. Progress is reported via `status` so the
/// UI can highlight the active step and any step that failed to resolve.
/// Steps are spaced by the recorded time gaps (scaled by `opts.speed`), and the
/// whole sequence is repeated `opts.loops` times.
pub fn replay(
    adb_path: &str,
    serial: &str,
    u2: Option<&crate::u2::U2>,
    steps: &[RecordStep],
    status: &Sender<ReplayMsg>,
    opts: &ReplayOpts,
) {
    let size = screen_size(adb_path, serial);
    let loops = if opts.loops == 0 { 1 } else { opts.loops };
    let speed = opts.speed.max(0.1);
    for lp in 0..loops {
        for (i, step) in steps.iter().enumerate() {
            // Replay the natural pause before each step (except the first of
            // every loop, where the gap from the previous loop's last step is
            // irrelevant). This keeps the replay faithful to the recording pace.
            if i > 0 {
                let gap = (step.ts - steps[i - 1].ts).max(0.0);
                let wait = (gap / speed as f64).clamp(0.0, 30.0);
                if wait > 0.001 {
                    std::thread::sleep(Duration::from_secs_f64(wait));
                }
            }
            let _ = status.send(ReplayMsg::Progress {
                index: i,
                total: steps.len(),
                loop_idx: lp as usize,
                loops: loops as usize,
                action: step.action.clone(),
            });
            // Tracks whether any expected selector failed to resolve.
            let mut failed = false;
            match step.action.as_str() {
                "tap" | "long_tap" => {
                    let (pt, ok) = resolve(
                        adb_path,
                        serial,
                        u2,
                        &step.selector,
                        step.fx.unwrap_or(0.5),
                        step.fy.unwrap_or(0.5),
                        size,
                        12,
                    );
                    if !ok {
                        failed = true;
                    }
                    if let Some((x, y)) = pt {
                        if step.action == "long_tap" {
                            adb_input(adb_path, serial, &[format!("swipe {x} {y} {x} {y} 600")]);
                        } else {
                            adb_input(adb_path, serial, &[format!("tap {x} {y}")]);
                        }
                    }
                }
                "swipe" => {
                    let (s, ok_s) = resolve(
                        adb_path,
                        serial,
                        u2,
                        &step.from_selector,
                        step.from_fx.unwrap_or(0.5),
                        step.from_fy.unwrap_or(0.5),
                        size,
                        6,
                    );
                    let (e, ok_e) = resolve(
                        adb_path,
                        serial,
                        u2,
                        &step.to_selector,
                        step.to_fx.unwrap_or(0.5),
                        step.to_fy.unwrap_or(0.5),
                        size,
                        6,
                    );
                    if !ok_s || !ok_e {
                        failed = true;
                    }
                    if let (Some((x1, y1)), Some((x2, y2))) = (s, e) {
                        adb_input(adb_path, serial, &[format!("swipe {x1} {y1} {x2} {y2} 200")]);
                    }
                }
                "text" => {
                    let (pt, ok) = resolve(
                        adb_path,
                        serial,
                        u2,
                        &step.selector,
                        step.fx.unwrap_or(0.5),
                        step.fy.unwrap_or(0.5),
                        size,
                        12,
                    );
                    if !ok {
                        failed = true;
                    }
                    if let Some((x, y)) = pt {
                        adb_input(adb_path, serial, &[format!("tap {x} {y}")]);
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    if let Some(t) = &step.text {
                        let escaped = t.replace('%', "%%").replace(' ', "%s");
                        adb_input(adb_path, serial, &[format!("text {escaped}")]);
                    }
                }
                "key" => {
                    if let Some(code) = step.keycode {
                        adb_input(adb_path, serial, &[format!("keyevent {code}")]);
                    }
                }
                _ => {}
            }
            if failed {
                let _ = status.send(ReplayMsg::Failed {
                    index: i,
                    text: "未找到匹配元素，已用坐标回退".to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(600));
        }
        if loops > 1 {
            let _ = status.send(ReplayMsg::Info(format!(
                "第 {} 轮回放完成，准备下一轮…",
                lp + 1
            )));
            std::thread::sleep(Duration::from_millis(800));
        }
    }
    let _ = status.send(ReplayMsg::Done);
}
