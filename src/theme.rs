//! 视觉主题：design tokens + 控件样式。
//!
//! 架构（M2 起）：
//! - [`Theme`] 语义色 token：一次解析、全处引用，明暗主题共用同一套字段名；
//! - [`fs`] 字号刻度 / [`radius`] 圆角刻度：全局唯一出处，组件代码禁止裸写数字；
//! - [`overlay`] 截图覆盖层常量：画在设备截图上的高亮（选中/hover/REC/回放），
//!   截图本身不随主题变，这组色也不随主题变。层级树行高亮与截图框共用同一组色，
//!   保证"同一元素在两个视图里颜色一致"；
//! - [`shadow`] 阴影分级：Card / Popup / Window 三档，禁止散落自拼 Shadow。
//!
//! 间距策略（item_spacing/button_padding/...）集中在 [`apply_style`]，是唯一出处。

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Vec2,
};

// ---------------------------------------------------------------------------
// design tokens
// ---------------------------------------------------------------------------

/// 语义色 token 集。用 [`Theme::of`] / [`Theme::of_ui`] 解析，不要自己 if dark。
pub struct Theme {
    pub dark: bool,
    /// 最底层画布（窗口底色）。卡片浮在它上面。
    pub canvas: Color32,
    /// 卡片 / 面板 / 弹窗底色。
    pub card: Color32,
    /// 次级表面：输入框、代码段、条纹背景、按钮静息态填充。
    pub surface: Color32,
    /// 常规分隔线 / 控件描边。
    pub border: Color32,
    /// 强调型描边（hover / 焦点）。
    pub border_strong: Color32,
    /// 品牌强调色（青蓝）。
    pub accent: Color32,
    /// 强调色的超淡版本，用于 hover 底与选中行的淡底。
    pub accent_soft: Color32,
    /// 落在强调色填充之上的文字色（选中/按下状态）。
    pub on_accent: Color32,
    /// 主文本（比 egui 默认的灰白更亮，是"清晰"的第一要素）。
    pub text: Color32,
    /// 次要文本 / weak 标签。
    pub text_dim: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub danger: Color32,
}

impl Theme {
    pub fn of(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                canvas: Color32::from_rgb(10, 12, 17),
                card: Color32::from_rgb(20, 24, 31),
                surface: Color32::from_rgb(27, 32, 41),
                border: Color32::from_rgb(35, 42, 54),
                border_strong: Color32::from_rgb(51, 62, 80),
                accent: Color32::from_rgb(91, 140, 255),
                accent_soft: Color32::from_rgba_unmultiplied(91, 140, 255, 30),
                on_accent: Color32::from_rgb(12, 16, 26),
                text: Color32::from_rgb(231, 236, 246),
                text_dim: Color32::from_rgb(151, 161, 184),
                success: Color32::from_rgb(63, 200, 132),
                warn: Color32::from_rgb(245, 176, 66),
                danger: Color32::from_rgb(248, 113, 113),
            }
        } else {
            Self {
                dark,
                canvas: Color32::from_rgb(237, 241, 247),
                card: Color32::from_rgb(255, 255, 255),
                surface: Color32::from_rgb(245, 247, 251),
                border: Color32::from_rgb(223, 229, 239),
                border_strong: Color32::from_rgb(195, 205, 221),
                accent: Color32::from_rgb(37, 99, 235),
                accent_soft: Color32::from_rgba_unmultiplied(37, 99, 235, 26),
                on_accent: Color32::WHITE,
                text: Color32::from_rgb(22, 30, 44),
                text_dim: Color32::from_rgb(94, 107, 128),
                success: Color32::from_rgb(22, 145, 90),
                warn: Color32::from_rgb(180, 116, 10),
                danger: Color32::from_rgb(209, 59, 59),
            }
        }
    }

    /// 按当前 ui 的主题解析。每帧调用开销可忽略（纯栈上拷贝）。
    pub fn of_ui(ui: &egui::Ui) -> Self {
        Self::of(ui.visuals().dark_mode)
    }
}

/// 字号刻度。全部取整数：epaint 按 round(点数 × 总缩放) 光栅化字形再绘制，
/// 带小数的字号（如 13.5）在整数缩放下会出现"图集 14px、绘制 13.5px"的重采样，
/// 视觉发虚。整数字号在总缩放 1.0/2.0 时严格 1:1，是最锐利的选择。
/// （0.34 起 hinting 已回归，但整数字号依然是防重采样的硬前提。）
pub mod fs {
    /// 截图上的录制/回放徽章。
    pub const BADGE: f32 = 16.0;
    /// 全局 Heading。
    pub const HEADING: f32 = 18.0;
    /// 全局正文 / 顶栏产品名。
    pub const BODY: f32 = 15.0;
    /// 全局按钮 / 面板标题 / 强调按钮。
    pub const BUTTON: f32 = 14.0;
    /// 全局等宽。
    pub const MONO: f32 = 14.0;
    /// 分段控件 / 紧凑面板正文。
    pub const COMPACT: f32 = 13.0;
    /// 小标签 / chip / 工具栏注记。
    pub const SMALL: f32 = 12.0;
    /// 树内占位 / 弱注释（等宽）。
    pub const MINI: f32 = 11.0;
}

/// 圆角刻度。组件代码用这些常量，不要裸写数字。
pub mod radius {
    /// 弹窗。
    pub const WIN: u8 = 12;
    /// 卡片。
    pub const CARD: u8 = 10;
    /// 控件 / 输入框 / 分段轨道 / 内嵌分组。
    pub const CTRL: u8 = 8;
    /// 徽章。
    pub const BADGE: u8 = 6;
    /// 层级树行高亮。
    pub const ROW: u8 = 3;
    /// 面板标题强调竖条。
    pub const BAR: u8 = 2;
}

/// 截图覆盖层高亮色。设备截图不随主题变，这组色也不随主题变；
/// 层级树行高亮与截图框共用，保证同一元素两个视图颜色一致。
/// 需要半透明时配 [`with_alpha`]。
pub mod overlay {
    use eframe::egui::Color32;

    /// 选中元素（截图框 + 树行）。
    pub const SELECT: Color32 = Color32::from_rgb(0, 150, 255);
    /// 悬停元素（截图框 + 树行）。
    pub const HOVER: Color32 = Color32::from_rgb(255, 210, 0);
    /// 全部控件边界的淡网格。
    pub const FAINT: Color32 = Color32::from_rgb(120, 200, 255);
    /// 录制中徽章底色。
    pub const REC: Color32 = Color32::from_rgb(220, 40, 40);
    /// 回放中徽章底色。
    pub const REPLAY: Color32 = Color32::from_rgb(230, 150, 20);
}

/// 给覆盖层常量配 alpha 的唯一途径（保留 rgb、替换 alpha）。
pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// 阴影分级。
#[derive(Clone, Copy)]
pub enum ShadowLevel {
    /// 卡片：最轻，贴在画布上。
    Card,
    /// 下拉 / 弹层。
    Popup,
    /// 浮窗：最重。
    Window,
}

/// 三档阴影的唯一出处。明暗主题同构：暗色用黑影、亮色用蓝灰影。
pub fn shadow(dark: bool, level: ShadowLevel) -> egui::Shadow {
    let (color, offset, blur) = match level {
        ShadowLevel::Card => (
            if dark {
                Color32::from_black_alpha(90)
            } else {
                Color32::from_rgba_unmultiplied(40, 55, 90, 34)
            },
            [0, 2],
            if dark { 10 } else { 12 },
        ),
        ShadowLevel::Popup => (
            if dark {
                Color32::from_black_alpha(120)
            } else {
                Color32::from_rgba_unmultiplied(40, 55, 90, 45)
            },
            [0, 3],
            12,
        ),
        ShadowLevel::Window => (
            if dark {
                Color32::from_black_alpha(120)
            } else {
                Color32::from_rgba_unmultiplied(40, 55, 90, 45)
            },
            [0, 4],
            18,
        ),
    };
    egui::Shadow {
        offset,
        blur,
        spread: 0,
        color,
    }
}

// ---------------------------------------------------------------------------
// 卡片容器
// ---------------------------------------------------------------------------

/// 圆角 + 描边 + 柔和投影的卡片框，用于顶栏与左右侧栏。
/// 轻投影让卡片从画布上"浮"起来，是去廉价感最便宜的一招。
pub fn card_frame(dark: bool) -> egui::Frame {
    egui::Frame::NONE
        .fill(Theme::of(dark).card)
        .corner_radius(CornerRadius::same(radius::CARD))
        .stroke(Stroke::new(1.0, Theme::of(dark).border))
        .shadow(shadow(dark, ShadowLevel::Card))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .outer_margin(egui::Margin::symmetric(4, 4))
}

/// 内嵌分组（比卡片轻一档，用于面板内的分区）。
pub fn sub_frame(dark: bool) -> egui::Frame {
    egui::Frame::NONE
        .fill(Theme::of(dark).surface)
        .corner_radius(CornerRadius::same(radius::CTRL))
        .stroke(Stroke::new(1.0, Theme::of(dark).border))
        .inner_margin(egui::Margin::symmetric(8, 6))
}

// ---------------------------------------------------------------------------
// 字体
// ---------------------------------------------------------------------------

/// 优先使用系统里锐利且完整的字体：拉丁用 Segoe UI、等宽用 Consolas，
/// 中文用微软雅黑作为回退。这样拉丁字符不再被中文字体的软字形拖累，
/// 中文也不会缺字——比"整个界面都用雅黑"清晰得多。
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // 别写死 C:\Windows——不少机器系统盘在 D:/E: 上，写死会让所有
    // 自定义字体静默加载失败，退回 egui 内置字体（无中文）。
    let font_dir = std::env::var("SystemRoot")
        .map(|root| std::path::Path::new(&root).join("Fonts"))
        .unwrap_or_else(|_| std::path::Path::new(r"C:\Windows\Fonts").to_path_buf());

    let load = |name: &str| -> Option<Vec<u8>> { std::fs::read(font_dir.join(name)).ok() };

    let mut latin = false;
    if let Some(b) = load("segoeui.ttf") {
        fonts
            .font_data
            .insert("ui-latin".to_owned(), FontData::from_owned(b).into());
        latin = true;
    }
    let mut mono = false;
    if let Some(b) = load("consola.ttf") {
        fonts
            .font_data
            .insert("ui-mono".to_owned(), FontData::from_owned(b).into());
        mono = true;
    }

    // CJK：按顺序挑第一个存在的，作为中/日/韩字形的回退源。
    let cjk = [
        "msyh.ttc",
        "msyhbd.ttc",
        "Deng.ttf",
        "simsun.ttc",
        "msjh.ttc",
    ]
    .iter()
    .find_map(|f| load(f).map(|b| (f.to_string(), b)));
    let mut cjk_loaded = false;
    if let Some((_name, bytes)) = cjk {
        fonts
            .font_data
            .insert("ui-cjk".to_owned(), FontData::from_owned(bytes).into());
        cjk_loaded = true;
    }

    // 拉丁在前、CJK 紧随其后，egui 内置字体保留作最后兜底。
    if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
        if cjk_loaded {
            fam.insert(1, "ui-cjk".to_owned());
        }
        if latin {
            fam.insert(0, "ui-latin".to_owned());
        }
    }
    if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
        if cjk_loaded {
            fam.insert(1, "ui-cjk".to_owned());
        }
        if mono {
            fam.insert(0, "ui-mono".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// 字号表，从 [`fs`] 刻度派生（egui 默认正文 12.5 在中文界面上偏小偏糊，
/// 这里整体上调并拉开 Small/Body/Button/Heading 的层次）。
fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    [
        (TextStyle::Small, FontId::proportional(fs::SMALL)),
        (TextStyle::Body, FontId::proportional(fs::BODY)),
        (TextStyle::Button, FontId::proportional(fs::BUTTON)),
        (TextStyle::Heading, FontId::proportional(fs::HEADING)),
        (TextStyle::Monospace, FontId::monospace(fs::MONO)),
    ]
    .into()
}

// ---------------------------------------------------------------------------
// 样式
// ---------------------------------------------------------------------------

/// 一次性套用完整视觉（明暗主题 + 控件 + 间距 + 字号）。
/// 只在会话开始和主题/缩放变化时调用——每帧重建 Style 会让 egui 反复
/// 重新排版文字，直播模式下会明显卡顿。
pub fn apply_style(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let t = Theme::of(dark);

    // 全局文本色：egui 默认的灰白偏暗，提亮后清晰度立竿见影。
    visuals.override_text_color = Some(t.text);
    visuals.hyperlink_color = t.accent;
    visuals.faint_bg_color = t.surface;
    visuals.extreme_bg_color = t.surface;
    visuals.code_bg_color = t.surface;
    visuals.warn_fg_color = t.warn;
    visuals.error_fg_color = t.danger;

    // 画布 / 卡片 / 弹窗
    visuals.panel_fill = t.canvas;
    visuals.window_fill = t.card;
    visuals.window_stroke = Stroke::new(1.0, t.border);
    visuals.window_corner_radius = CornerRadius::same(radius::WIN);
    visuals.menu_corner_radius = CornerRadius::same(radius::CTRL);
    visuals.text_cursor = egui::style::TextCursorStyle {
        stroke: Stroke::new(2.0, t.accent),
        ..Default::default()
    };
    // 窗口与下拉/弹层加投影，浮动层次更明显（egui 默认阴影偏脏偏重）。
    visuals.window_shadow = shadow(dark, ShadowLevel::Window);
    visuals.popup_shadow = shadow(dark, ShadowLevel::Popup);

    // 选中态（文本选择、选中行）
    visuals.selection.bg_fill = t.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, t.accent);

    let r = CornerRadius::same(radius::CTRL);

    // 静态文本 / 分组框
    visuals.widgets.noninteractive.bg_fill = t.surface;
    visuals.widgets.noninteractive.weak_bg_fill = t.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    visuals.widgets.noninteractive.corner_radius = r;

    // 静息：淡填充 + 细描边，让按钮在卡片上有可辨识的轮廓
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.weak_bg_fill = t.surface;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, t.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    visuals.widgets.inactive.corner_radius = r;
    visuals.widgets.inactive.expansion = 0.0;

    // 悬停：强调色淡底 + 强调色描边
    visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.hovered.weak_bg_fill = t.accent_soft;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, t.accent);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    visuals.widgets.hovered.corner_radius = r;
    visuals.widgets.hovered.expansion = 0.5;

    // 按下 / 选中：实心强调色
    visuals.widgets.active.bg_fill = t.accent;
    visuals.widgets.active.weak_bg_fill = t.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, t.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, t.on_accent);
    visuals.widgets.active.corner_radius = r;
    visuals.widgets.active.expansion = 0.0;

    // 展开的下拉/菜单
    visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.weak_bg_fill = t.accent_soft;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, t.accent);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, t.text);
    visuals.widgets.open.corner_radius = r;

    // 层次与细节
    visuals.button_frame = true;
    visuals.collapsing_header_frame = true;
    visuals.indent_has_left_vline = true;
    visuals.striped = true;
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;
    visuals.clip_rect_margin = 4.0;
    visuals.resize_corner_size = 12.0;

    let mut style = (*ctx.global_style()).clone();
    style.visuals = visuals;
    style.text_styles = text_styles();
    // 弱文本（ui.weak 之类）单独降一档，形成稳定的三级层次。
    style.visuals.widgets.noninteractive.fg_stroke.color = t.text;

    // 间距：更舒展的呼吸感（默认 item_spacing 偏挤、按钮偏局促）。
    // 这里是全局间距策略的唯一出处。
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size = Vec2::new(40.0, 28.0);
    style.spacing.indent = 16.0;
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.icon_width = 15.0;
    style.spacing.icon_spacing = 8.0;
    style.spacing.combo_width = 140.0;
    style.spacing.text_edit_width = 220.0;
    style.spacing.tooltip_width = 420.0;
    // 细滚动条：不喧宾夺主，但位置始终可见。
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.animation_time = 0.12;

    ctx.set_global_style(std::sync::Arc::new(style));

    // text_dim 目前只被组件代码直接取用；这里消除 unused 字段告警的同时
    // 保证对比度口径一致。
    let _ = t.text_dim;
}

// ---------------------------------------------------------------------------
// 小组件
// ---------------------------------------------------------------------------

/// 分段控件（Segmented control）：一组互斥选项，装在一个浅色轨道里，
/// 选中项用强调色实心填充。比两个独立按钮更像现代工具栏。
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    options: &[(T, &str)],
    current: T,
    enabled: bool,
) -> Option<T> {
    let dark = ui.visuals().dark_mode;
    let mut clicked = None;
    let track = egui::Frame::NONE
        .fill(Theme::of(dark).surface)
        .stroke(Stroke::new(1.0, Theme::of(dark).border))
        .corner_radius(CornerRadius::same(radius::CTRL))
        .inner_margin(egui::Margin::same(3));
    track.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // 轨道已经提供了底色与边界，段本身只在选中/hover 时才显色。
            ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
            for (value, label) in options {
                let resp = ui.add_enabled(
                    enabled,
                    egui::Button::selectable(
                        *value == current,
                        egui::RichText::new(*label).size(fs::COMPACT),
                    ),
                );
                if resp.clicked() {
                    clicked = Some(*value);
                }
            }
        });
    });
    clicked
}

/// 缩小侧边面板（属性 / 层级树）的字号，让同屏能放下更多信息、
/// 长属性值不那么容易换行溢出。仅作用于调用处的 ui，不影响顶栏/中央。
/// 这里整体比全局刻度小一档，是"紧凑档"的唯一定义处。
pub fn compact_fonts(ui: &mut egui::Ui) {
    let st = ui.style_mut();
    // 整数字号（理由见 fs 模块文档）。
    st.text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(fs::MINI));
    st.text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(fs::COMPACT));
    st.text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(fs::SMALL));
    st.text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(fs::COMPACT));
    st.text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(fs::BUTTON));
    // 紧凑行距，信息更密而不乱。
    st.spacing.item_spacing = Vec2::new(6.0, 4.0);
    st.spacing.button_padding = Vec2::new(8.0, 4.0);
}

/// 面板标题：一条强调色竖条 + 标题，右侧可追加一个控件或说明。
/// 让三个面板的头部风格统一，也避免 17px 的 heading 显得过重。
pub fn panel_header(ui: &mut egui::Ui, title: &str, add_right: impl FnOnce(&mut egui::Ui)) {
    let t = Theme::of_ui(ui);
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, 15.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(bar, CornerRadius::same(radius::BAR), t.accent);
        ui.add_space(5.0);
        ui.label(egui::RichText::new(title).size(fs::BUTTON).color(t.text));
        add_right(ui);
    });
}

/// 顶部状态徽章（连接状态 / 回放进度等），比纯文本更好扫读。
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let t = Theme::of_ui(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(fs::SMALL), color);
    let pad = Vec2::new(8.0, 4.0);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let pill = CornerRadius::same((size.y / 2.0) as u8);
    ui.painter().rect_filled(rect, pill, t.surface);
    ui.painter().rect_stroke(
        rect,
        pill,
        Stroke::new(1.0, with_alpha(color, 90)),
        egui::StrokeKind::Middle,
    );
    ui.painter().galley(rect.min + pad, galley, color);
    resp
}
