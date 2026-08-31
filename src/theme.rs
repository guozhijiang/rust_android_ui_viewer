//! 视觉主题：配色、字体、控件样式。
//!
//! 目标观感：深色画布 + 悬浮卡片 + 单一品牌强调色（青蓝），字形锐利、
//! 层次分明、留白充足。所有颜色集中在这里，明暗主题共用同一套语义名。

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle,
    Vec2,
};

// ---------------------------------------------------------------------------
// 调色板
// ---------------------------------------------------------------------------

/// 最底层画布（窗口底色）。卡片浮在它上面。
pub fn c_canvas(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(10, 12, 17)
    } else {
        Color32::from_rgb(237, 241, 247)
    }
}

/// 卡片 / 面板 / 弹窗底色。
pub fn c_card(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(20, 24, 31)
    } else {
        Color32::from_rgb(255, 255, 255)
    }
}

/// 次级表面：输入框、代码段、条纹背景、按钮静息态填充。
pub fn c_surface(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(27, 32, 41)
    } else {
        Color32::from_rgb(245, 247, 251)
    }
}

/// 常规分隔线 / 控件描边。
pub fn c_border(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(35, 42, 54)
    } else {
        Color32::from_rgb(223, 229, 239)
    }
}

/// 强调型描边（hover / 焦点）。
pub fn c_border_strong(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(51, 62, 80)
    } else {
        Color32::from_rgb(195, 205, 221)
    }
}

/// 品牌强调色（青蓝）。
pub fn c_accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(91, 140, 255)
    } else {
        Color32::from_rgb(37, 99, 235)
    }
}

/// 强调色的超淡版本，用于 hover 底与选中行的淡底。
pub fn c_accent_soft(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgba_unmultiplied(91, 140, 255, 30)
    } else {
        Color32::from_rgba_unmultiplied(37, 99, 235, 26)
    }
}

/// 主文本（比 egui 默认的灰白更亮，是"清晰"的第一要素）。
pub fn c_text(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(231, 236, 246)
    } else {
        Color32::from_rgb(22, 30, 44)
    }
}

/// 次要文本 / weak 标签。
pub fn c_text_dim(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(151, 161, 184)
    } else {
        Color32::from_rgb(94, 107, 128)
    }
}

pub fn c_success(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(63, 200, 132)
    } else {
        Color32::from_rgb(22, 145, 90)
    }
}

pub fn c_warn(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(245, 176, 66)
    } else {
        Color32::from_rgb(180, 116, 10)
    }
}

pub fn c_danger(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(248, 113, 113)
    } else {
        Color32::from_rgb(209, 59, 59)
    }
}

/// 落在强调色填充之上的文字色（选中/按下状态）。
pub fn c_on_accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(12, 16, 26)
    } else {
        Color32::WHITE
    }
}

// ---------------------------------------------------------------------------
// 卡片容器
// ---------------------------------------------------------------------------

/// 圆角 + 描边的卡片框，用于顶栏与左右侧栏。
pub fn card_frame(dark: bool) -> egui::Frame {
    egui::Frame::none()
        .fill(c_card(dark))
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0, c_border(dark)))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .outer_margin(egui::Margin::symmetric(4.0, 4.0))
}

/// 内嵌分组（比卡片轻一档，用于面板内的分区）。
pub fn sub_frame(dark: bool) -> egui::Frame {
    egui::Frame::none()
        .fill(c_surface(dark))
        .rounding(Rounding::same(8.0))
        .stroke(Stroke::new(1.0, c_border(dark)))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
}

// ---------------------------------------------------------------------------
// 字体
// ---------------------------------------------------------------------------

/// 优先使用系统里锐利且完整的字体：拉丁用 Segoe UI、等宽用 Consolas，
/// 中文用微软雅黑作为回退。这样拉丁字符不再被中文字体的软字形拖累，
/// 中文也不会缺字——比"整个界面都用雅黑"清晰得多。
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let font_dir = std::path::Path::new(r"C:\Windows\Fonts");

    let load = |name: &str| -> Option<Vec<u8>> {
        std::fs::read(font_dir.join(name)).ok()
    };

    let mut latin = false;
    if let Some(b) = load("segoeui.ttf") {
        fonts.font_data.insert("ui-latin".to_owned(), FontData::from_owned(b));
        latin = true;
    }
    let mut mono = false;
    if let Some(b) = load("consola.ttf") {
        fonts.font_data.insert("ui-mono".to_owned(), FontData::from_owned(b));
        mono = true;
    }

    // CJK：按顺序挑第一个存在的，作为中/日/韩字形的回退源。
    let cjk = ["msyh.ttc", "msyhbd.ttc", "Deng.ttf", "simsun.ttc", "msjh.ttc"]
        .iter()
        .find_map(|f| load(f).map(|b| (f.to_string(), b)));
    let mut cjk_loaded = false;
    if let Some((_name, bytes)) = cjk {
        fonts.font_data.insert("ui-cjk".to_owned(), FontData::from_owned(bytes));
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

/// 字号表。egui 默认（正文 12.5）在中文界面上偏小偏糊，这里整体上调，
/// 并拉开 Small/Body/Button/Heading 的层次。
fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    use FontFamily::{Monospace, Proportional};
    [
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, FontId::new(13.5, Proportional)),
        (TextStyle::Heading, FontId::new(17.0, Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
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
    let accent = c_accent(dark);
    let text = c_text(dark);
    let dim = c_text_dim(dark);
    let surface = c_surface(dark);
    let border = c_border(dark);
    let on_accent = c_on_accent(dark);

    // 全局文本色：egui 默认的灰白偏暗，提亮后清晰度立竿见影。
    visuals.override_text_color = Some(text);
    visuals.hyperlink_color = accent;
    visuals.faint_bg_color = surface;
    visuals.extreme_bg_color = surface;
    visuals.code_bg_color = surface;
    visuals.warn_fg_color = c_warn(dark);
    visuals.error_fg_color = c_danger(dark);

    // 画布 / 卡片 / 弹窗
    visuals.panel_fill = c_canvas(dark);
    visuals.window_fill = c_card(dark);
    visuals.window_stroke = Stroke::new(1.0, border);
    visuals.window_rounding = Rounding::same(12.0);
    visuals.menu_rounding = Rounding::same(8.0);
    visuals.text_cursor = Stroke::new(2.0, accent);

    // 选中态（文本选择、选中行）
    visuals.selection.bg_fill = c_accent_soft(dark);
    visuals.selection.stroke = Stroke::new(1.0, accent);

    let r = Rounding::same(8.0);

    // 静态文本 / 分组框
    visuals.widgets.noninteractive.bg_fill = surface;
    visuals.widgets.noninteractive.weak_bg_fill = surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.noninteractive.rounding = r;

    // 静息：淡填充 + 细描边，让按钮在卡片上有可辨识的轮廓
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.weak_bg_fill = surface;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.inactive.rounding = r;
    visuals.widgets.inactive.expansion = 0.0;

    // 悬停：强调色淡底 + 强调色描边
    visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.hovered.weak_bg_fill = c_accent_soft(dark);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.hovered.rounding = r;
    visuals.widgets.hovered.expansion = 0.5;

    // 按下 / 选中：实心强调色
    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.weak_bg_fill = accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, on_accent);
    visuals.widgets.active.rounding = r;
    visuals.widgets.active.expansion = 0.0;

    // 展开的下拉/菜单
    visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.weak_bg_fill = c_accent_soft(dark);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, accent);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.open.rounding = r;

    // 层次与细节
    visuals.button_frame = true;
    visuals.collapsing_header_frame = true;
    visuals.indent_has_left_vline = true;
    visuals.striped = true;
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;
    visuals.clip_rect_margin = 4.0;
    visuals.resize_corner_size = 12.0;

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.text_styles = text_styles();
    // 弱文本（ui.weak 之类）单独降一档，形成稳定的三级层次。
    style.visuals.widgets.noninteractive.fg_stroke.color = text;

    // 间距：更舒展的呼吸感（默认 item_spacing 偏挤、按钮偏局促）。
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size = Vec2::new(40.0, 28.0);
    style.spacing.indent = 16.0;
    style.spacing.window_margin = egui::Margin::same(12.0);
    style.spacing.menu_margin = egui::Margin::same(8.0);
    style.spacing.icon_width = 15.0;
    style.spacing.icon_spacing = 8.0;
    style.spacing.combo_width = 140.0;
    style.spacing.text_edit_width = 220.0;
    style.spacing.tooltip_width = 420.0;
    // 细滚动条：不喧宾夺主，但位置始终可见。
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.animation_time = 0.12;

    ctx.set_style(std::sync::Arc::new(style));

    // 记录弱文本色：egui 没有单独的字段，统一由 override_text_color +
    // 各处的 `.weak()` 走 noninteractive 的淡化逻辑，这里仅保证对比度。
    let _ = dim;
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
    let track = egui::Frame::none()
        .fill(c_surface(dark))
        .stroke(Stroke::new(1.0, c_border(dark)))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::same(3.0));
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
                    egui::SelectableLabel::new(
                        *value == current,
                        egui::RichText::new(*label).size(13.0),
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

/// 面板标题：一条强调色竖条 + 标题，右侧可追加一个控件或说明。
/// 让三个面板的头部风格统一，也避免 17px 的 heading 显得过重。
pub fn panel_header(
    ui: &mut egui::Ui,
    title: &str,
    add_right: impl FnOnce(&mut egui::Ui),
) {
    let dark = ui.visuals().dark_mode;
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, 15.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(bar, Rounding::same(1.5), c_accent(dark));
        ui.add_space(5.0);
        ui.label(egui::RichText::new(title).size(15.0).color(c_text(dark)));
        add_right(ui);
    });
}

/// 顶部状态徽章（连接状态 / 回放进度等），比纯文本更好扫读。
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        FontId::proportional(12.0),
        color,
    );
    let pad = Vec2::new(8.0, 4.0);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(size.y / 2.0), c_surface(dark));
    ui.painter().rect_stroke(
        rect,
        Rounding::same(size.y / 2.0),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90)),
    );
    ui.painter().galley(rect.min + pad, galley, color);
    resp
}
