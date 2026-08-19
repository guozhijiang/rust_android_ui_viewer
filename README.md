# Android UI Viewer

基于 Rust + [eframe/egui](https://github.com/emilk/egui) 的 Android 界面层级查看器（类似 `uiautomatorviewer`）。通过 adb 抓取设备当前界面的截图与 UI 层级（XML），直观地查看控件树、控件边界与属性。

## 界面预览

![Android UI Viewer 主界面](assets/screenshot.png)

## 功能

- **一键抓取**：通过 adb 执行 `screencap` 和 `uiautomator dump`，同时获取截图与界面层级。
- **本地导入**：支持将截图（png/jpg/jpeg）和 uiautomator XML 拖入窗口或通过文件对话框加载。
- **三栏布局**：
  - 左侧：元素属性（class、resource-id、text、bounds 等，全部显示无需滚动）。
  - 中间：截图，可缩放（0.25x~5x）、拖拽平移，并叠加控件边界高亮。
  - 右侧：UI 层级结构树（默认较宽，支持横向/纵向滚动）。
- **双向联动高亮**：
  - 点击截图任意位置，自动选中最内层控件并定位到层级树对应节点。
  - 点击/悬停层级树中的节点，截图上的对应控件同步高亮。
- **搜索过滤**：按任意属性关键字过滤节点；选中节点时自动展开其祖先链。
- **坐标信息**：顶栏实时显示鼠标在截图上的像素坐标，以及当前选中控件的边界与尺寸。
- **中文字体**：自动加载系统 CJK 字体，保证属性中文正常显示。

## 环境要求

- Rust 工具链（`cargo`）
- adb 且已接入一台 Android 设备（仅使用「Capture」功能时需要；浏览本地文件不需要）

## 构建与运行

```bash
cargo run           # 调试运行
cargo build --release && ./target/release/android-ui-viewer.exe
```

## 使用说明

1. 连接设备并确认 `adb devices` 能识别到设备。
2. 点击顶栏 **`📱 Capture (adb)`** 抓取当前屏幕。
3. 或直接把 `.png` / `.xml` 文件拖入窗口。
4. 在截图或层级树中点击元素，即可查看属性并高亮对应控件。

## 项目结构

```
src/
├── main.rs      # 程序入口与窗口配置
├── app.rs       # GUI 布局、树渲染、属性展示、截图覆盖层
├── adb.rs       # adb 截图与 uiautomator dump 封装
└── ui_tree.rs   # UI 层级 XML 解析、命中检测、节点查询
```