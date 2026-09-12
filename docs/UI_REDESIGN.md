# UI 优化方案评估(ui-redesign 分支)

> 状态:评估完成,路线已定。分支 `ui-redesign`,不动 main。

## 1. 现状问题

| 问题 | 根因 | 状态 |
|---|---|---|
| 低分屏文字发糊 | egui 0.27 用 ab_glyph,无字体 hinting,非整数缩放下重采样 | **根治(M1:egui 0.34 skrifa + hinting)** |
| 界面观感偏旧 | egui 默认控件风格,主题只做了配色层 | 待做(视觉系统重做) |
| 竖屏/窄窗布局 | 面板宽度不响应 | 已修(width_range 逐帧钳制) |

## 2. 候选方案对比

| 方案 | 文字质量 | 观感上限 | 改造成本 | 包体 | 结论 |
|---|---|---|---|---|---|
| **egui 0.27 → 0.34.x** | **skrifa + vello_cpu + 字体 hinting,官方"Sharper text"** | 中高(0.35 起 CSS-like classes、atoms) | 中(API 迁移,无重写) | 略增(skrifa/vello 约 +0.5~1MB) | **推荐** |
| egui 0.35 | 同上 + harfrust 排版 | 同上 | 大(移除所有 deprecated:`App::update`→`App::ui`、统一 Panel、去 `impl Into` 参数) | 同上 | 0.34 站稳后再升 |
| iced | cosmic-text,质量好 | 高(自带现代主题) | 全重写 UI 层 + Elm 架构重学;视频纹理要走 wgpu | 中 | 不值:无明确收益 |
| Slint | 好(自绘) | 高(内置 Fluent/Material 风格) | 全重写 + **GPLv3/商业双许可**,个人工具可忍但有隐患 | 中 | 许可证是硬伤 |
| Tauri v2 (WebView2) | 最好(DirectWrite/ClearType) | 无上限(HTML/CSS) | 架构重写:前后端分离、60fps 视频流要经 IPC/canvas,延迟与 CPU 代价大 | exe 小但依赖 WebView2 运行时 | 只有"网页级观感"是刚需才选 |
| 原生 WinUI | 原生最佳 | 高 | 全重写,绑定 Windows | 小 | 否 |

## 3. 推荐路线

**留在 egui,升级到 0.34.x,然后做一轮视觉系统重做。**

决定性理由:0.34 的字体后端重写(skrifa + vello_cpu + hinting)正面解决这个项目最大的
痛点——任何 DPI、任何缩放比下的文字清晰度,而且这是"白拿"的,不需要重写任何业务逻辑。
其他框架换来的主要是观感上限,但都要付全量重写 + 视频管线重接的代价;而本项目 60fps
scrcpy 视频纹理、录制回放、u2 桥接全部长在 egui 上。

0.35 的 classes/atoms 样式系统值得跟进,但它移除了全部 deprecated API,迁移面大一圈;
先落 0.34(deprecated 仍可用),后续增量升级。

## 4. 路线图

- [x] **M1 升级迁移**:eframe/egui 0.27 → 0.34.3,修完 API 变更,构建+测试全绿(48/48)
  - 主要变更:`App::update`→`App::ui(ui,frame)`(需 `ui.ctx().clone()` 取 Context)、
    统一 `Panel::top/left/right` + `default_size/size_range` + `show_inside`、
    `Rounding`→`CornerRadius`(u8)、`Margin`/`Shadow` 字段整型化、`Frame::NONE` 常量、
    `StrokeKind`(注意变体是驼峰 `Middle` 不是 `MIDDLE`)、`id_salt`、`.range`、
    `ctx.copy_text`、`global_style/set_global_style`、`content_rect`、`Button::selectable`、
    `FontData` 需 `Arc`(`.into()`)
  - 顺手对齐了 rustfmt/clippy 1.95 工具链门禁(全量格式化 + 老代码 lint 修复,
    与迁移分开提交)
  - 待真机复核:live smoke test 帧率基线(264 帧)
- [ ] **M2 视觉系统**:design tokens(色板/间距/圆角/阴影分级)、组件样式统一
  (按钮/输入框/下拉/滚动条/树),整体走"深色工具"风格
- [ ] **M3 布局细节**:窄窗/竖屏进一步优化(属性面板可自动收纳)、空状态/加载态
- [ ] **M4 复评**:对照 0.35 样式系统评估是否继续跟进

## 5. 验收口径

- 任意 DPI(100%/125%/150%/200%)文字清晰不发糊(对照截图)
- release 包体:**实测 6.04MB**(基线 3.7MB)。skrifa/read-fonts/vello 系列 + 新版
  idna/icu 是 0.34 字体栈的固定成本,release profile 已拉满(z/lto/cgu=1/abort/strip),
  ≤4.5MB 的原目标不现实;唯一可再省的 ~1MB 是砍 `default_fonts`(改用纯系统字体),
  但会引入 UI 符号(⚙↻ 等)缺字形的 tofu 风险,不做。清晰度优先于包体。
- live 模式帧率无回归(smoke test 264 帧基线,M1 后待真机复核)
- `cargo clippy --all-targets -- -D warnings` 全绿(含 clippy 1.95 新 lint)
