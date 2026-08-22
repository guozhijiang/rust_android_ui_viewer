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

use crate::adb::{dump_ui, dump_ui_serial, capture, CaptureResult};
use crate::live::{self, LiveControl, LiveEvent};
use crate::ui_tree::Node;

const FAINT_NODE_LIMIT: usize = 2000;

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

    // ---- Operation (live scrcpy) mode ----
    op_mode: bool,
    live_started: bool,
    live_rx: Option<Receiver<LiveEvent>>,
    live_stop: Option<Arc<AtomicBool>>,
    live_tex: Option<TextureHandle>,
    live_size: Option<(u32, u32)>,
    live_control: Option<LiveControl>,
    live_serial: String,
    live_serial_hint: String,
    scrcpy_dir: String,
    max_video_size: u32,
    input_text: String,
    op_gest: Option<OpGest>,
    op_gest2: Option<OpGest>,
    xml_rx: Option<Receiver<anyhow::Result<String>>>,
}

impl UiViewerApp {
    pub fn new() -> Self {
        Self {
            adb_path: "adb".to_string(),
            screenshot: None,
            image_size: None,
            tree: None,
            tree_count: 0,
            selected: None,
            hovered_tree: None,
            hover_pix: None,
            search: String::new(),
            status: "就绪。点击 “Capture (adb)” 抓取设备界面，或把截图/XML 拖入窗口。".to_string(),
            capturing: false,
            zoom: 1.0,
            jump_to: None,
            pan: Vec2::ZERO,
            panning: false,
            rx: None,
            op_mode: false,
            live_started: false,
            live_rx: None,
            live_stop: None,
            live_tex: None,
            live_size: None,
            live_control: None,
            live_serial: String::new(),
            live_serial_hint: String::new(),
            scrcpy_dir: String::new(),
            max_video_size: 0,
            input_text: String::new(),
            op_gest: None,
            op_gest2: None,
            xml_rx: None,
        }
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
                break;
            }
        }
        ctx.set_fonts(fonts);
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
        Ok(())
    }

    fn start_capture(&mut self) {
        self.capturing = true;
        self.status = "正在抓取设备界面…".to_string();
        let adb = self.adb_path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<CaptureResult> {
                let screenshot = capture(&adb)?;
                let xml = dump_ui(&adb)?;
                Ok(CaptureResult { screenshot, xml })
            })();
            let _ = tx.send(res);
        });
    }

    fn load_xml(&mut self, xml: &str) {
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

    /// Spawn the live scrcpy session (video stream + touch control).
    fn start_live(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        live::start(
            self.adb_path.clone(),
            self.live_serial_hint.clone(),
            self.scrcpy_dir.clone(),
            self.max_video_size,
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
        if let Some(c) = &self.live_control {
            c.press_key(code);
        } else {
            self.adb_sh(&format!("input keyevent {code}"));
        }
    }

    fn send_text(&self, text: &str) {
        if let Some(c) = &self.live_control {
            c.text(text);
        } else {
            // `input text` needs spaces/% escaped; keep it simple and reliable.
            let escaped = text.replace('%', "%%").replace(' ', "%s");
            self.adb_sh(&format!("input text {escaped}"));
        }
    }

    /// Dump + load the UI hierarchy while the live session keeps running.
    fn capture_hierarchy_now(&mut self) {
        self.status = "正在抓取界面层级…".to_string();
        let adb = self.adb_path.clone();
        let serial = self.live_serial.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.xml_rx = Some(rx);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<String> {
                if serial.is_empty() {
                    dump_ui(&adb)
                } else {
                    dump_ui_serial(&adb, &serial)
                }
            })();
            let _ = tx.send(res);
        });
    }
}

impl eframe::App for UiViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Consume a pending "jump to selected node" request (set on the previous
        // frame when an element was selected). `jump` drives this frame's scroll.
        let jump = self.jump_to.take();

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

        // Drain a pending hierarchy-only dump (op mode "grab hierarchy").
        if let Some(rx) = &self.xml_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(xml) => self.load_xml(&xml),
                    Err(e) => self.status = format!("层级抓取失败：{e}"),
                }
                self.xml_rx = None;
                self.status = format!("{}  点击界面可操作；Ctrl+点击可选中元素。", self.status);
                ctx.request_repaint();
            }
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
                    if !serial.is_empty() {
                        self.live_serial = serial;
                    }
                    let ctrl = if self.live_control.is_some() {
                        "实时控制已就绪"
                    } else {
                        "控制通道不可用，已回退 adb"
                    };
                    self.status = format!("已连接 {device_name}（{width}x{height}） · {ctrl}");
                }
                LiveEvent::Status(s) => self.status = s,
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
                    self.status = format!("操作会话错误：{e}");
                    self.stop_live();
                    ctx.request_repaint();
                }
                LiveEvent::Stopped => {
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

        // ---- Top panel: actions + adb path + status ----
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            // Mode: static capture vs live control.
            ui.horizontal_wrapped(|ui| {
                let in_op = self.op_mode;
                if ui.selectable_label(in_op, "操作模式").clicked() {
                    if !in_op {
                        if !self.live_started {
                            self.start_live();
                        }
                        self.op_mode = true;
                        // Drop any stale capture-mode artifacts so the live view
                        // doesn't keep showing the previous screenshot / tree.
                        self.screenshot = None;
                        self.image_size = None;
                        self.tree = None;
                        self.tree_count = 0;
                        self.selected = None;
                        self.jump_to = None;
                        self.hover_pix = None;
                        self.xml_rx = None;
                    }
                }
                if ui.selectable_label(!in_op, "抓取模式").clicked() {
                    if in_op {
                        self.op_mode = false;
                        if self.live_started {
                            self.stop_live();
                        }
                        // Ensure no leftover live frame lingers in the center.
                        self.live_tex = None;
                        self.live_size = None;
                    }
                }
                ui.separator();
                if self.live_started {
                    ui.spinner();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("📱 Capture (adb)").clicked() && !self.capturing {
                    self.start_capture();
                }
                if ui.button("🖼 Load Screenshot").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Image", &["png", "jpg", "jpeg"])
                        .pick_file()
                    {
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
                    }
                }
                if ui.button("🌳 Load XML").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("XML", &["xml"])
                        .pick_file()
                    {
                        match std::fs::read_to_string(&path) {
                            Ok(s) => self.load_xml(&s),
                            Err(e) => self.status = format!("读取 XML 失败：{e}"),
                        }
                    }
                }
                ui.separator();
                ui.label("ADB 路径:");
                ui.text_edit_singleline(&mut self.adb_path);
                ui.separator();
                if self.capturing {
                    ui.spinner();
                    ui.label("抓取中…");
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("搜索:");
                ui.text_edit_singleline(&mut self.search);
                ui.separator();
                if let Some((x, y)) = self.hover_pix {
                    ui.monospace(format!("坐标: ({x}, {y})"));
                }
                if let Some(id) = self.selected {
                    if let Some(node) = self.tree.as_ref().and_then(|t| t.find(id)) {
                        if let Some(b) = &node.bounds {
                            ui.monospace(format!(
                                "选中: [{},{}][{},{}]  ({} × {} px)",
                                b.left,
                                b.top,
                                b.right,
                                b.bottom,
                                b.width(),
                                b.height()
                            ));
                        }
                    }
                }
                ui.separator();
                ui.colored_label(Color32::from_rgb(220, 220, 220), &self.status);
            });
        });

        // ---- Bottom panel: live control quick buttons (aligned under the screen) ----
        if self.op_mode {
            egui::TopBottomPanel::bottom("op_controls").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("序列号:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.live_serial_hint).desired_width(110.0),
                    );
                    ui.label("scrcpy 目录:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.scrcpy_dir).desired_width(160.0),
                    );
                    ui.label("最大尺寸:");
                    ui.add(egui::DragValue::new(&mut self.max_video_size).clamp_range(0u32..=10000))
                        .on_hover_text("0 = 设备原始分辨率");
                    ui.separator();
                    ui.label("文本输入:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.input_text).desired_width(160.0),
                    );
                    if ui.button("发送").clicked() && !self.input_text.trim().is_empty() {
                        self.send_text(self.input_text.trim());
                    }
                    if ui.button("抓取层级").clicked() {
                        self.capture_hierarchy_now();
                    }
                    ui.separator();
                    if ui.button("返回").clicked() {
                        self.send_key(4);
                    }
                    if ui.button("主页").clicked() {
                        self.send_key(3);
                    }
                    if ui.button("最近").clicked() {
                        self.send_key(187);
                    }
                    if ui.button("电源").clicked() {
                        self.send_key(26);
                    }
                    if ui.button("音量+").clicked() {
                        self.send_key(24);
                    }
                    if ui.button("音量-").clicked() {
                        self.send_key(25);
                    }
                    if ui.button("结束会话").clicked() {
                        self.stop_live();
                        self.op_mode = false;
                    }
                });
            });
        }

        // ---- Left panel: element properties (full height, no scrolling needed) ----
        egui::SidePanel::left("props")
            .default_width(300.0)
            .min_width(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("元素属性");
                    if let Some(id) = self.selected {
                        ui.weak(format!("id = {id}"));
                    }
                });
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

        // ---- Right panel: full-height hierarchy tree ----
        egui::SidePanel::right("right")
            .default_width(560.0)
            .min_width(340.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("UI 层级结构");
                    if self.tree.is_some() {
                        ui.weak(format!("{} 个节点", self.tree_count));
                    }
                });
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

        // ---- Center: live view (op mode) or screenshot + overlays ----
        egui::CentralPanel::default().show(ctx, |ui| {
            if let (Some(tex), Some((w, h))) = (self.live_tex.as_ref(), self.live_size) {
                ui.horizontal(|ui| {
                    ui.label("缩放:");
                    ui.add(egui::Slider::new(&mut self.zoom, 0.5..=4.0).text("x"));
                });
                let avail = ui.available_size();
                let scale = (avail.x / w as f32).min(avail.y / h as f32).min(self.zoom);
                let content_size = Vec2::new(w as f32 * scale, h as f32 * scale);
                let (viewport, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
                let draw_rect = Rect::from_center_size(viewport.center(), content_size);

                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );

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
                    .and_then(|p| to_dev(p));

                // Gesture tracking: press -> drag (swipe) or tap / long-press.
                if ui.input(|i| i.pointer.button_pressed(PointerButton::Primary)) {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        if let Some(d) = to_dev(p) {
                            self.op_gest = Some(OpGest {
                                start_xy: d,
                                start_time: Instant::now(),
                                moved: false,
                            });
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
                            .and_then(|p| to_dev(p));
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
                        // Also select the tapped element in the tree (tap only).
                        if !g.moved {
                            if let Some(tree) = &self.tree {
                                if let Some(id) = tree.hit_test(lifted.0, lifted.1) {
                                    self.selected = Some(id);
                                    self.jump_to = Some(id);
                                }
                            }
                        }
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
                            .and_then(|p| to_dev(p));
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
                        }
                    }
                }
                // Ctrl+click -> inspect (hit test) without sending a tap.
                let ctrl = ui.input(|i| i.modifiers.command);
                if resp.clicked() && ctrl {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        if let Some((ix, iy)) = to_dev(p) {
                            if let Some(tree) = &self.tree {
                                if let Some(id) = tree.hit_test(ix, iy) {
                                    self.selected = Some(id);
                                    self.jump_to = Some(id);
                                }
                            }
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

                // Overlay the current hierarchy (if any) on the live image.
                if let Some(tree) = &self.tree {
                    let draw_faint = self.tree_count < FAINT_NODE_LIMIT;
                    draw_overlays(
                        ui.painter(),
                        tree,
                        draw_rect,
                        scale,
                        self.selected,
                        self.hovered_tree,
                        draw_faint,
                    );
                }
            } else if self.op_mode {
                ui.centered_and_justified(|ui| {
                    ui.label("操作会话启动中…（正在通过 scrcpy 获取视频流）");
                });
            } else if let (Some(tex), Some((w, h))) = (&self.screenshot, self.image_size) {
                // Zoom controls
                ui.horizontal(|ui| {
                    ui.label("缩放:");
                    ui.add(egui::Slider::new(&mut self.zoom, 0.25..=5.0).text("x"));
                    if ui.button("适配").clicked() {
                        self.zoom = 1.0;
                    }
                });

                let viewport_avail = ui.available_size();
                let base = (viewport_avail.x / w as f32).min(viewport_avail.y / h as f32);
                let scale = base * self.zoom;
                let content_size = Vec2::new(w as f32 * scale, h as f32 * scale);

                let (viewport, resp) =
                    ui.allocate_exact_size(viewport_avail, Sense::click_and_drag());

                // Pan: at fit zoom the image is centered; when zoomed in the user
                // can drag to pan (clamped so it can't be lost off-screen).
                if self.zoom <= 1.001 {
                    self.pan = (viewport.size() - content_size) * 0.5;
                } else {
                    if resp.drag_started() {
                        self.panning = true;
                    }
                    self.pan += resp.drag_delta();
                    let min_pan = viewport.size() - content_size;
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
                            self.pan = viewport.center() - (draw_rect.min + Vec2::new(cx, cy));
                            let min_pan = viewport.size() - content_size;
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

        // Physical keyboard -> device (real-time operation). Forward only while
        // the cursor is over the live phone image, so typing in our own text
        // fields (search box, send-text) is unaffected.
        if self.op_mode {
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
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_live();
    }
}

// ----- Tree rendering -----

fn node_label(node: &Node) -> String {
    let class = node
        .attrs
        .get("class")
        .map(|s| s.rsplit('.').last().unwrap_or(s).to_string())
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
    let is_ancestor = jump.map_or(false, |j| node.find(j).is_some());
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
