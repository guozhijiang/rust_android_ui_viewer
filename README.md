# Android UI Viewer

基于 Rust + [eframe/egui](https://github.com/emilk/egui) 的 Android 界面层级查看器（类似 `uiautomatorviewer`）。通过 adb 抓取设备当前界面的截图与 UI 层级（XML），直观地查看控件树、控件边界与属性。

## 界面预览

![Android UI Viewer 操作模式实时控制](assets/screenshot.png)

## 功能

- **一键抓取**：通过 adb 同时获取截图（`screencap`）与界面层级 XML。层级优先走 **u2（uiautomator2）快速抓取**——比 `uiautomator dump` 快、且对部分机型/高版本系统可用的唯一途径；未配置 u2 时自动回退到 `uiautomator dump`。
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

## 操作模式（scrcpy 实时控制）

除了静态抓取，本工具还内置了「操作模式」：通过 scrcpy 在设备端拉起视频流，本地用 FFmpeg（scrcpy 自带的 `avcodec`/`avutil` DLL）解码 H.264 并显示实时画面，同时**经由 scrcpy 控制通道**把触摸/按键实时下发到设备。

- 在顶栏切换到「操作模式」即开始会话（首次需设置 scrcpy 目录，例如 `D:\scrcpy-win64-v4.0`，需包含 `scrcpy-server` 与 `avcodec-62.dll`/`avutil-60.dll`）。
- 直接在画面上**点击 = 轻触、按住拖动 = 滑动、长按 = 长按**，右键 = 返回；这些都走 scrcpy 控制通道，延迟远低于 `adb shell input`。
- **实体键盘透传**：鼠标悬停在手机画面上时，物理键盘直接输入到设备——字母/数字/标点走文本通道（保留大小写与中文输入法），方向键/回车/删除/Home/End/翻页/F1–F12 等走按键通道；Ctrl/Cmd+字母（如 Ctrl+C）作为组合键下发（含 Shift/Alt/Ctrl/Meta 修饰符）。
- **双指操作**：在画面上按住**右键并拖动**即模拟第二根手指，可双指捏合缩放；单纯右键点击仍作为「返回」。
- **鼠标滚轮滚动**：鼠标悬停在手机画面上时，滚轮直接转为设备的 `AMOTION_EVENT_AXIS_VSCROLL/HSCROLL`（走 scrcpy 独立的 inject_scroll 消息，21 字节协议）；向下滚 = 内容上滚，符合直觉。
- 顶栏按钮：返回 / 主页 / 最近 / 电源 / 音量±，以及文本输入框与「抓取层级」（抓取当前界面 XML 叠加到画面上，可点选元素）。
- 触摸坐标以**视频帧分辨率**为基准、由设备端按当前分辨率缩放，因此设置「最大尺寸」也不会错位。
- 若控制通道建立失败（如旧版 server），会自动回退到 `adb shell input`，状态栏会提示。

## 左面板：设备与应用管理（操作模式）

进入「操作模式」后，左侧面板提供三个标签（数据在连接设备后自动加载，也支持右上角「↻ 刷新」手动刷新）：

- **设备信息**：机型 / 品牌 / 系统版本（Android + API）/ 分辨率 / DPI / 电量 / 存储 / 软件版本 / 序列号。
- **应用**：应用列表（全部 / 三方 / 系统 / 运行中）与包名搜索；点选后显示版本、安装时间等属性，并可 **启动 / 强制停止 / 清除数据 / 打开应用设置 / 卸载**（卸载仅三方应用可用）；底部支持 **从本地选择 APK 安装**（后台安装，不卡界面）。
- **系统设置**：一键直达 Wi-Fi、蓝牙、声音、显示、通知、应用、电池、辅助功能等系统设置页；以及设备快捷操作（锁屏 / 主页 / 返回 / 最近任务 / 音量± / 亮度分档与自动亮度）。

## 录制与回放

「操作模式」左面板的**录制控制**区可以把在设备上的操作录制成脚本，之后一键回放：

- **开始录制**后，画面上的点击 / 长按 / 滑动、键盘输入与实体按键都会被记录；左上角出现 `● 录制中` 指示，停止时弹窗选择 YAML 保存位置。
- 每一步同时记录**两套定位**：控件 `UiSelector`（resource-id / text / content-desc / class，取自录制时的层级树）与**分数坐标** `fx/fy ∈ 0..1`——回放时优先用 selector 在当前层级树里定位（容忍界面微调），失败再回退到分数坐标 × 当前分辨率。
- 每步还带**前台应用注解**（app/activity，`dumpsys` 结果缓存约 1.5s），便于人工审阅脚本。
- **回放**支持倍速（1.0 = 录制节奏，2.0 = 两倍速）与循环次数；回放中左上角显示 `▶ 回放中`，逐步上报进度，selector 解析失败的步骤会标红提示。
- YAML 录制文件与 **web 版双向互通**（同一套 snake_case 字段格式）。

## Web 版（`web/`）

`web/` 目录提供一个浏览器版本（FastAPI 后端 + 原生 JS 前端，视频流走 WebCodecs），功能与桌面版基本对齐：静态抓取检视、scrcpy 实时控制、右键复制节点信息、录制与回放（与桌面版 YAML 双向互通）。

```bash
cd web && uvicorn backend.main:app --port 8000   # 需 Python 3.10+ 与 adb
```

详见 **[web/README.md](web/README.md)**（三模式说明、互通矩阵、41 个 API 速览、E2E 用法）。

## u2（uiautomator2）加速

部分 Android 真机 / 高版本系统上 `uiautomator dump` 会失败（不产出 XML），本工具自动依赖 **u2** 服务实现快速抓取：

- 工具会从配置里指定的 `u2_core.jar`（或默认路径 `%USERPROFILE%\.u2\u2_core.jar`）推送到设备，经 `adb forward` 暴露本地 JSON-RPC 端口，以 `app_process` 启动服务。
- 抓取层级 / 实时刷新 / 录制回放定位元素时，均优先走 u2 的快速 `dumpWindowHierarchy`；u2 不可用时自动回退到 adb dump。
- 启动软件时会自动尝试启用 u2；可在「配置」窗口手动指定 jar 路径并重新「推送并启动」。

## 环境要求

- Rust 工具链（`cargo`）
- adb 且已接入一台 Android 设备（仅使用「Capture」功能时需要；浏览本地文件不需要）
- **操作模式（实时控制）需要 scrcpy**：PC 端需有 scrcpy v4.0 的 Windows 包（含 `scrcpy-server` 与 `avcodec-62.dll` / `avutil-60.dll`）。
  - 优先使用界面里填写的「scrcpy 目录」，或环境变量 `SCRCPY_SERVER_PATH`，或用 `where scrcpy` 自动探测本机已安装的 scrcpy。
  - **若都找不到，工具会在首次启动时自动从官方下载 `scrcpy-win64-v4.0.zip`，只抽取所需文件到可执行文件旁的 `scrcpy-bundle/` 缓存目录（仅下载一次，之后复用）。**
  - 也可手动下载：<https://github.com/Genymobile/scrcpy/releases>（取 `scrcpy-win64-v4.0.zip` 解压即可）。

## 构建与运行

```bash
cargo run           # 调试运行
cargo build --release && ./target/release/android-ui-viewer.exe
```

本地编译、CI 自动构建并发布 exe 到 GitHub Releases 的完整逻辑与操作步骤，见 **[BUILD_AND_RELEASE.md](BUILD_AND_RELEASE.md)**。

## 验证 / 测试

实时操作模式（scrcpy 控制通道）有两层验证：

- **真机端到端冒烟测试**（需要一台已 `adb` 授权连接的设备）：
  ```bash
  cargo run --example smoke
  ```
  它会拉起真实的 `live::start` 会话，确认「视频 + 控制双连接」建立，
  并注入 点按 + BACK + 文本，再确认视频帧仍在持续到达。全部通过则输出
  `RESULT: PASS`。

- **离线协议测试**（无需设备，CI 友好）：
  ```bash
  cargo run --example control_protocol_test
  ```
  用一个模拟的 scrcpy 控制服务器接收真实 `LiveControl` 发出的字节，
  逐字段断言触摸(32B)/按键(14B)/文本(len+utf8)/滚动(21B) 完全符合 scrcpy v4.0 线格式。
  输出 `RESULT: PASS` 即代表序列化与官方协议字节级一致。

## 使用说明

1. 连接设备并确认 `adb devices` 能识别到设备。
2. 点击顶栏 **`📱 Capture (adb)`** 抓取当前屏幕。
3. 或直接把 `.png` / `.xml` 文件拖入窗口。
4. 在截图或层级树中点击元素，即可查看属性并高亮对应控件。
5. 需要实时操作或录制脚本时，切换到 **`操作模式`**（首次会自动拉起 scrcpy），画面即设备实时画面，可直接触摸/键盘操控；左面板「录制控制」可录制与回放。

## 项目结构

```
src/
├── main.rs      # 程序入口与窗口配置
├── lib.rs       # 模块声明（app / adb / ui_tree / live / scrcpy / record / log / u2）
├── app.rs       # GUI 布局、树渲染、属性展示、截图覆盖层、操作模式交互、左面板设备/应用管理
├── adb.rs       # adb 封装：截图、UI dump、设备属性、应用安装/启动/停止/卸载、系统设置跳转
├── ui_tree.rs   # UI 层级 XML 解析、命中检测、节点查询
├── u2.rs        # uiautomator2 (u2.jar) 集成：jar 推送、服务启动、JSON-RPC 快速抓层级
├── live.rs      # scrcpy 实时会话：推送 server、拉流、控制通道、自动下载 scrcpy 包
├── scrcpy.rs    # 动态加载 FFmpeg DLL 软解 H.264 → RGBA
├── record.rs    # 录制与回放：步骤采集、selector 定位、YAML 持久化
└── log.rs       # 日志初始化与滚动日志
```

## 参考与出处

本工具的实时操作模式与视频解码直接基于以下开源项目，协议与线格式均对照其官方源码实现：

- **[scrcpy](https://github.com/Genymobile/scrcpy)**（Genymobile，Apache-2.0）
  - 实时控制模式的核心：设备端 `scrcpy-server` 推流 + 控制通道（触摸 / 按键 / 文本 / 滚动）的线格式，均对照 scrcpy v4.0 源码（`DesktopConnection.java` / `ControlMessageReader.java`）实现。
  - PC 端视频解码复用 scrcpy 自带的 FFmpeg（`avcodec` / `avutil`）动态库软解 H.264。
  - 官方发布包（含 server 与 DLL）：<https://github.com/Genymobile/scrcpy/releases>
- **[egui / eframe](https://github.com/emilk/egui)**（emilk，MIT）
  - 全部 GUI（三栏布局、截图覆盖层、属性面板、层级树）基于 egui 即时模式框架，eframe 提供窗口与事件循环。
- **[uiautomator](https://developer.android.com/tools/help/uiautomator)**（Android 官方）
  - 「Capture」模式的 UI 层级来源：`adb shell uiautomator dump` 产出的控件树 XML，以及 `adb exec-out screencap -p` 截取的屏幕图像。
- **[uiautomator2](https://github.com/openatx/openatx)**（openatx/openatx，MIT）
  - 快速抓取所用的 `u2_core.jar`（`com.wetest.uia2.Main`，`dumpWindowHierarchy` JSON-RPC）：`<https://github.com/openatx/android-uiautomator-server-jar>`（源包内 `assets/u2.jar`，v0.4.0）。用于替代部分设备上不可用的 `uiautomator dump`。
- **[FFmpeg](https://ffmpeg.org/)**（LGPL/GPL）
  - 通过 scrcpy 附带的 FFmpeg 动态库（`avcodec-62.dll` / `avutil-60.dll`，FFmpeg 7.x）完成 H.264 解码。