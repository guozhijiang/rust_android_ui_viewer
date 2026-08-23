#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui::{
    Align, Color32, ColorImage, FontData, FontDefinitions, FontFamily, Pos2, Rect, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use egui::PointerButton;

use crate::adb::{
    app_properties, capture_serial, clear_app, device_info, dump_ui, dump_ui_serial, force_stop,
    install_apk, list_apps, list_devices, open_app_settings, open_settings_action, start_app,
    AppInfo, CaptureResult, DeviceInfo, SYSTEM_SETTINGS,
};
use crate::live::{self, LiveControl, LiveEvent};
use crate::record::{self, ReplayMsg};
use crate::ui_tree::Node;

const FAINT_NODE_LIMIT: usize = 2000;

/// Compact UTC timestamp for default save-file names, e.g. `20260823_153045`.
/// Avoids pulling in a date crate; good enough for a unique, sortable suffix.
fn file_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (t / 86400) as i64;
    let sod = t % 86400;
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let (y, mo, d) = ymd_from_days(days);
    format!("{y:04}{mo:02}{d:02}_{h:02}{m:02}{s:02}")
}

/// Default location of the on-device u2 server jar, if present. Picking this
/// as the pre-filled path means the user can hit "推送并启动" right away.
/// Only returns a path if the file actually exists (so an empty value prompts
/// the user to pick one explicitly instead of failing the push silently).
fn default_u2_jar() -> String {
    let home = std::env::var_os("USERPROFILE").map(|p| p.to_string_lossy().into_owned());
    let cand = home
        .map(|p| {
            let root = p.trim_end_matches(['\\', '/']);
            format!(r"{root}\.u2\u2_core.jar")
        })
        .unwrap_or_default();
    if std::path::Path::new(&cand).is_file() {
        cand
    } else {
        String::new()
    }
}

fn ymd_from_days(mut days: i64) -> (i64, i64, i64) {
    let mut y = 1970;
    loop {
        let leap = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            366
        } else {
            365
        };
        if days < leap {
            break;
        }
        days -= leap;
        y += 1;
    }
    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mut mo = 0;
    let mut d = days;
    loop {
        let md = month_days[mo] + if mo == 1 && leap { 1 } else { 0 };
        if d < md {
            break;
        }
        d -= md;
        mo += 1;
    }
    (y, mo as i64 + 1, d + 1)
}

/// A press gesture on the live screen being tracked for tap/swipe detection.
struct OpGest {
    /// Device-space coordinates at press.
    start_xy: (i32, i32),
    start_time: Instant,
    moved: bool,
}

/// Map an egui key to an Android KEYCODE. Returns `None` for printable keys
/// with no modifier held, because those are forwarded as text via the
/// control channel (so IME / casing is preserved). With a modifier held the
/// keycode is returned so shortcuts (e.g. Ctrl+C) reach the device.
fn android_keycode(key: egui::Key, has_mod: bool) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        // Non-printable: always a keycode.
        ArrowDown => 20,
        ArrowLeft => 21,
        ArrowRight => 22,
        ArrowUp => 19,
        Escape => 111,
        Tab => 61,
        Backspace => 67,
        Enter => 66,
        Insert => 124,
        Delete => 112,
        Home => 122,
        End => 123,
        PageUp => 92,
        PageDown => 93,
        F1 => 131,
        F2 => 132,
        F3 => 133,
        F4 => 134,
        F5 => 135,
        F6 => 136,
        F7 => 137,
        F8 => 138,
        F9 => 139,
        F10 => 140,
        F11 => 141,
        F12 => 142,
        // Printable: only as a keycode when a modifier is held.
        _ => {
            if !has_mod {
                return None;
            }
            match key {
                A => 29,
                B => 30,
                C => 31,
                D => 32,
                E => 33,
                F => 34,
                G => 35,
                H => 36,
                I => 37,
                J => 38,
                K => 39,
                L => 40,
                M => 41,
                N => 42,
                O => 43,
                P => 44,
                Q => 45,
                R => 46,
                S => 47,
                T => 48,
                U => 49,
                V => 50,
                W => 51,
                X => 52,
                Y => 53,
                Z => 54,
                Num0 => 7,
                Num1 => 8,
                Num2 => 9,
                Num3 => 10,
                Num4 => 11,
                Num5 => 12,
                Num6 => 13,
                Num7 => 14,
                Num8 => 15,
                Num9 => 16,
                _ => return None,
            }
        }
    })
}

/// Android META_* flags derived from the held egui modifiers.
fn android_meta(mods: egui::Modifiers) -> u32 {
    let mut m = 0;
    if mods.shift {
        m |= 0x1; // META_SHIFT_ON
    }
    if mods.alt {
        m |= 0x2; // META_ALT_ON
    }
    if mods.ctrl {
        m |= 0x1000; // META_CTRL_ON
    }
    if mods.command || mods.mac_cmd {
        m |= 0x10000; // META_META_ON
    }
    m
}

pub struct UiViewerApp {
    adb_path: String,
    screenshot: Option<TextureHandle>,
    image_size: Option<(u32, u32)>,
    /// Raw bytes of the last loaded/captured screenshot (for "保存").
    last_screenshot: Option<Vec<u8>>,
    /// Raw XML of the last loaded/captured hierarchy (for "保存").
    last_xml: Option<String>,
    tree: Option<Node>,
    tree_count: usize,
    selected: Option<usize>,
    hovered_tree: Option<usize>,
    hover_pix: Option<(i32, i32)>,
    search: String,
    status: String,
    capturing: bool,
    zoom: f32,
    jump_to: Option<usize>,
    pan: Vec2,
    panning: bool,
    rx: Option<Receiver<anyhow::Result<CaptureResult>>>,

    // ---- Mode: exclusive "view UI" (dump inspection) vs "operate" (live
    // scrcpy). Only one is active at a time, but switching is a single toggle
    // and does not discard the other side's state. ----
    op_mode: bool,
    /// Ensures the view-mode auto-capture fires only once (so a failed capture
    /// doesn't spin up a new background thread every frame).
    auto_captured: bool,
    live_started: bool,
    live_rx: Option<Receiver<LiveEvent>>,
    live_stop: Option<Arc<AtomicBool>>,
    live_tex: Option<TextureHandle>,
    live_size: Option<(u32, u32)>,
    live_control: Option<LiveControl>,
    live_serial: String,
    live_serial_hint: String,
    /// Authorized devices currently seen by `adb devices`. Refreshed when the
    /// user enters live-control mode or clicks the "刷新" button.
    devices: Vec<String>,
    scrcpy_dir: String,
    max_video_size: u32,
    /// Stream quality preset for the live session:
    /// 0 = 清晰(原始分辨率), 1 = 流畅(中), 2 = 极速(低). Controls both
    /// max_size and video bitrate to trade clarity for lower latency/bandwidth.
    quality: u8,
    input_text: String,
    op_gest: Option<OpGest>,
    op_gest2: Option<OpGest>,
    xml_rx: Option<Receiver<anyhow::Result<String>>>,

    // ---- Recording / replay of UI operations ----
    recording: bool,
    steps: Vec<record::RecordStep>,
    last_tap_selector: Option<record::UiSelector>,
    replaying: bool,
    record_rx: Option<Receiver<ReplayMsg>>,
    /// Replay repetitions (0 = single pass).
    replay_loops: u32,
    /// Replay speed multiplier (1.0 = recorded pace, 2.0 = twice as fast).
    replay_speed: f32,
    /// Whether the floating configuration window (adb / device / scrcpy) is open.
    show_config: bool,
    /// Set once the initial configuration (adb path + a target device) is done.
    /// Until then the main features/pages stay disabled behind a setup screen.
    configured: bool,
    /// One-shot flag set when the user finishes config via the "完成配置并进入"
    /// button, so the config window closes exactly once (and can be reopened later).
    config_just_completed: bool,
    /// Throttle for the background hierarchy refresh done while recording so each
    /// gesture resolves to a real element instead of an empty selector.
    last_hier: Option<Instant>,
    /// When set, the next hierarchy result should not overwrite the status line
    /// (used for the quiet background refresh during recording).
    hier_quiet: bool,
    /// Index of the step currently being replayed (for list highlighting).
    replay_current: Option<usize>,
    /// Indices of replay steps that failed to resolve on the device.
    replay_failed: Vec<usize>,

    // ---- Device + app management panel (shown in operate mode) ----
    /// Active tab in the left panel: "设备" | "应用" | "设置".
    pan_tab: String,
    /// Filter for the app list: all | system | third | running.
    app_filter: String,
    app_search: String,
    apps: Vec<AppInfo>,
    device: Option<DeviceInfo>,
    /// Package currently selected (shows its props / action buttons).
    sel_pkg: Option<String>,
    /// Cached text dump of the selected package's properties.
    app_props: Option<String>,
    /// One-shot flag so a refresh only fires once per entry into operate mode.
    panel_loaded: bool,
    /// Receiver for slow panel data produced in the background.
    panel_rx: Option<Receiver<PanelMsg>>,
    /// Text shown under the install button after an install attempt.
    install_result: Option<String>,

    // ---- u2 (uiautomator2 fast-hierarchy) integration ----
    /// Host-side path to the u2 jar the user points at; empty = disabled.
    u2_jar: String,
    /// Live handle to a started u2 server on the device (through adb forward).
    u2: Option<crate::u2::U2>,
    /// Last status text for the u2 connect attempt.
    u2_status: Option<String>,
    /// Result channel for the background u2 connect attempt.
    u2_rx: Option<Receiver<anyhow::Result<bool>>>,
    /// True once we have auto-attempted to start u2 (one try per launch), so a
    /// missing device or transient failure does not trigger repeated attempts.
    u2_auto_attempted: bool,

    // ---- Appearance ----
    /// System dark-mode captured on the first frame (the app follows the OS
    /// theme automatically; not user-configurable).
    system_dark: bool,
    /// Whether `system_dark` has been captured yet.
    system_theme_captured: bool,
    /// Whether the one-time look (theme + density) has been applied. Applying
    /// it every frame allocates a new Style and churns egui's caching, which
    /// measurably stutters the live-stream operate mode — so it runs once.
    look_applied: bool,
}

/// Icons drawn with the painter so they render identically everywhere
/// (no dependency on an emoji font being present).
#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Back,
    Home,
    Recent,
    Power,
    VolUp,
    VolDown,
    End,
}

/// Background results produced for the left device/app-management panel.
enum PanelMsg {
    Device(crate::adb::DeviceInfo),
    Apps(Vec<crate::adb::AppInfo>),
    Props(String),
    Install(String),
}

/// Draw a filled badge with text at `pos` (top-left), used for prominent
/// recording / replaying indicators over the live image.
fn draw_badge(p: &egui::Painter, pos: Pos2, text: &str, color: Color32) {
    let galley = p.ctx().fonts(|f| {
        f.layout_no_wrap(text.to_string(), egui::FontId::proportional(16.0), Color32::WHITE)
    });
    let size = galley.size() + egui::vec2(16.0, 10.0);
    let rect = Rect::from_min_size(pos, size);
    p.rect_filled(rect, 6.0, color);
    p.galley(rect.min + egui::vec2(8.0, 5.0), galley, Color32::WHITE);
}

/// Theme-aware palette driving the "card panels over a canvas" look.
/// A single source of truth so light/dark switch consistently everywhere.
fn c_card(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(27, 27, 32) } else { Color32::from_rgb(255, 255, 255) }
}
fn c_canvas(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(15, 15, 20) } else { Color32::from_rgb(236, 239, 245) }
}
fn c_border(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(50, 50, 58) } else { Color32::from_rgb(216, 221, 231) }
}
fn c_accent(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(130, 150, 255) } else { Color32::from_rgb(86, 106, 248) }
}
fn c_accent_soft(dark: bool) -> Color32 {
    if dark { Color32::from_rgba_unmultiplied(130, 150, 255, 26) } else { Color32::from_rgba_unmultiplied(86, 106, 248, 28) }
}
/// Rounded, bordered "card" frame used for the top bar and side panels.
fn card_frame(dark: bool) -> egui::Frame {
    egui::Frame::none()
        .fill(c_card(dark))
        .rounding(egui::Rounding::same(8.0))
        .stroke(Stroke::new(1.0, c_border(dark)))
        .inner_margin(egui::Margin::same(6.0))
        .outer_margin(egui::Margin::same(2.0))
}

/// Draw a 28px-ish icon centered in `rect`.
fn draw_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let w = s * 0.10;
    let stroke = Stroke::new(w, color);
    let seg = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment(
            [Pos2::new(c.x + a.0 * s, c.y + a.1 * s), Pos2::new(c.x + b.0 * s, c.y + b.1 * s)],
            stroke,
        );
    };
    match icon {
        Icon::Back => {
            // iOS-style back chevron: one clean left-pointing fold with rounded
            // joins/caps. Two arms meet at the left tip and open to the right.
            let at = |nx: f32, ny: f32| Pos2::new(c.x + nx * s, c.y + ny * s);
            let tip = at(-0.42, 0.0);
            p.add(egui::Shape::line(
                vec![at(0.40, -0.28), tip, at(0.40, 0.28)],
                stroke,
            ));
        }
        Icon::Home => {
            // roof
            seg((-0.34, -0.02), (0.0, -0.34));
            seg((0.0, -0.34), (0.34, -0.02));
            // body
            seg((-0.24, -0.02), (-0.24, 0.34));
            seg((0.24, -0.02), (0.24, 0.34));
            seg((-0.24, 0.34), (0.24, 0.34));
            // door
            seg((0.0, 0.06), (0.0, 0.34));
        }
        Icon::Recent => {
            // two overlapping rounded squares (recent apps)
            p.rect_stroke(
                Rect::from_center_size(
                    Pos2::new(c.x - s * 0.08, c.y - s * 0.08),
                    Vec2::new(s * 0.42, s * 0.42),
                ),
                s * 0.06,
                stroke,
            );
            p.rect_stroke(
                Rect::from_center_size(
                    Pos2::new(c.x + s * 0.08, c.y + s * 0.08),
                    Vec2::new(s * 0.42, s * 0.42),
                ),
                s * 0.06,
                stroke,
            );
        }
        Icon::Power => {
            p.circle_stroke(c, s * 0.30, stroke);
            seg((0.0, -0.30), (0.0, -0.46));
        }
        Icon::VolUp | Icon::VolDown => {
            // speaker
            let sx = -0.20;
            p.rect_stroke(
                Rect::from_center_size(
                    Pos2::new(c.x + sx * s - s * 0.10, c.y),
                    Vec2::new(s * 0.14, s * 0.20),
                ),
                s * 0.02,
                stroke,
            );
            seg((sx - 0.03, -0.10), (sx + 0.16, -0.22));
            seg((sx - 0.03, 0.10), (sx + 0.16, 0.22));
            seg((sx + 0.16, -0.22), (sx + 0.16, 0.22));
            // plus / minus
            if icon == Icon::VolUp {
                seg((0.30, 0.0), (0.46, 0.0));
                seg((0.38, -0.08), (0.38, 0.08));
            } else {
                seg((0.30, 0.0), (0.46, 0.0));
            }
        }
        Icon::End => {
            // phone handset (handle + ear/mouth pieces) with end-call slash
            let a = Pos2::new(c.x - s * 0.26, c.y - s * 0.22);
            let b = Pos2::new(c.x + s * 0.26, c.y + s * 0.22);
            p.line_segment([a, b], Stroke::new(s * 0.16, color));
            p.circle_filled(a, s * 0.10, color);
            p.circle_filled(b, s * 0.10, color);
            seg((-0.30, 0.30), (0.30, -0.30));
        }
    }
}

/// A clickable control button positioned at an absolute rect (outside the
/// phone image). Drawn as a premium "glass" circular control: a small, mostly
/// transparent disc with a large crisp icon on top — no heavy panel background.
fn overlay_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, tooltip: &str) -> bool {
    let p = ui.painter();
    let center = rect.center();
    let r = rect.width().min(rect.height()) * 0.42; // disc radius (small footprint)
    let id_key = format!("ovbtn_{:.0}_{:.0}", rect.min.x, rect.min.y);
    let resp = ui.interact(rect, ui.id().with(id_key), egui::Sense::click());
    let hovered = resp.hovered() || resp.has_focus();
    let active = resp.is_pointer_button_down_on();

    // Background: soft glass disc. Transparent at rest, slightly stronger on
    // hover/active for tactile feedback.
    let fill_alpha = if active { 0.34 } else if hovered { 0.22 } else { 0.12 };
    let ring_alpha = if active { 0.55 } else if hovered { 0.45 } else { 0.32 };
    let base = if ui.visuals().dark_mode {
        egui::Color32::from_white_alpha((fill_alpha * 255.0) as u8)
    } else {
        egui::Color32::from_black_alpha((fill_alpha * 255.0) as u8)
    };
    p.add(egui::epaint::CircleShape::filled(
        center,
        r,
        base,
    ));
    p.add(egui::epaint::CircleShape::stroke(
        center,
        r,
        egui::Stroke::new(
            if hovered || active { 1.6 } else { 1.2 },
            egui::Color32::from_white_alpha((ring_alpha * 255.0) as u8),
        ),
    ));

    // Icon: large relative to the disc.
    let icon_size = r * 1.15;
    let icon_box = Rect::from_center_size(center, Vec2::splat(icon_size));
    let icon_color = if ui.visuals().dark_mode {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(40)
    };
    draw_icon(p, icon_box, icon, icon_color);

    let clicked = resp.clicked();
    if !tooltip.is_empty() {
        resp.on_hover_text(tooltip);
    }
    clicked
}

impl UiViewerApp {
    pub fn new() -> Self {
        crate::log::info!("UiViewerApp 已创建");
        let mut this = Self {
            adb_path: "adb".to_string(),
            screenshot: None,
            image_size: None,
            last_screenshot: None,
            last_xml: None,
            tree: None,
            tree_count: 0,
            selected: None,
            hovered_tree: None,
            hover_pix: None,
            search: String::new(),
            status: "就绪。点击 “Capture (adb)” 抓取设备界面，或点击「启动操作会话」实时操作设备；把截图/XML 拖入窗口也可加载。".to_string(),
            capturing: false,
            zoom: 1.0,
            jump_to: None,
            pan: Vec2::ZERO,
            panning: false,
            rx: None,
            op_mode: false,
            auto_captured: false,
            live_started: false,
            live_rx: None,
            live_stop: None,
            live_tex: None,
            live_size: None,
            live_control: None,
            live_serial: String::new(),
            live_serial_hint: String::new(),
            devices: Vec::new(),
            scrcpy_dir: String::new(),
            max_video_size: 0,
            quality: 1,
            input_text: String::new(),
            op_gest: None,
            op_gest2: None,
            xml_rx: None,
            recording: false,
            steps: Vec::new(),
            last_tap_selector: None,
            replaying: false,
            record_rx: None,
            replay_loops: 0,
            replay_speed: 1.0,
            show_config: true,
            configured: false,
            config_just_completed: false,
            last_hier: None,
            hier_quiet: false,
            replay_current: None,
            replay_failed: Vec::new(),
            pan_tab: "设备".to_string(),
            app_filter: "third".to_string(),
            app_search: String::new(),
            apps: Vec::new(),
            device: None,
            sel_pkg: None,
            app_props: None,
            panel_loaded: false,
            panel_rx: None,
            install_result: None,
            u2_jar: default_u2_jar(),
            u2: None,
            u2_status: None,
            u2_rx: None,
            u2_auto_attempted: false,
            system_dark: false,
            system_theme_captured: false,
            look_applied: false,
        };
        // Populate the device list up front so the config window (open by
        // default) immediately shows what `adb devices` reports.
        this.refresh_devices();
        this
    }

    /// Best-effort load of a system CJK font so Chinese text in properties renders.
    pub fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = FontDefinitions::default();
        for path in [
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\simsun.ttc",
            "C:\\Windows\\Fonts\\msjh.ttc",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("cjk".to_string(), FontData::from_owned(bytes));
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "cjk".to_string());
                }
                // The recording-steps list is drawn with the monospace family;
                // without CJK there it would render Chinese as tofu boxes.
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
                    fam.insert(0, "cjk".to_string());
                }
                break;
            }
        }
        ctx.set_fonts(fonts);
    }

    /// Apply the chosen theme (light/dark/follow-system) and UI density on
    /// every frame. "Follow system" re-uses the dark-mode value egui picked at
    /// startup (egui 0.27 exposes no live OS-theme API, so we capture it once).
    fn apply_look(&mut self, ctx: &egui::Context) {
        // Apply once — re-running each frame just re-allocates the Style and
        // forces egui to re-reshape text every frame (stutters the live video).
        if self.look_applied {
            return;
        }
        if !self.system_theme_captured {
            self.system_theme_captured = true;
            self.system_dark = ctx.style().visuals.dark_mode;
        }
        let dark = self.system_dark;
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let accent = c_accent(dark);
        // Text color rendered ON the accent fill (selected/pressed states).
        let on_accent = if dark { Color32::from_rgb(18, 18, 26) } else { Color32::WHITE };
        // Distinct "canvas" base with cards floating on top + brand accent.
        visuals.panel_fill = c_canvas(dark);
        visuals.window_fill = c_card(dark);
        visuals.window_stroke = Stroke::new(1.0, c_border(dark));
        visuals.selection.bg_fill = accent;
        visuals.selection.stroke = Stroke::new(1.0, on_accent);
        visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);
        visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);
        visuals.widgets.active.rounding = egui::Rounding::same(6.0);
        visuals.widgets.hovered.weak_bg_fill = c_accent_soft(dark);
        // Selected/pressed widgets (e.g. the mode switch) get a solid brand-blue
        // fill with a readable, contrasting text color.
        visuals.widgets.active.weak_bg_fill = accent;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, on_accent);
        // Make text-edit fields clearly visible against the white card frame —
        // egui's default field fill is white, which vanishes on the card, so
        // give it a tinted fill plus a defined border (inactive=read-only,
        // active=focused). This covers the operate-mode text input and the
        // config window's ADB/scrcpy path fields.
        let field_bg = if dark { Color32::from_rgb(42, 42, 50) } else { Color32::from_rgb(245, 246, 250) };
        let field_bd = if dark { Color32::from_rgb(72, 72, 84) } else { Color32::from_rgb(178, 183, 194) };
        let field_bd_h = if dark { Color32::from_rgb(92, 92, 106) } else { Color32::from_rgb(150, 155, 168) };
        visuals.extreme_bg_color = field_bg;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, field_bd);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, field_bd_h);
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, field_bd);
        // Focused field gets an accent border for a clear active state.
        visuals.widgets.active.bg_stroke = Stroke::new(1.5, accent);
        let mut style = (*ctx.style()).clone();
        style.visuals = visuals;
        // Fixed "normal" density (not user-configurable).
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(6.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        ctx.set_style(std::sync::Arc::new(style));
        self.look_applied = true;
    }

    fn load_screenshot(&mut self, ctx: &egui::Context, bytes: &[u8]) -> anyhow::Result<()> {
        let img = image::load_from_memory(bytes)
            .map_err(|e| anyhow::anyhow!(e))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let pixels = img.into_raw();
        let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
        let tex = ctx.load_texture("screenshot", color, TextureOptions::default());
        self.screenshot = Some(tex);
        self.image_size = Some((w, h));
        self.last_screenshot = Some(bytes.to_vec());
        Ok(())
    }

    /// The serial to target for left-panel (device/app/settings) operations.
    /// Prefers the connected live session, then the user-selected device, then
    /// the first device on the bus — so the panel still works before/without a
    /// live (scrcpy) session. Empty means "adb default device".
    fn target_serial(&self) -> String {
        if !self.live_serial.is_empty() {
            self.live_serial.clone()
        } else if !self.live_serial_hint.trim().is_empty() {
            self.live_serial_hint.clone()
        } else {
            self.devices.first().cloned().unwrap_or_default()
        }
    }

    /// Refresh the list of connected, authorized devices via `adb devices`.
    fn refresh_devices(&mut self) {
        match list_devices(&self.adb_path) {
            Ok(d) => {
                self.devices = d;
                if self.devices.is_empty() {
                    self.status = "未检测到已连接设备（请确认 adb 已授权连接）。".to_string();
                }
            }
            Err(e) => self.status = format!("刷新设备列表失败：{e}"),
        }
    }

    /// Whether the essentials for using the app are configured: an ADB path
    /// and a target device (either already selected or detected on the bus).
    fn config_ready(&self) -> bool {
        !self.adb_path.trim().is_empty()
            && (!self.live_serial_hint.trim().is_empty() || !self.devices.is_empty())
    }

    fn start_capture(&mut self) {
        self.capturing = true;
        self.status = "正在抓取设备界面…".to_string();
        let adb = self.adb_path.clone();
        // Target the device chosen in live-control mode; empty = default device.
        let serial = self.live_serial_hint.clone();
        let u2 = self.u2.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<CaptureResult> {
                let screenshot = capture_serial(&adb, &serial)?;
                // Prefer the u2 fast hierarchy when available; it works on
                // devices where `uiautomator dump` does not (e.g. Android 16).
                let xml = match u2.as_ref() {
                    Some(srv) => crate::u2::fetch_hierarchy(&adb, &serial, Some(srv), 1600)?,
                    None if serial.is_empty() => dump_ui(&adb)?,
                    None => dump_ui_serial(&adb, &serial)?,
                };
                Ok(CaptureResult { screenshot, xml })
            })();
            let _ = tx.send(res);
        });
    }

    fn load_xml(&mut self, xml: &str) {
        self.last_xml = Some(xml.to_string());
        match crate::ui_tree::parse(xml) {
            Ok(tree) => {
                self.tree_count = tree.count();
                self.tree = Some(tree);
                self.selected = None;
                self.jump_to = None;
                self.status = format!("已加载层级，共 {} 个节点。", self.tree_count);
            }
            Err(e) => self.status = format!("XML 解析失败：{e}"),
        }
    }

    /// Save the current screenshot + hierarchy to `<name>.png` and `<name>.xml`.
    fn save_dump(&mut self) {
        let (bytes, xml) = match (self.last_screenshot.as_ref(), self.last_xml.as_ref()) {
            (Some(b), Some(x)) => (b, x),
            _ => {
                self.status = "没有可保存的截图/XML（请先抓取或加载）。".to_string();
                return;
            }
        };
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("dump")
            .save_file()
        {
            let img_path = path.with_extension("png");
            let xml_path = path.with_extension("xml");
            let mut ok = true;
            if let Err(e) = std::fs::write(&img_path, bytes) {
                self.status = format!("保存截图失败：{e}");
                ok = false;
            }
            if let Err(e) = std::fs::write(&xml_path, xml) {
                self.status = format!("保存 XML 失败：{e}");
                ok = false;
            }
            if ok {
                self.status = format!(
                    "已保存：{} 和 {}",
                    img_path.display(),
                    xml_path.display()
                );
            }
        }
    }

    /// Load a screenshot and its matching XML hierarchy in a single dialog.
    /// Both files can be selected at once (in any order); if only the image is
    /// chosen we also auto-load a sibling `<same-name>.xml` from its folder.
    fn load_dump(&mut self, ctx: &egui::Context) {
        let files = rfd::FileDialog::new()
            .add_filter("截图与层级", &["png", "jpg", "jpeg", "xml"])
            .pick_files();
        let (img_path, xml_path) = match files {
            Some(fs) => {
                let mut img = None;
                let mut xml = None;
                for f in &fs {
                    let p = f;
                    let ext = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                        if img.is_none() {
                            img = Some(p.to_path_buf());
                        }
                    } else if ext == "xml" {
                        if xml.is_none() {
                            xml = Some(p.to_path_buf());
                        }
                    }
                }
                (img, xml)
            }
            None => return,
        };
        if let Some(path) = img_path {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if let Err(e) = self.load_screenshot(ctx, &bytes) {
                        self.status = format!("图片加载失败：{e}");
                    } else {
                        self.status = format!("已加载截图：{}", path.display());
                    }
                }
                Err(e) => self.status = format!("读取图片失败：{e}"),
            }
            // Auto-pick the sibling XML next to the screenshot when it wasn't
            // explicitly chosen.
            if xml_path.is_none() {
                let sibling = path.with_extension("xml");
                if sibling.exists() {
                    if let Ok(s) = std::fs::read_to_string(&sibling) {
                        self.load_xml(&s);
                    }
                }
            }
        }
        if let Some(path) = xml_path {
            match std::fs::read_to_string(&path) {
                Ok(s) => self.load_xml(&s),
                Err(e) => self.status = format!("读取 XML 失败：{e}"),
            }
        }
    }

    /// Spawn the live scrcpy session (video stream + touch control).
    fn start_live(&mut self) {
        // Quality preset → (max_size, video_bitrate in kbps).
        // Lower resolution + bitrate reduces transport size and latency, which
        // is the main cause of sluggish on-device response over adb.
        let (max_size, bitrate) = match self.quality {
            0 => (self.max_video_size, 0), // 清晰：原始分辨率，不限制码率
            1 => (1024, 4_000_000),        // 流畅：最长边 1024，~4 Mbps
            _ => (720, 2_000_000),         // 极速：最长边 720，~2 Mbps
        };
        crate::log::info!(
            "启动操作会话: adb={}, serial_hint={:?}, scrcpy_dir={:?}, quality={}, max_size={}, bitrate={}",
            self.adb_path,
            self.live_serial_hint,
            self.scrcpy_dir,
            self.quality,
            max_size,
            bitrate
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        live::start(
            self.adb_path.clone(),
            self.live_serial_hint.clone(),
            self.scrcpy_dir.clone(),
            max_size,
            bitrate,
            stop.clone(),
            tx,
        );
        self.live_stop = Some(stop);
        self.live_rx = Some(rx);
        self.live_started = true;
        self.status = "已启动操作会话…".to_string();
    }

    /// Tear the live session down (the worker thread observes the stop flag).
    fn stop_live(&mut self) {
        crate::log::info!("结束操作会话");
        if let Some(stop) = &self.live_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.live_stop = None;
        self.live_rx = None;
        self.live_tex = None;
        self.live_size = None;
        self.live_control = None;
        self.live_serial.clear();
        self.live_started = false;
        self.op_gest = None;
        self.op_gest2 = None;
    }

    /// Fire-and-forget `adb shell <cmd>` against the live device.
    fn adb_sh(&self, cmd: &str) {
        let mut c = Command::new(&self.adb_path);
        #[cfg(windows)]
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: don't pop a cmd box
        if !self.live_serial.is_empty() {
            c.arg("-s").arg(&self.live_serial);
        }
        let _ = c
            .args(["shell", cmd])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    fn send_key(&self, code: u32) {
        crate::log::debug!("发送按键 code={}", code);
        if let Some(c) = &self.live_control {
            c.press_key(code);
        } else {
            self.adb_sh(&format!("input keyevent {code}"));
        }
    }

    fn start_replay(&mut self, steps: Vec<record::RecordStep>) {
        if self.replaying || self.live_serial.is_empty() {
            self.status = if self.live_serial.is_empty() {
                "回放需要先启动操作会话并连接设备。".to_string()
            } else {
                "回放已在进行中。".to_string()
            };
            return;
        }
        // Show the exact steps being replayed in the list (covers the case where
        // they were loaded from a file rather than recorded this session).
        self.steps = steps.clone();
        self.replaying = true;
        self.replay_current = None;
        self.replay_failed.clear();
        let adb = self.adb_path.clone();
        let serial = self.live_serial.clone();
        let u2 = self.u2.clone();
        let opts = record::ReplayOpts {
            speed: self.replay_speed,
            loops: self.replay_loops,
        };
        let (tx, rx) = std::sync::mpsc::channel::<ReplayMsg>();
        self.record_rx = Some(rx);
        std::thread::spawn(move || {
            record::replay(&adb, &serial, u2.as_ref(), &steps, &tx, &opts);
        });
        self.status = "回放中…".to_string();
    }

    fn send_text(&self, text: &str) {
        crate::log::debug!("发送文本 ({} 字符)", text.chars().count());
        if let Some(c) = &self.live_control {
            c.text(text);
        } else {
            // `input text` needs spaces/% escaped; keep it simple and reliable.
            let escaped = text.replace('%', "%%").replace(' ', "%s");
            self.adb_sh(&format!("input text {escaped}"));
        }
    }

    // ---- Recording helpers ----
    fn now_ts() -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn record_step(&mut self, mut step: record::RecordStep) {
        if !self.recording {
            return;
        }
        if let Some((pkg, act)) = crate::adb::current_app(&self.adb_path, &self.live_serial) {
            step.app = Some(pkg);
            step.activity = Some(act);
        }
        step.ts = Self::now_ts();
        self.steps.push(step);
    }

    fn node_at(&self, x: i32, y: i32) -> Option<record::UiSelector> {
        self.tree
            .as_ref()
            .and_then(|t| t.hit_test(x, y).and_then(|id| t.find(id)))
            .map(record::build_selector)
    }

    fn record_tap(&mut self, x: i32, y: i32, long: bool) {
        if !self.recording {
            return;
        }
        let sel = self.node_at(x, y);
        self.last_tap_selector = sel.clone();
        let (w, h) = self.live_size.unwrap_or((0, 0));
        let mut s = record::RecordStep::new(if long { "long_tap" } else { "tap" });
        s.selector = sel;
        if w > 0 {
            s.fx = Some(x as f32 / w as f32);
            s.fy = Some(y as f32 / h as f32);
        }
        self.record_step(s);
    }

    fn record_swipe(&mut self, sx: i32, sy: i32, ex: i32, ey: i32) {
        if !self.recording {
            return;
        }
        let (w, h) = self.live_size.unwrap_or((0, 0));
        let mut s = record::RecordStep::new("swipe");
        s.from_selector = self.node_at(sx, sy);
        s.to_selector = self.node_at(ex, ey);
        if w > 0 {
            s.from_fx = Some(sx as f32 / w as f32);
            s.from_fy = Some(sy as f32 / h as f32);
            s.to_fx = Some(ex as f32 / w as f32);
            s.to_fy = Some(ey as f32 / h as f32);
        }
        self.record_step(s);
    }

    fn record_text(&mut self, text: &str) {
        if !self.recording {
            return;
        }
        let mut s = record::RecordStep::new("text");
        s.text = Some(text.to_string());
        s.selector = self.last_tap_selector.clone();
        self.record_step(s);
    }

    fn record_key(&mut self, code: u32, label: &str) {
        if !self.recording {
            return;
        }
        let mut s = record::RecordStep::new("key");
        s.keycode = Some(code);
        s.key = Some(label.to_string());
        self.record_step(s);
    }

    /// Dump + load the UI hierarchy while the live session keeps running.
    fn capture_hierarchy_now(&mut self) {
        crate::log::info!("抓取界面层级 serial={:?}", self.live_serial);
        self.status = "正在抓取界面层级…".to_string();
        self.hier_quiet = false;
        self.spawn_hierarchy_dump();
    }

    /// Same as `capture_hierarchy_now` but without touching the status line, for
    /// the periodic background refresh done while recording.
    fn refresh_hierarchy_quiet(&mut self) {
        self.hier_quiet = true;
        self.spawn_hierarchy_dump();
    }

    fn spawn_hierarchy_dump(&mut self) {
        let adb = self.adb_path.clone();
        let serial = self.live_serial.clone();
        let u2 = self.u2.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.xml_rx = Some(rx);
        let u2_on = u2.is_some();
        std::thread::spawn(move || {
            let res = if let Some(srv) = u2.as_ref() {
                match crate::u2::fetch_hierarchy(&adb, &serial, Some(srv), 1500) {
                    Ok(x) => {
                        crate::log::info!("dump: u2 OK len={}", x.len());
                        Ok(x)
                    }
                    Err(e) => {
                        crate::log::warn!("dump: u2 failed ({e}); falling back to adb dump");
                        if serial.is_empty() {
                            dump_ui(&adb)
                        } else {
                            dump_ui_serial(&adb, &serial)
                        }
                    }
                }
            } else {
                crate::log::warn!("dump: u2 NOT available (u2_on={u2_on}); using adb dump");
                if serial.is_empty() {
                    dump_ui(&adb)
                } else {
                    dump_ui_serial(&adb, &serial)
                }
            };
            let _ = tx.send(res);
        });
    }

    // ---- Left panel: device / app management (background refresh) ----
    /// Kick off background fetches for device info + the full app list.
    fn panel_refresh(&mut self) {
        let adb = self.adb_path.clone();
        let serial = self.target_serial();
        let (tx, rx) = std::sync::mpsc::channel::<PanelMsg>();
        self.panel_rx = Some(rx);
        let t = tx.clone();
        let (adb2, serial2) = (adb.clone(), serial.clone());
        std::thread::spawn(move || {
            let d = device_info(&adb, &serial);
            let _ = t.send(PanelMsg::Device(d));
        });
        std::thread::spawn(move || {
            let apps = list_apps(&adb2, &serial2, "all");
            let _ = tx.send(PanelMsg::Apps(apps));
        });
    }

    /// Fetch a single package's properties in the background.
    fn panel_props(&mut self, pkg: String) {
        let adb = self.adb_path.clone();
        let serial = self.target_serial();
        let (tx, rx) = std::sync::mpsc::channel::<PanelMsg>();
        self.panel_rx = Some(rx);
        std::thread::spawn(move || {
            let props = app_properties(&adb, &serial, &pkg);
            let _ = tx.send(PanelMsg::Props(props));
        });
    }

    /// Kick off a background attempt to push + start the on-device u2 server.
    fn try_connect_u2(&mut self) {
        let jar = self.u2_jar.clone();
        let adb = self.adb_path.clone();
        let serial = self.target_serial();
        let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<bool>>();
        self.u2_rx = Some(rx);
        self.u2_status = Some("正在推送并启动 u2（可能需要几秒）…".to_string());
        std::thread::spawn(move || {
            let res = crate::u2::start(&adb, &serial, &jar, crate::u2::DEFAULT_PORT);
            let _ = tx.send(res);
        });
    }

    /// Install the selected APK in the background, reporting the result via the
    /// panel channel so the UI thread never blocks on a long `adb install -r`.
    fn panel_install_apk(&mut self, path: String) {
        self.install_result = Some("正在安装…".to_string());
        let adb = self.adb_path.clone();
        let serial = self.target_serial();
        let (tx, rx) = std::sync::mpsc::channel::<PanelMsg>();
        self.panel_rx = Some(rx);
        std::thread::spawn(move || {
            let result = install_apk(&adb, &serial, &path)
                .unwrap_or_else(|e| format!("安装失败: {e}"));
            let _ = tx.send(PanelMsg::Install(result));
        });
    }

    // ---- Left-panel UI (operate mode): device info / apps / settings ----
    fn render_dev_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.pan_tab, "设备".to_string(), "设备信息");
            ui.selectable_value(&mut self.pan_tab, "应用".to_string(), "应用");
            ui.selectable_value(&mut self.pan_tab, "设置".to_string(), "系统设置");
        });
        ui.add_space(4.0);
        ui.separator();
        match self.pan_tab.as_str() {
            "设备" => self.render_device_tab(ui),
            "应用" => self.render_apps_tab(ui),
            _ => self.render_settings_tab(ui),
        }
    }

    fn render_device_tab(&mut self, ui: &mut egui::Ui) {
        if let Some(d) = &self.device {
            egui::Grid::new("dev_info")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.strong(k);
                        ui.label(v);
                        ui.end_row();
                    };
                    row("机型", d.model.clone());
                    row("品牌", d.brand.clone());
                    row(
                        "系统",
                        if d.android.is_empty() {
                            "?".to_string()
                        } else {
                            format!("Android {} (SDK {})", d.android, d.sdk)
                        },
                    );
                    row("分辨率", d.resolution.clone());
                    row("DPI", d.density.clone());
                    row("电量", d.battery.clone());
                    if !d.serial.is_empty() {
                        row("序列号", d.serial.clone());
                    }
                });
        } else {
            let msg = if self.panel_rx.is_some() {
                "设备信息加载中…"
            } else {
                "设备信息为空（请先连接设备并点击刷新）"
            };
            ui.label(msg);
        }
        ui.add_space(8.0);
        ui.separator();
        ui.strong("安装 APK");
        ui.add_space(2.0);
        if ui.button("选择 APK 并安装").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("APK", &["apk"])
                .pick_file()
            {
                self.panel_install_apk(path.display().to_string());
            }
        }
        if let Some(r) = &self.install_result {
            ui.add_space(2.0);
            ui.label(r);
        }
    }

    fn render_apps_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.app_filter, "third".to_string(), "三方");
            ui.selectable_value(&mut self.app_filter, "all".to_string(), "全部");
            ui.selectable_value(&mut self.app_filter, "system".to_string(), "系统");
            ui.selectable_value(&mut self.app_filter, "running".to_string(), "运行中");
        });
        ui.add_space(2.0);
        ui.add(egui::TextEdit::singleline(&mut self.app_search).hint_text("搜索包名…"));
        ui.add_space(4.0);

        let q = self.app_search.to_lowercase();
        let filter = self.app_filter.clone();
        let mut shown = 0usize;
        let mut click: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_source("app_list")
            .auto_shrink([false, false])
            .max_height(300.0)
            .show(ui, |ui| {
                for a in &self.apps {
                    let ok = match filter.as_str() {
                        "third" => a.third_party,
                        "system" => !a.third_party,
                        "running" => a.running,
                        _ => true,
                    };
                    if !ok {
                        continue;
                    }
                    if !q.is_empty() && !a.package.to_lowercase().contains(&q) {
                        continue;
                    }
                    shown += 1;
                    let selected = self.sel_pkg.as_deref() == Some(a.package.as_str());
                    let text = if a.running {
                        format!("{}  ●", a.package)
                    } else {
                        a.package.clone()
                    };
                    if ui.selectable_label(selected, text).clicked() {
                        click = Some(a.package.clone());
                    }
                }
                if shown == 0 {
                    ui.label("没有匹配的应用。");
                }
            });
        if let Some(pkg) = click {
            self.sel_pkg = Some(pkg.clone());
            self.app_props = None;
            self.panel_props(pkg);
        }
        ui.label(format!("本屏 {} 个应用", shown));

        if let Some(pkg) = self.sel_pkg.clone() {
            let serial = self.target_serial();
            ui.separator();
            ui.strong(&pkg);
            ui.horizontal_wrapped(|ui| {
                if ui.button("▶ 启动").clicked() {
                    start_app(&self.adb_path, &serial, &pkg);
                }
                if ui.button("■ 停止").clicked() {
                    force_stop(&self.adb_path, &serial, &pkg);
                }
                if ui.button("清数据").clicked() {
                    let r = clear_app(&self.adb_path, &serial, &pkg);
                    self.install_result = Some(r);
                }
                if ui.button("⚙ 详情").clicked() {
                    open_app_settings(&self.adb_path, &serial, &pkg);
                }
            });
            if let Some(props) = &self.app_props {
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("app_props")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        let dark = ui.visuals().dark_mode;
                        ui.label(
                            egui::RichText::new(props.clone())
                                .monospace()
                                .color(if dark {
                                    Color32::from_rgb(200, 205, 220)
                                } else {
                                    Color32::from_rgb(30, 33, 42)
                                }),
                        );
                    });
            }
        }
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        let serial = self.target_serial();
        for (name, action) in SYSTEM_SETTINGS {
            if ui.button(*name).clicked() {
                open_settings_action(&self.adb_path, &serial, action);
            }
        }
    }
}

impl eframe::App for UiViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme + density first so every panel below uses the chosen look.
        self.apply_look(ctx);
        // Consume a pending "jump to selected node" request (set on the previous
        // frame when an element was selected). `jump` drives this frame's scroll.
        let jump = self.jump_to.take();

        // In "查看 UI" mode, auto-capture the screen + hierarchy on first entry
        // (and whenever nothing is loaded yet) so inspection works immediately
        // without a separate Capture click. Gated on being configured first.
        // Auto-start u2 once on launch if a jar is configured, so capture works
        // even on devices where `uiautomator dump` is unavailable (Android 16).
        if self.configured
            && self.u2.is_none()
            && !self.u2_auto_attempted
            && self.u2_rx.is_none()
            && !self.u2_jar.trim().is_empty()
        {
            self.u2_auto_attempted = true;
            self.try_connect_u2();
        }

        if self.configured
            && !self.op_mode
            && self.tree.is_none()
            && !self.capturing
            && !self.auto_captured
            && self.u2_rx.is_none() // wait for the auto u2 attempt to settle
        {
            self.auto_captured = true;
            self.start_capture();
        }

        // Tree-hover highlight is recomputed every frame.
        self.hovered_tree = None;

        // Drain an in-flight capture result (produced by the background thread).
        if let Some(rx) = &self.rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(cap) => {
                        if let Err(e) = self.load_screenshot(ctx, &cap.screenshot) {
                            self.status = format!("截图加载失败：{e}");
                        }
                        self.load_xml(&cap.xml);
                    }
                    Err(e) => self.status = format!("抓取失败：{e}"),
                }
                self.rx = None;
                self.capturing = false;
                ctx.request_repaint();
            }
        }

        // Drain a pending hierarchy-only dump (op mode "grab hierarchy" or the
        // background refresh done while recording). The quiet refresh (started by
        // the recording auto-refresh) must not overwrite the status line.
        if let Some(rx) = &self.xml_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(xml) => self.load_xml(&xml),
                    Err(e) => self.status = format!("层级抓取失败：{e}"),
                }
                self.xml_rx = None;
                if !self.hier_quiet {
                    self.status =
                        format!("{}  点击界面可操作；Ctrl+点击可选中元素。", self.status);
                }
                ctx.request_repaint();
            }
        }

        // Drain left-panel background results (device info / app list / props).
        if self.panel_rx.is_some() {
            let mut drained = false;
            while !drained {
                match self.panel_rx.as_ref().unwrap().try_recv() {
                    Ok(PanelMsg::Device(d)) => self.device = Some(d),
                    Ok(PanelMsg::Apps(a)) => self.apps = a,
                    Ok(PanelMsg::Props(p)) => self.app_props = Some(p),
                    Ok(PanelMsg::Install(r)) => self.install_result = Some(r),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                    | Err(std::sync::mpsc::TryRecvError::Disconnected) => drained = true,
                }
            }
            self.panel_rx = None;
            ctx.request_repaint();
        }

        // Drain a background u2 connect attempt result.
        if let Some(rx) = &self.u2_rx {
            if let Ok(res) = rx.try_recv() {
                use crate::u2::U2;
                match res {
                    Ok(true) => {
                        self.u2 = Some(U2::new(crate::u2::DEFAULT_PORT));
                        self.u2_status = Some("✓ u2 已就绪（快速抓树已启用）".to_string());
                    }
                    Ok(false) => {
                        self.u2_status = Some("u2 服务未响应，已回退使用 uiautomator dump".to_string());
                    }
                    Err(e) => {
                        self.u2_status = Some(format!("u2 启动失败: {e}"));
                    }
                }
                self.u2_rx = None;
                ctx.request_repaint();
            }
        }

        // Load the device/app panel once when entering operate mode (works as
        // soon as a device is available, before/without a live session).
        if self.op_mode && !self.panel_loaded && !self.target_serial().is_empty() {
            self.panel_loaded = true;
            self.panel_refresh();
        }

        // While recording, keep the hierarchy fresh in the background so each
        // gesture resolves to a real element (text / resource-id) instead of an
        // empty selector. Throttled to ~1s and skipped while a dump is in flight.
        if self.recording && self.live_started && self.xml_rx.is_none() {
            let need = match self.last_hier {
                None => true,
                Some(t) => t.elapsed() > Duration::from_millis(1000),
            };
            if need {
                self.last_hier = Some(Instant::now());
                self.refresh_hierarchy_quiet();
            }
        }

        // Drain replay progress messages (and remember the active/failed steps).
        let keep_rx = if let Some(rx) = self.record_rx.take() {
            let mut finished = false;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ReplayMsg::Progress {
                        index,
                        total,
                        loop_idx,
                        loops,
                        action,
                    } => {
                        self.replay_current = Some(index);
                        self.status = format!(
                            "回放 {}/{} (第 {}/{} 轮): {}",
                            index + 1,
                            total,
                            loop_idx + 1,
                            loops,
                            action
                        );
                    }
                    ReplayMsg::Failed { index, text } => {
                        self.replay_current = Some(index);
                        if !self.replay_failed.contains(&index) {
                            self.replay_failed.push(index);
                        }
                        self.status = format!("回放步骤 {} 失败：{}", index + 1, text);
                    }
                    ReplayMsg::Info(s) => {
                        self.status = s;
                    }
                    ReplayMsg::Done => {
                        finished = true;
                    }
                }
            }
            if finished {
                self.replaying = false;
                self.replay_current = None;
                let failed = self.replay_failed.len();
                self.status = if failed == 0 {
                    "回放完成。".to_string()
                } else {
                    format!("回放完成，其中 {} 个步骤未找到匹配元素。", failed)
                };
                None
            } else {
                Some(rx)
            }
        } else {
            None
        };
        if let Some(rx) = keep_rx {
            self.record_rx = Some(rx);
        }

        // Drain live-session events (frames, status, errors).
        let live_events: Vec<LiveEvent> = self
            .live_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for ev in live_events {
            match ev {
                LiveEvent::Connected {
                    width,
                    height,
                    device_name,
                    serial,
                    control,
                } => {
                    self.live_size = Some((width, height));
                    self.live_control = control;
                    let ctrl = if self.live_control.is_some() {
                        "实时控制已就绪"
                    } else {
                        "控制通道不可用，已回退 adb"
                    };
                    crate::log::info!(
                        "已连接设备 {} ({}x{}) serial={:?} {}",
                        device_name,
                        width,
                        height,
                        serial,
                        ctrl
                    );
                    if !serial.is_empty() {
                        self.live_serial = serial;
                    }
                    self.status = format!("已连接 {device_name}（{width}x{height}） · {ctrl}");
                }
                LiveEvent::Status(s) => {
                    crate::log::debug!("会话状态: {}", s);
                    self.status = s
                }
                LiveEvent::Frame(f) => {
                    let color = ColorImage::from_rgba_unmultiplied(
                        [f.width as usize, f.height as usize],
                        &f.rgba,
                    );
                    let tex = ctx.load_texture("live_frame", color, TextureOptions::default());
                    self.live_tex = Some(tex);
                    self.live_size = Some((f.width, f.height));
                }
                LiveEvent::Error(e) => {
                    crate::log::error!("操作会话错误: {}", e);
                    self.status = format!("操作会话错误：{e}");
                    self.stop_live();
                    ctx.request_repaint();
                }
                LiveEvent::Stopped => {
                    crate::log::info!("操作会话已停止");
                    self.stop_live();
                    self.status = "操作会话已结束".to_string();
                    ctx.request_repaint();
                }
            }
        }
        if self.live_started {
            // Keep the UI repainting at (roughly) the stream rate.
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        // Drag & drop: image -> screenshot, xml -> hierarchy.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in &dropped {
            if let Some(path) = &f.path {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ext == "png" || ext == "jpg" || ext == "jpeg" {
                    match std::fs::read(path) {
                        Ok(bytes) => {
                            if let Err(e) = self.load_screenshot(ctx, &bytes) {
                                self.status = format!("图片加载失败：{e}");
                            } else {
                                self.status = format!("已加载截图：{}", path.display());
                            }
                        }
                        Err(e) => self.status = format!("读取图片失败：{e}"),
                    }
                } else if ext == "xml" {
                    match std::fs::read_to_string(path) {
                        Ok(s) => self.load_xml(&s),
                        Err(e) => self.status = format!("读取 XML 失败：{e}"),
                    }
                }
            }
        }

        // ---- Top panel: actions + config toggle + status ----
        egui::TopBottomPanel::top("top")
            .frame(card_frame(ctx.style().visuals.dark_mode))
            .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                // Grabbing is automatic when entering "查看 UI", so the top bar
                // only needs Save (keep screenshot + XML) and Load (screenshot + XML).
                if ui.button("保存截图和XML").clicked() {
                    self.save_dump();
                }
                if ui.button("加载截图和XML").clicked() {
                    self.load_dump(ctx);
                }
                ui.separator();
                if ui.button("⚙ 配置").clicked() {
                    self.show_config = !self.show_config;
                }
                if self.capturing {
                    ui.spinner();
                    ui.label("抓取中…");
                }
            });
            ui.horizontal_wrapped(|ui| {
                if let Some((x, y)) = self.hover_pix {
                    ui.monospace(format!("坐标: ({x}, {y})"));
                }
                ui.separator();
                let status_color = if self.status.contains("失败")
                    || self.status.contains("错误")
                    || self.status.contains("请")
                    || self.status.contains("无法")
                    || self.status.contains("为空")
                    || self.status.contains("需要")
                {
                    Color32::from_rgb(240, 130, 130)
                } else {
                    Color32::from_rgb(220, 220, 220)
                };
                ui.colored_label(status_color, &self.status);
                // Connection badge (operate mode only).
                if self.op_mode {
                    ui.separator();
                    if self.live_started {
                        ui.colored_label(Color32::from_rgb(90, 200, 120), "● 已连接")
                            .on_hover_text("操作会话已连接（实时控制或已回退 adb）");
                    } else {
                        ui.colored_label(Color32::from_rgb(228, 150, 140), "○ 未连接")
                            .on_hover_text("操作会话正在建立或已结束");
                    }
                }
                // Replay progress when a replay is running.
                if self.replaying {
                    if let Some(idx) = self.replay_current {
                        let n = self.steps.len().max(1);
                        ui.separator();
                        ui.colored_label(
                            Color32::from_rgb(230, 160, 60),
                            format!("回放 {}/{}", idx + 1, n),
                        );
                        ui.add(
                            egui::ProgressBar::new(((idx + 1) as f32 / n as f32).clamp(0.0, 1.0))
                                .desired_width(140.0),
                        );
                    }
                }
            });
        });

        // ---- Floating configuration window: ADB path, device selection,
        // scrcpy options. Consolidates everything that was previously spread
        // across the top/bottom bars so the main UI stays focused on the task. ----
        if self.show_config {
            let mut cfg_open = self.show_config;
            egui::Window::new("⚙ 配置")
                .open(&mut cfg_open)
                .default_width(360.0)
                .show(ctx, |ui| {
                    ui.label("ADB 路径:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.adb_path);
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("adb", &["exe"])
                                .pick_file()
                            {
                                self.adb_path = p.display().to_string();
                            }
                        }
                    });
                    ui.separator();
                    ui.label("设备:");
                    let sel_text = if self.live_serial_hint.is_empty() {
                        "<选择设备>".to_string()
                    } else {
                        self.live_serial_hint.clone()
                    };
                    egui::ComboBox::from_id_source("dev_pick")
                        .selected_text(sel_text)
                        .show_ui(ui, |ui| {
                            if self.devices.is_empty() {
                                ui.label("（无设备，点刷新）");
                            }
                            for d in self.devices.clone() {
                                ui.selectable_value(&mut self.live_serial_hint, d.clone(), d);
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("刷新").clicked() {
                            self.refresh_devices();
                        }
                        if self.live_started {
                            ui.label("● 已连接");
                        } else if ui.button("连接").clicked() {
                            self.start_live();
                        }
                    });
                    ui.separator();
                    ui.label("scrcpy 目录:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.scrcpy_dir);
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                self.scrcpy_dir = p.display().to_string();
                            }
                        }
                    });
                    ui.label("画质:")
                        .on_hover_text("清晰度越低，传输量越小、操作越跟手；卡顿时优先选极速");
                    egui::ComboBox::from_id_source("quality")
                        .selected_text(match self.quality {
                            0 => "清晰",
                            1 => "流畅",
                            _ => "极速",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.quality, 0, "清晰 (原始分辨率)");
                            ui.selectable_value(&mut self.quality, 1, "流畅 (1024, ~4Mbps)");
                            ui.selectable_value(&mut self.quality, 2, "极速 (720, ~2Mbps)");
                        });
                    if self.quality == 0 {
                        ui.horizontal(|ui| {
                            ui.label("最大尺寸:");
                            ui.add(
                                egui::DragValue::new(&mut self.max_video_size)
                                    .clamp_range(0u32..=10000),
                            )
                            .on_hover_text("0 = 设备原始分辨率");
                        });
                    }
                    ui.separator();
                    ui.strong("u2 快速抓树（可选）");
                    if let Some(s) = &self.u2_status {
                        ui.label(s);
                    }
                    ui.label("u2 jar 路径 (core-src.jar):");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.u2_jar);
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("Jar", &["jar"])
                                .pick_file()
                            {
                                self.u2_jar = p.display().to_string();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if self.u2.is_some() {
                            ui.label("● 已连接");
                        } else if ui
                            .add_enabled(!self.u2_jar.trim().is_empty(), egui::Button::new("推送并启动"))
                            .clicked()
                        {
                            self.try_connect_u2();
                        }
                    });
                    ui.weak("不配置时自动使用 uiautomator dump（功能不受影响）");
                    ui.separator();
                    let ready = self.config_ready();
                    let btn = egui::Button::new("完成配置并进入")
                        .fill(if ready {
                            Color32::from_rgb(40, 150, 70)
                        } else {
                            Color32::from_gray(120)
                        });
                    if ui.add_enabled(ready, btn).clicked() {
                        self.refresh_devices();
                        self.configured = true;
                        self.config_just_completed = true;
                        self.status = "配置完成，可以开始使用了。".to_string();
                    }
                    if !ready {
                        ui.label("请先设置 ADB 路径，并在「刷新」后选择/连接一台设备。");
                    }
                });
            // Close the window precisely when the user finished config; otherwise
            // mirror the window's own open/close state (so it can be reopened any
            // time via the ⚙ 配置 button and closed again via X).
            if self.config_just_completed {
                self.show_config = false;
                self.config_just_completed = false;
            } else {
                self.show_config = cfg_open;
            }
        }

        // ---- Left panel: element properties (always present so the layout
        // width is constant; content hidden in operate mode) ----
        egui::SidePanel::left("props")
            .frame(card_frame(ctx.style().visuals.dark_mode))
            .default_width(300.0)
            .min_width(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading(if self.op_mode { "设备 / 应用" } else { "元素属性" });
                    if self.op_mode {
                        if ui.button("↻ 刷新").clicked() {
                            self.panel_refresh();
                        }
                    } else if let Some(id) = self.selected {
                        ui.weak(format!("id = {id}"));
                    }
                });
                if self.op_mode {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_source("dev_panel")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.render_dev_panel(ui));
                    return;
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("element_props")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(id) = self.selected {
                            if let Some(node) = self.tree.as_ref().and_then(|t| t.find(id)) {
                                render_props(ui, node);
                            } else {
                                ui.label("所选元素已不存在。");
                            }
                        } else {
                            ui.label("在截图或层级树中点击一个元素以查看其属性。");
                        }
                    });
            });

        // ---- Right panel: full-height hierarchy tree (always present so the
        // layout width is constant; content hidden in operate mode) ----
        egui::SidePanel::right("right")
            .frame(card_frame(ctx.style().visuals.dark_mode))
            .default_width(560.0)
            .min_width(340.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if self.op_mode {
                        ui.heading("录制控制");
                    } else {
                        ui.heading("UI 层级结构");
                        if self.tree.is_some() {
                            ui.weak(format!("{} 个节点", self.tree_count));
                        }
                    }
                });
                if self.op_mode {
                    ui.separator();
                    ui.label("文本输入:");
                    ui.text_edit_singleline(&mut self.input_text);
                    if ui.button("发送").clicked() && !self.input_text.trim().is_empty() {
                        self.send_text(self.input_text.trim());
                        let t = self.input_text.trim().to_string();
                        self.record_text(&t);
                    }
                    ui.separator();
                    if ui.button("抓取层级").clicked() {
                        self.capture_hierarchy_now();
                    }
                    ui.separator();
                    ui.label("录制 / 回放:");
                    let rec_label = if self.recording { "■ 停止录制" } else { "● 开始录制" };
                    let rec_btn = egui::Button::new(rec_label).fill(if self.recording {
                        Color32::from_rgb(200, 40, 40)
                    } else {
                        Color32::from_rgb(40, 150, 70)
                    });
                    if ui.add(rec_btn).clicked() {
                        self.recording = !self.recording;
                        if self.recording {
                            self.steps.clear();
                            self.replay_failed.clear();
                            self.replay_current = None;
                            self.last_hier = None;
                            // Grab the current hierarchy so element selectors get
                            // recorded alongside the fractional coordinate fallback.
                            self.capture_hierarchy_now();
                            self.status =
                                "● 录制已开始：在设备上执行的操作会被记录，停止后会提示保存。".to_string();
                        } else {
                            // Stopped: offer to save. Cancelling the dialog simply
                            // discards without saving — no separate save button needed.
                            let n = self.steps.len();
                            if n > 0 {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_file_name(format!("recording_{}.yaml", file_timestamp()))
                                    .add_filter("YAML", &["yaml", "yml"])
                                    .save_file()
                                {
                                    match record::save_yaml(&path, &self.steps) {
                                        Ok(_) => {
                                            self.status = format!(
                                                "■ 录制已停止，已保存 {} 步到 {}",
                                                n,
                                                path.display()
                                            )
                                        }
                                        Err(e) => {
                                            self.status = format!("保存失败: {e}")
                                        }
                                    }
                                } else {
                                    self.status = format!(
                                        "■ 录制已停止（共 {} 步，未保存）。",
                                        n
                                    );
                                }
                            } else {
                                self.status =
                                    "■ 录制已停止，没有录制到任何步骤。".to_string();
                            }
                        }
                    }
                    ui.label(format!("已录制: {} 步", self.steps.len()));
                    ui.horizontal(|ui| {
                        if ui.button("清除").clicked() {
                            self.steps.clear();
                            self.status = "已清除录制。".to_string();
                        }
                    });
                    ui.separator();
                    ui.label("回放设置:");
                    ui.horizontal(|ui| {
                        ui.label("速度:");
                        ui.add(
                            egui::DragValue::new(&mut self.replay_speed)
                                .speed(0.1)
                                .clamp_range(0.25..=8.0),
                        )
                        .on_hover_text("1.0 = 录制时节奏，2.0 = 两倍速，0.5 = 半速");
                        ui.label("循环:");
                        ui.add(
                            egui::DragValue::new(&mut self.replay_loops)
                                .clamp_range(0u32..=100),
                        )
                        .on_hover_text("0 = 单次；N = 重复 N 次");
                    });
                    if ui.button("加载并回放…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("YAML", &["yaml", "yml"])
                            .pick_file()
                        {
                            match record::load_yaml(&path) {
                                Ok(steps) => {
                                    if steps.is_empty() {
                                        self.status = "录制文件为空。".to_string();
                                    } else {
                                        self.start_replay(steps);
                                    }
                                }
                                Err(e) => self.status = format!("{e}"),
                            }
                        }
                    }
                    if self.replaying {
                        ui.label("回放进行中…");
                    }
                    ui.separator();
                    egui::collapsing_header::CollapsingHeader::new("录制步骤列表")
                        .default_open(true)
                        .show(ui, |ui| {
                            if self.steps.is_empty() {
                                ui.label("（暂无录制步骤）");
                            } else {
                                // Highlight: red = replay failed, green = the step
                                // currently being recorded (last) or replayed.
                                let active = if self.recording {
                                    self.steps.len().saturating_sub(1)
                                } else {
                                    self.replay_current.unwrap_or(usize::MAX)
                                };
                                let base = ui.style().visuals.text_color();
                                egui::ScrollArea::vertical()
                                    .max_height(220.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        for (i, s) in self.steps.iter().enumerate() {
                                            let color = if self.replay_failed.contains(&i) {
                                                Color32::from_rgb(235, 90, 90)
                                            } else if i == active
                                                && (self.recording || self.replaying)
                                            {
                                                Color32::from_rgb(70, 200, 100)
                                            } else {
                                                base
                                            };
                                            ui.colored_label(
                                                color,
                                                format!("{:>3}. {}", i + 1, s.describe()),
                                            );
                                        }
                                    });
                            }
                        });
                    return;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("搜索:");
                    ui.text_edit_singleline(&mut self.search);
                });
                if let Some(id) = self.selected {
                    if let Some(node) = self.tree.as_ref().and_then(|t| t.find(id)) {
                        if let Some(b) = &node.bounds {
                            ui.monospace(format!(
                                "选中: [{},{}][{},{}]  ({} x {} px)",
                                b.left, b.top, b.right, b.bottom, b.width(), b.height()
                            ));
                        }
                    }
                }
                ui.separator();

                // The tree gets the full panel height and can scroll both
                // directions so long labels are never cut off.
                egui::ScrollArea::both()
                    .id_source("hierarchy_tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(tree) = &self.tree {
                            render_tree(
                                ui,
                                tree,
                                0,
                                &self.search,
                                &mut self.selected,
                                &mut self.hovered_tree,
                                &mut self.jump_to,
                                jump,
                            );
                        } else {
                            ui.label("尚未加载界面层级。");
                        }
                    });
            });

        // ---- Center: mode strip + live view (op mode) or screenshot + overlays ----
        egui::CentralPanel::default().show(ctx, |ui| {
            // Until the initial configuration is completed, show a setup screen
            // and don't load the device-dependent pages (capture / live).
            if !self.configured {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("欢迎使用 Android UI Viewer");
                        ui.add_space(8.0);
                        ui.label("使用前请先完成配置：");
                        ui.label("1. 点击右上角「⚙ 配置」，设置 ADB 路径（可浏览选择或手填）");
                        ui.label("2. 点击「刷新」选择目标设备，并点击「连接」");
                        ui.label("3. 点击「完成配置并进入」开始使用");
                        ui.add_space(8.0);
                        if ui.button("打开配置").clicked() {
                            self.show_config = true;
                        }
                    });
                });
                return;
            }
            // Mode selector sits directly above the image. Picking "查看 UI"
            // immediately captures the screen + hierarchy (no separate Capture
            // click needed); picking "操作设备" starts the live control session.
            // One compact toolbar directly above the picture: mode switch + zoom.
            // Because the side panels are always shown, this toolbar keeps a
            // constant width and the buttons never jump when switching modes.
            ui.horizontal_wrapped(|ui| {
                ui.label("模式:");
                let enabled = self.configured;
                if ui
                    .add_enabled(enabled, egui::SelectableLabel::new(!self.op_mode, "查看 UI"))
                    .clicked()
                {
                    // Always re-capture on (re)entering view mode: the device
                    // screen may have changed while operating, so refresh the
                    // screenshot + hierarchy instead of reusing the stale dump.
                    self.op_mode = false;
                    if !self.capturing {
                        self.start_capture();
                    }
                }
                if ui
                    .add_enabled(enabled, egui::SelectableLabel::new(self.op_mode, "操作设备"))
                    .clicked()
                {
                    if !self.op_mode {
                        self.op_mode = true;
                        if !self.live_started {
                            self.refresh_devices();
                            let auto_connect = if self.devices.is_empty() {
                                true
                            } else if self.devices.len() == 1 {
                                self.live_serial_hint = self.devices[0].clone();
                                true
                            } else if !self.live_serial_hint.is_empty()
                                && self.devices.iter().any(|d| d == &self.live_serial_hint)
                            {
                                true
                            } else {
                                self.status =
                                    "检测到多台设备，请在下方选择目标设备后点击「连接」。"
                                        .to_string();
                                false
                            };
                            if auto_connect {
                                self.start_live();
                            }
                        }
                    }
                }
                ui.separator();
                ui.label("缩放:");
                ui.add(egui::Slider::new(&mut self.zoom, 0.5..=4.0).text("x"));
                if !self.op_mode && ui.button("适配").clicked() {
                    self.zoom = 1.0;
                }
                ui.separator();
                if self.live_started || self.capturing {
                    ui.spinner();
                }
            });

            // Both modes share the SAME image geometry: identical letterbox
            // reserves and scale formula, so flipping between "查看 UI" and
            // "操作设备" never makes the picture jump or resize.
            let full_avail = ui.available_size();
            let side_w = (full_avail.x * 0.05).clamp(34.0, 46.0);
            let reserve_right = side_w + 14.0;
            let reserve_bottom = side_w * 2.0 + 20.0;
            let avail = Vec2::new(
                (full_avail.x - reserve_right).max(50.0),
                (full_avail.y - reserve_bottom).max(50.0),
            );

            // In "操作设备" mode the center shows the live stream and forwards
            // touch/key input to the device; no hierarchy overlay or inspection.
            if self.op_mode {
                if let (Some(tex), Some((w, h))) = (self.live_tex.as_ref(), self.live_size) {
                let scale = (avail.x / w as f32).min(avail.y / h as f32) * self.zoom;
                let content_size = Vec2::new(w as f32 * scale, h as f32 * scale);
                let (viewport, resp) = ui.allocate_exact_size(full_avail, Sense::click_and_drag());
                // Top-align the picture so the mode bar sits directly above it
                // (no vertical centering gap); horizontal centering is unchanged.
                let draw_rect = Rect::from_min_size(
                    Pos2::new(
                        viewport.min.x + (avail.x - content_size.x) / 2.0,
                        viewport.min.y,
                    ),
                    content_size,
                );

                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );

                // Prominent recording / replaying indicator over the live image.
                if self.recording {
                    // Pulsing "REC" so the recording state is unmistakable.
                    let phase = ((ui.input(|i| i.time) * 2.2).sin() * 0.5 + 0.5) as f32;
                    let alpha = 120 + (135.0 * phase) as u8;
                    draw_badge(
                        ui.painter(),
                        draw_rect.min + Vec2::new(10.0, 10.0),
                        "● 录制中",
                        Color32::from_rgba_unmultiplied(220, 40, 40, alpha),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(90));
                }
                if self.replaying {
                    draw_badge(
                        ui.painter(),
                        draw_rect.min + Vec2::new(10.0, 10.0),
                        "▶ 回放中",
                        Color32::from_rgb(230, 150, 20),
                    );
                }

                // Map a pointer position to device coordinates (None if outside).
                let to_dev = |p: Pos2| -> Option<(i32, i32)> {
                    let local = p - draw_rect.min;
                    if local.x < 0.0
                        || local.y < 0.0
                        || local.x > content_size.x
                        || local.y > content_size.y
                    {
                        return None;
                    }
                    let ix = (local.x / scale) as i32;
                    let iy = (local.y / scale) as i32;
                    if ix >= 0 && iy >= 0 && ix < w as i32 && iy < h as i32 {
                        Some((ix, iy))
                    } else {
                        None
                    }
                };

                self.hover_pix = ui
                    .input(|i| i.pointer.hover_pos())
                    .and_then(to_dev);

                // Gesture tracking: press -> drag (swipe) or tap / long-press.
                if ui.input(|i| i.pointer.button_pressed(PointerButton::Primary)) {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        if let Some(d) = to_dev(p) {
                            self.op_gest = Some(OpGest {
                                start_xy: d,
                                start_time: Instant::now(),
                                moved: false,
                            });
                            crate::log::debug!("touch_down ({}, {})", d.0, d.1);
                            // Begin the touch immediately so drags are live.
                            if let Some(c) = &self.live_control {
                                c.touch_down(d.0, d.1);
                            }
                        }
                    }
                }
                if let Some(g) = &mut self.op_gest {
                    if resp.dragged() {
                        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                            if let Some(d) = to_dev(p) {
                                let dx = (d.0 - g.start_xy.0).abs();
                                let dy = (d.1 - g.start_xy.1).abs();
                                if dx + dy > 12 {
                                    g.moved = true;
                                }
                                // Forward every move for a real-time swipe.
                                if let Some(c) = &self.live_control {
                                    c.touch_move(d.0, d.1);
                                }
                            }
                        }
                    }
                }
                if ui.input(|i| i.pointer.button_released(PointerButton::Primary)) {
                    if let Some(g) = self.op_gest.take() {
                        let end = ui
                            .input(|i| i.pointer.interact_pos())
                            .and_then(to_dev);
                        let elapsed = g.start_time.elapsed().as_millis();
                        let (sx, sy) = g.start_xy;
                        let lifted = end.unwrap_or((sx, sy));
                        if let Some(c) = &self.live_control {
                            // Lift the pointer where it was released; the press
                            // already happened on pointer-down, so a short hold
                            // becomes a tap and a long hold a long-press.
                            c.touch_up(lifted.0, lifted.1);
                        } else if g.moved {
                            if let Some((ex, ey)) = end {
                                self.adb_sh(&format!("input swipe {sx} {sy} {ex} {ey} 200"));
                            }
                        } else if elapsed > 500 {
                            // Long press: hold in place.
                            self.adb_sh(&format!("input swipe {sx} {sy} {sx} {sy} 600"));
                        } else {
                            self.adb_sh(&format!("input tap {sx} {sy}"));
                        }
                        // Record the gesture (selector + fractional fallback).
                        if g.moved {
                            if let Some((ex, ey)) = end {
                                self.record_swipe(sx, sy, ex, ey);
                            }
                        } else {
                            self.record_tap(lifted.0, lifted.1, elapsed > 500);
                        }
                        crate::log::debug!(
                            "touch_up ({}, {}) moved={} elapsed_ms={} ({} 坐标)",
                            lifted.0,
                            lifted.1,
                            g.moved,
                            elapsed,
                            if self.live_control.is_some() {
                                "scrcpy"
                            } else {
                                "adb"
                            }
                        );
                    }
                }
                // Secondary pointer (right button): a second finger for pinch,
                // or a plain right-click that acts as the Back button.
                if ui.input(|i| i.pointer.button_pressed(PointerButton::Secondary)) {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        if let Some(d) = to_dev(p) {
                            self.op_gest2 = Some(OpGest {
                                start_xy: d,
                                start_time: Instant::now(),
                                moved: false,
                            });
                            if let Some(c) = &self.live_control {
                                c.touch_down_pid(1, d.0, d.1);
                            }
                        }
                    }
                }
                if let Some(g) = &mut self.op_gest2 {
                    if ui.input(|i| i.pointer.button_down(PointerButton::Secondary)) {
                        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                            if let Some(d) = to_dev(p) {
                                let dx = (d.0 - g.start_xy.0).abs();
                                let dy = (d.1 - g.start_xy.1).abs();
                                if dx + dy > 12 {
                                    g.moved = true;
                                }
                                if let Some(c) = &self.live_control {
                                    c.touch_move_pid(1, d.0, d.1);
                                }
                            }
                        }
                    }
                }
                if ui.input(|i| i.pointer.button_released(PointerButton::Secondary)) {
                    if let Some(g) = self.op_gest2.take() {
                        let end = ui
                            .input(|i| i.pointer.interact_pos())
                            .and_then(to_dev);
                        let (sx, sy) = g.start_xy;
                        let lifted = end.unwrap_or((sx, sy));
                        if let Some(c) = &self.live_control {
                            c.touch_up_pid(1, lifted.0, lifted.1);
                        } else if !g.moved {
                            self.adb_sh(&format!("input tap {sx} {sy}"));
                        }
                        // A right click (no drag) acts as the Back button.
                        if !g.moved {
                            self.send_key(4);
                            self.record_key(4, "返回");
                        }
                    }
                }
                // Mouse wheel -> device scroll (only while hovering the phone).
                if resp.hovered() {
                    let mut wheel = egui::Vec2::ZERO;
                    let events = ctx.input(|i| i.events.to_vec());
                    for e in &events {
                        if let egui::Event::MouseWheel { delta, .. } = e {
                            wheel += *delta;
                        }
                    }
                    if wheel.x != 0.0 || wheel.y != 0.0 {
                        if let Some((sx, sy)) = self.hover_pix {
                            if let Some(c) = &self.live_control {
                                // egui convention: +y = scroll down, +x = scroll
                                // right. scrcpy convention: +vScroll = up,
                                // +hScroll = right. So negate y.
                                const SCROLL_SCALE: f32 = 0.04;
                                let v_units = -wheel.y * SCROLL_SCALE;
                                let h_units = wheel.x * SCROLL_SCALE;
                                c.scroll(sx, sy, h_units, v_units);
                            }
                        }
                    }
                }

                // In operate mode the live image is shown bare (no hierarchy
                // overlay) so touches map 1:1 to the device and there is no
                // inspection framing confusing the operator.

                // On-screen controls placed OUTSIDE the screen image, matching a
                // real device: power + volume on the right edge, a navigation
                // bar just below the screen, and an end-session button. They sit
                // in the letterbox around the picture so they never cover it, and
                // clicks on them cannot start a tap/swipe (those only act inside
                // draw_rect).
                let gap = 8.0;
                let spacing = 10.0;
                let side_w = (draw_rect.width() * 0.05).clamp(34.0, 46.0);
                let side_h = side_w * 2.0;
                // Right-hand column: end-session, power, volume+, volume-.
                // Anchored near the TOP of the screen (like a real phone's
                // side keys sit high) rather than vertically centered.
                let stack: [(Icon, &str, Option<u32>); 4] = [
                    (Icon::End, "结束会话", None),
                    (Icon::Power, "电源", Some(26)),
                    (Icon::VolUp, "音量+", Some(24)),
                    (Icon::VolDown, "音量-", Some(25)),
                ];
                let total_h = side_h * stack.len() as f32
                    + spacing * (stack.len() as f32 - 1.0);
                let max_side_x = viewport.max.x - side_w - 2.0;
                let side_x = (draw_rect.max.x + gap).min(max_side_x);
                // Start just below the top of the screen and grow downward,
                // clamped so the whole column stays inside the viewport. Use a
                // safe range: when the column is taller than the viewport, fall
                // back to the top edge rather than panicking on min > max.
                let y_min = viewport.min.y + 2.0;
                let y_max = (viewport.max.y - total_h - 2.0).max(y_min);
                let mut y = (draw_rect.min.y + gap).clamp(y_min, y_max);
                for (icon, tip, key) in stack {
                    let r = Rect::from_min_size(Pos2::new(side_x, y), Vec2::new(side_w, side_h));
                    if overlay_button(ui, r, icon, tip) {
                        match key {
                            None => {
                                self.stop_live();
                                self.op_mode = false;
                            }
                            Some(k) => {
                                self.send_key(k);
                                self.record_key(k, tip);
                            }
                        }
                    }
                    y += side_h + spacing;
                }

                // Navigation bar just below the screen.
                let nav_h = side_h;
                let nav_btn_w = (draw_rect.width() * 0.18).clamp(64.0, 120.0);
                let nav_total = nav_btn_w * 3.0 + spacing * 2.0;
                let nav_y = (draw_rect.max.y + gap).min(viewport.max.y - nav_h - 2.0);
                // Safe clamp: if the nav bar is wider than the viewport, pin to
                // the left edge instead of panicking on min > max.
                let nav_l = viewport.min.x + 2.0;
                let nav_r = (viewport.max.x - nav_total - 2.0).max(nav_l);
                let nav_x = (draw_rect.center().x - nav_total / 2.0).clamp(nav_l, nav_r);
                let nav: [(Icon, &str, u32); 3] = [
                    (Icon::Back, "返回", 4),
                    (Icon::Home, "主页", 3),
                    (Icon::Recent, "最近", 187),
                ];
                let mut x = nav_x;
                for (icon, tip, key) in nav {
                    let r = Rect::from_min_size(Pos2::new(x, nav_y), Vec2::new(nav_btn_w, nav_h));
                    if overlay_button(ui, r, icon, tip) {
                        self.send_key(key);
                        self.record_key(key, tip);
                    }
                    x += nav_btn_w + spacing;
                }
            } else if self.live_started {
                ui.centered_and_justified(|ui| {
                    ui.label("操作会话启动中…（正在通过 scrcpy 获取视频流）");
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("尚未启动操作会话。");
                });
            }
        } else if let (Some(tex), Some((w, h))) = (&self.screenshot, self.image_size) {
                let scale = (avail.x / w as f32).min(avail.y / h as f32) * self.zoom;
                let content_size = Vec2::new(w as f32 * scale, h as f32 * scale);

                let (viewport, resp) =
                    ui.allocate_exact_size(full_avail, Sense::click_and_drag());

                // Pan: at fit zoom the image is centered; when zoomed in the user
                // can drag to pan (clamped so it can't be lost off-screen).
                if self.zoom <= 1.001 {
                    // Top-align so the mode bar is directly above the image;
                    // horizontal centering matches the operate mode.
                    self.pan = Vec2::new((avail.x - content_size.x) / 2.0, 0.0);
                } else {
                    if resp.drag_started() {
                        self.panning = true;
                    }
                    self.pan += resp.drag_delta();
                    let min_pan = avail - content_size;
                    self.pan.x = self.pan.x.clamp(min_pan.x.min(0.0), 0.0);
                    self.pan.y = self.pan.y.clamp(min_pan.y.min(0.0), 0.0);
                }

                let draw_rect = Rect::from_min_size(viewport.min + self.pan, content_size);
                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );

                // Hovered element (cursor -> image space).
                let mut hovered = None;
                if let Some(hp) = ui.input(|i| i.pointer.hover_pos()) {
                    let local = hp - draw_rect.min;
                    let ix = (local.x / scale) as i32;
                    let iy = (local.y / scale) as i32;
                    if ix >= 0 && iy >= 0 && ix < w as i32 && iy < h as i32 {
                        self.hover_pix = Some((ix, iy));
                    } else {
                        self.hover_pix = None;
                    }
                    if let Some(tree) = &self.tree {
                        hovered = tree.hit_test(ix, iy);
                    }
                }
                // Also highlight the control whose row is hovered in the tree.
                if hovered.is_none() {
                    hovered = self.hovered_tree;
                }

                if let Some(tree) = &self.tree {
                    let draw_faint = self.tree_count < FAINT_NODE_LIMIT;
                    draw_overlays(ui.painter(), tree, draw_rect, scale, self.selected, hovered, draw_faint);
                }

                // Jump-to: center the selected control in the viewport.
                if let Some(j) = jump {
                    if let Some(node) = self.tree.as_ref().and_then(|t| t.find(j)) {
                        if let Some(b) = &node.bounds {
                            let cx = (b.left + b.right) as f32 * 0.5 * scale;
                            let cy = (b.top + b.bottom) as f32 * 0.5 * scale;
                            self.pan =
                                (viewport.min + avail / 2.0) - (draw_rect.min + Vec2::new(cx, cy));
                            let min_pan = avail - content_size;
                            self.pan.x = self.pan.x.clamp(min_pan.x.min(0.0), 0.0);
                            self.pan.y = self.pan.y.clamp(min_pan.y.min(0.0), 0.0);
                        }
                    }
                }

                // Click to select (ignore drags used for panning).
                if resp.clicked() && !self.panning {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        let local = pos - draw_rect.min;
                        let ix = (local.x / scale) as i32;
                        let iy = (local.y / scale) as i32;
                        if let Some(tree) = &self.tree {
                            if let Some(id) = tree.hit_test(ix, iy) {
                                self.selected = Some(id);
                                self.jump_to = Some(id);
                            }
                        }
                    }
                }
                if resp.drag_stopped() {
                    self.panning = false;
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("暂无截图。点击 “Capture (adb)” 抓取设备，或把截图/XML 文件拖入此窗口。");
                });
            }
        });

        // Physical keyboard -> device (real-time operation). Forward only in
        // operate mode, and only while the cursor is over the live phone image,
        // so typing in our own text fields is unaffected.
        if self.op_mode {
            let mut rec_keys: Vec<(u32, String)> = Vec::new();
            if let Some(ctrl) = &self.live_control {
                if self.hover_pix.is_some() {
                    let mods = ctx.input(|i| i.modifiers);
                    let meta = android_meta(mods);
                    // A "shortcut" is Ctrl / Cmd / Win held: route letters as
                    // keycodes so combinations (e.g. Ctrl+C) reach the device.
                    // Shift/Alt alone route through text input instead.
                    let is_shortcut = mods.ctrl || mods.command || mods.mac_cmd;
                    for ev in ctx.input(|i| i.events.clone()) {
                        match ev {
                            egui::Event::Key { key, pressed, .. } => {
                                if let Some(code) = android_keycode(key, is_shortcut) {
                                    if pressed {
                                        ctrl.key_down_meta(code, meta);
                                        rec_keys.push((code, format!("{key:?}")));
                                    } else {
                                        ctrl.key_up_meta(code, meta);
                                    }
                                }
                            }
                            egui::Event::Text(t) => ctrl.text(&t),
                            egui::Event::Paste(t) => ctrl.text(&t),
                            _ => {}
                        }
                    }
                }
            }
            for (code, label) in rec_keys {
                self.record_key(code, &label);
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::log::info!("应用退出");
        self.stop_live();
    }
}

// ----- Tree rendering -----

fn node_label(node: &Node) -> String {
    let class = node
        .attrs
        .get("class")
        .map(|s| s.rsplit('.').next_back().unwrap_or(s).to_string())
        .unwrap_or_else(|| "?".to_string());
    let rid = node
        .attrs
        .get("resource-id")
        .filter(|s| !s.is_empty())
        .map(|s| format!(" #{s}"));
    let text = node
        .attrs
        .get("text")
        .filter(|s| !s.is_empty())
        .map(|s| format!(" “{s}”"));
    format!(
        "{}{}{}",
        class,
        rid.unwrap_or_default(),
        text.unwrap_or_default()
    )
}

// Rendering the tree needs several mutable selection cursors; grouping them all
// in a struct would hurt readability more than it helps, so allow the lint.
#[allow(clippy::too_many_arguments)]
fn render_tree(
    ui: &mut egui::Ui,
    node: &Node,
    depth: usize,
    search: &str,
    selected: &mut Option<usize>,
    hovered_tree: &mut Option<usize>,
    pending: &mut Option<usize>,
    jump: Option<usize>,
) {
    if !search.is_empty() && !node.subtree_matches(search) {
        return;
    }
    // Auto-expand the ancestor chain of the node we are jumping to.
    let is_ancestor = jump.is_some_and(|j| node.find(j).is_some());
    let is_target = jump == Some(node.id);
    let is_selected = *selected == Some(node.id);
    let label = node_label(node);
    let resp = egui::collapsing_header::CollapsingHeader::new(label)
        .id_source(node.id)
        .default_open(depth < 2)
        .open(if is_ancestor { Some(true) } else { None })
        .show(ui, |inner| {
            for c in &node.children {
                render_tree(inner, c, depth + 1, search, selected, hovered_tree, pending, jump);
            }
        });

    let header = &resp.header_response;

    // Hovering a tree row highlights the matching control on the screenshot.
    if header.hovered() {
        *hovered_tree = Some(node.id);
    }
    if header.clicked() {
        *selected = Some(node.id);
        *pending = Some(node.id);
    }

    if ui.is_rect_visible(header.rect) {
        // Highlight the selected row in the tree so selection stays visible
        // (painted over the header, like a selection overlay).
        if is_selected {
            let r = header.rect.expand2(Vec2::new(3.0, 2.0));
            ui.painter().rect_filled(
                r,
                3.0,
                Color32::from_rgba_unmultiplied(0, 150, 255, 55),
            );
            ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, Color32::from_rgb(0, 150, 255)));
        } else if *hovered_tree == Some(node.id) {
            let r = header.rect.expand2(Vec2::new(3.0, 2.0));
            ui.painter().rect_filled(
                r,
                3.0,
                Color32::from_rgba_unmultiplied(255, 210, 0, 28),
            );
        }
    }

    // Scroll the hierarchy tree to the selected node.
    if is_target {
        ui.scroll_to_rect(header.rect, Some(Align::Center));
    }
}

// ----- Properties rendering -----

fn render_props(ui: &mut egui::Ui, node: &Node) {
    let class = node.attrs.get("class").cloned().unwrap_or_default();
    ui.label(egui::RichText::new(class).strong());
    ui.separator();

    let mut keys: Vec<&String> = node.attrs.keys().collect();
    keys.sort();
    egui::Grid::new("props").striped(true).show(ui, |ui| {
        for k in keys {
            let v = node.attrs.get(k.as_str()).map(|s| s.as_str()).unwrap_or("");
            ui.label(egui::RichText::new(k).weak());
            ui.label(v);
            ui.end_row();
        }
    });

    if let Some(b) = &node.bounds {
        ui.separator();
        ui.label(format!(
            "bounds: [{},{}][{},{}]",
            b.left, b.top, b.right, b.bottom
        ));
        ui.label(format!("尺寸: {} x {} px", b.width(), b.height()));
    }
}

// ----- Overlay drawing -----

fn draw_overlays(
    painter: &egui::Painter,
    node: &Node,
    rect: Rect,
    scale: f32,
    selected: Option<usize>,
    hovered: Option<usize>,
    draw_faint: bool,
) {
    if let Some(b) = &node.bounds {
        let min = rect.min + Vec2::new(b.left as f32 * scale, b.top as f32 * scale);
        let max = rect.min + Vec2::new(b.right as f32 * scale, b.bottom as f32 * scale);
        let r = Rect::from_min_max(min, max);

        if Some(node.id) == selected {
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0, 180, 255, 50));
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(0, 180, 255)));
        } else if Some(node.id) == hovered {
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 210, 0, 40));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::from_rgb(255, 210, 0)));
        } else if draw_faint {
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 200, 255, 22)),
            );
        }
    }
    for c in &node.children {
        draw_overlays(painter, c, rect, scale, selected, hovered, draw_faint);
    }
}
