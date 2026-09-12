# UI 优化方案评估(ui-redesign 分支)

> 状态:评估完成,路线已定。分支 `ui-redesign`,不动 main。

## 1. 现状问题

| 问题 | 根因 | 状态 |
|---|---|---|
| 低分屏文字发糊 | egui 0.27 用 ab_glyph,无字体 hinting,非整数缩放下重采样 | **根治(M1:egui 0.34 skrifa + hinting)** |
| 界面观感偏旧 | egui 默认控件风格,主题只做了配色层 | **M2 已落 design tokens + 组件统一**(观感打磨随 M3 继续) |
| 竖屏/窄窗布局 | 面板宽度不响应 | 已修(width_range 逐帧钳制 + **M3 侧栏自动收纳**) |

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
  - 待真机复核:live smoke test 帧率基线(264 帧) → **已复核(2026-09-12):见下**
- [x] **M2 视觉系统**:design tokens(色板/间距/圆角/阴影分级)、组件样式统一
  (按钮/输入框/下拉/滚动条/树),整体走"深色工具"风格
  - theme.rs 重构为 token 中心:`Theme` 语义色集(15 字段,`of(dark)`/`of_ui(ui)` 解析)、
    `fs` 字号刻度(8 档全整数)、`radius` 圆角刻度(6 档)、`overlay` 截图覆盖层常量、
    `shadow` 三档分级(Card/Popup/Window)、`with_alpha` 唯一半透明途径
  - 旧 `c_x(dark)` 自由函数删除,app.rs ~30 处调用迁移到 token;间距策略唯一出处是
    `apply_style`
  - 统一决策:截图选中框(0,180,255)与树行(0,150,255)归并为同一 `overlay::SELECT`;
    步骤列表红/绿、属性文本灰、REC/回放徽章底色全部改语义 token;散落字号
    (11/12/13/14/15/16)全部挂到 `fs` 刻度
  - 覆盖层色定为常量而非主题字段:设备截图不随主题变,树行与截图框共用同组色,
    保证同一元素两个视图颜色一致
- [x] **M3 布局细节**:窄窗/竖屏进一步优化(属性面板可自动收纳)、空状态/加载态
  - 侧栏自动收纳:窗口 <700px 双侧栏收起、<880px 收右侧(层级树),阈值按窗口全宽
    判定,开合不会反馈进决策(无振荡);顶栏新增「属性/层级」(操作模式:设备/录制)
    开关 chips,点击即锁定该侧开闭,本会话不再自动收纳
  - 欢迎页改走 ScrollArea:centered_and_justified 按内容固有宽度居中,窄窗长行会被
    裁掉;改 ScrollArea + vertical_centered 后按面板宽度正常换行、超高可滚动
  - 加载态:抓取期间层级树显示"正在抓取界面…"、中央区显示 spinner + "正在抓取设备
    屏幕…";中央空状态文案 dim 化
  - 顺手:版本号 11.5px 非整数字号修复(fs::MINI),字号刻度自此全量合规
- [x] **M4 复评**:对照 0.35 样式系统评估是否继续跟进
  - 结论:**暂不升级,留 0.34.3**。0.35(2026-06-25)的 classes 只覆盖自定义 widget
    ("next step 才是内置 widget 样式化"),现在升拿不到核心收益;`Remove impl Into
    arguments`(#8194)会打掉 painter/Frame 大量调用点,迁移面又是一轮 M1 规模机械替换;
    harfrust 排版与 IME 改进对桌面中文工具收益边际
  - 已提前消化的升级成本:M1/M2 把 deprecated API 清零(clippy 零警告),将来升级
    只剩 impl Into 移除、Panel 方法改名(#8192)、clip_rect_margin 移除三件事
  - 重新评估触发条件:① classes 能样式化内置 widget;② 需要 egui_inspection/
    egui_mcp(EGUI_INSPECTION=1,agent 可视化调试 UI)——对"AI 辅助调 UI"有真实吸引力;
  - 0.35 起短期跟进的 patch(0.35.x)MSRV 1.95,与本地工具链一致,无额外成本

## 5. 验收口径

- 任意 DPI(100%/125%/150%/200%)文字清晰不发糊(对照截图)
- release 包体:**实测 6.04MB**(基线 3.7MB)。skrifa/read-fonts/vello 系列 + 新版
  idna/icu 是 0.34 字体栈的固定成本,release profile 已拉满(z/lto/cgu=1/abort/strip),
  ≤4.5MB 的原目标不现实;唯一可再省的 ~1MB 是砍 `default_fonts`(改用纯系统字体),
  但会引入 UI 符号(⚙↻ 等)缺字形的 tofu 风险,不做。清晰度优先于包体。
- live 模式无回归:**已真机复核(2026-09-12)**。smoke 四项判据(连接/控制通道/注入/
  帧流>0)全 PASS;**帧数受设备画面内容影响**(静止桌面 ~87-99,相机取景器 ~115-125),
  旧记的"264 帧基线"是当时特定画面状态下的一次性数字,不可复现、不宜作基线。
  硬对照:git worktree 检出 main(迁移前代码)同状态(相机前台)跑同款 smoke,
  旧 101 帧 vs 新 100 帧(1% 噪声内)——**M1 迁移零回归**。smoke 无 GUI,egui 版本
  本就不参与;PASS 判据 + 同条件对照才是可复现的验收口径。
- `cargo clippy --all-targets -- -D warnings` 全绿(含 clippy 1.95 新 lint)
