use std::sync::mpsc::Receiver;

use eframe::egui;
use eframe::egui::{
    Align, Color32, ColorImage, FontData, FontDefinitions, FontFamily, Pos2, Rect, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};

use crate::adb::{dump_ui, capture, CaptureResult};
use crate::ui_tree::Node;

const FAINT_NODE_LIMIT: usize = 2000;

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

        // ---- Center: screenshot + overlays (zoomable & pannable) ----
        egui::CentralPanel::default().show(ctx, |ui| {
            if let (Some(tex), Some((w, h))) = (&self.screenshot, self.image_size) {
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
