# Android UI Viewer — Web 版

基于 **FastAPI** 的网页版 Android 界面查看器，移植了原 Rust/egui 桌面应用的两大核心模式：

## 检视模式（Capture & Inspect）

- **一键抓取**：通过 adb 同时获取截图（`screencap`）与 UI 层级 XML（`uiautomator dump`）。
- **本地导入**：上传截图（png/jpg）与 uiautomator XML，无需连接设备。
- **三栏布局**：
  - 左：选中节点的全部属性（class / resource-id / text / bounds 等）。
  - 中：截图，支持缩放（滚轮）、拖拽平移，并叠加控件边界高亮。
  - 右：UI 层级结构树（可折叠）。
- **双向联动高亮**：点击截图上任意位置自动选中**最内层**控件并定位到层级树；点击/悬停层级树节点，截图上对应控件同步高亮（并淡显祖先链）。
- **搜索过滤**：按任意属性关键字过滤层级树。

## 实时模式（Live / scrcpy）

浏览器无法直接解码裸 H.264，本模式利用 **Chrome/Edge 的 WebCodecs** 在浏览器端软解，实现与桌面版一致的实时镜像与控制：

- **实时画面**：后端启动 scrcpy v4.0 server，H.264 Annex-B 流经 WebSocket 转发，前端 `VideoDecoder` 解码渲染到 canvas。
- **实时操作**（走 scrcpy 控制通道，非 adb 模拟，与桌面版操作语义对齐）：
  - **左键**按下/拖动/松开 = 触摸（DOWN/MOVE/UP，拖动实时转发，单击即点按）
  - **右键** = 第二根手指（双指捏合/拖动）；右键单击（无拖动）= 返回键（BACK）
  - **滚轮** = 滚动（与桌面版同系数 0.04）
  - **屏幕外控件**（仿真实手机布局，不遮画面）：
    - 右侧竖排：电源 / 音量+ / 音量−
    - 画面下方导航栏：返回 / 主页 / 最近任务
  - **键盘**（「键盘」开关开启后）：
    - 可打印字符（字母/数字/标点/空格）→ **文本注入**（保留大小写与 IME）
    - 方向键 / Esc / Tab / Enter / Backspace / Delete / Insert / Home / End / PageUp / PageDown / F1~F12 → Android keycode
    - **Ctrl/Cmd 组合**（如 Ctrl+C/V）→ keycode + meta 直发设备，快捷键可达设备
    - 支持**粘贴**（Ctrl+V 剪贴板内容 → 文本注入）
  - 文本输入框回车 = 注入文本
- **UI 叠加查看**：点「抓取 UI 树」实时 dump 层级，控件 bounds 按视频缩放比例（X/Y 独立）叠加到画面上，点击画面命中节点、属性面板联动、支持搜索。
  - 右侧层级标题会实时显示当前缩放参数（如 `scale 0.544×0.546 588×1280→1080×2344`），可据此核对叠加精度。
  - 叠加以**解码帧实际尺寸**为基准（自动跟随 scrcpy 取整与旋转），分辨率变化后自动重画，不会残留旧框。
  - 注意：**桌面/锁屏界面 uiautomator 树节点极少**（vivo OriginOS 桌面图标不暴露 bounds），叠加基本空白属正常；
    在设置、文件管理等列表型应用下叠加精度为像素级。
- **参数可调**：视频分辨率上限（0=原始 / 1920 / 1280 / 720）与码率（2M~16M）。
- **断开/刷新页面即释放设备会话**；会话异常中断时（如设备端进程被系统清理）后端自动重连，画面几秒内恢复，无需手动操作。

> 浏览器要求：Chrome / Edge（需支持 WebCodecs + H.264）。Firefox 不支持 H.264 WebCodecs，无法使用实时模式。

## 环境要求

- Python 3.10+
- `adb` 已安装并在 `PATH` 中（Capture 与 Live 都需要）
- 已 `adb` 授权连接的 Android 设备
- 实时模式需要 `scrcpy-server`：优先使用 `SCRCPY_SERVER_PATH` 环境变量指定，否则在 `PATH` 中查找 `scrcpy`（同目录的 `scrcpy-server`），都没有则自动下载官方 scrcpy v4.0 包。

## 运行

```bash
cd web/backend
pip install -r requirements.txt

# 方式一：直接运行（内置 uvicorn）
python main.py

# 方式二：用 uvicorn 启动
uvicorn main:app --host 0.0.0.0 --port 8000
```

启动后浏览器打开 <http://localhost:8000>，顶部标签切换「检视 / 实时」。

### 配置 adb 路径

```bash
set ADB_PATH=D:\platform-tools\adb.exe   # Windows
# 或 export ADB_PATH=/path/to/adb        # Linux/macOS
```

多设备时，顶栏下拉框选择目标设备序列号（对应 `adb -s <serial>`）。

## API 速览

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/devices` | 已连接设备序列号列表 |
| GET | `/api/device-info?serial=` | 设备属性（机型/分辨率/电量等） |
| POST | `/api/capture?serial=` | 抓取截图 + 层级，返回 `{image, width, height, nodeCount, tree}` |
| POST | `/api/import` | 上传 `screenshot` + `ui_xml`，返回同上结构 |
| POST | `/api/scrcpy/start` | 启动实时会话，body `{serial, max_size, bitrate}`，返回 `{width, height, deviceName, serial}` |
| POST | `/api/scrcpy/stop` | 停止实时会话 |
| GET | `/api/scrcpy/status` | 会话状态 `{running, serial, width, height, deviceName, error}` |
| WS | `/ws/scrcpy` | 实时通道：下行二进制 H.264 帧 + JSON（`codec`/`size`/`closed`）；上行 JSON 控制消息（`touch`/`key`/`text`/`scroll`）。单客户端，断开即停止会话 |

`tree` 为嵌套 JSON：`{ id, attrs, bounds:{left,top,right,bottom}, children:[...] }`。命中检测与搜索均在浏览器端完成。

## 项目结构

```
web/
├── backend/
│   ├── main.py      # FastAPI 应用、API 路由、WebSocket、前端托管
│   ├── adb.py       # adb 封装（screencap / uiautomator dump / 设备发现）
│   ├── scrcpy.py    # scrcpy v4.0 会话（隧道/握手/H.264 流解析/控制通道）
│   ├── uitree.py    # UI 层级 XML 解析 + hit_test
│   ├── errors.py    # 统一错误类型
│   └── requirements.txt
└── frontend/
    ├── index.html
    ├── style.css
    └── static/app.js   # 检视三栏 + 实时（WebCodecs 解码、触摸/键盘控制、UI 叠加）
```

## 已知注意事项

- 实时模式依赖的 `scrcpy-server` 由后端进程拉起；如会话异常退出，后端会在下次启动时自动清理设备端残留进程与 adb 隧道。
- 本机杀软可能间歇性干扰 `adb push` / `app_process` 启动，后端已做自动重试。
- UI 树叠加按「视频分辨率 / 设备物理分辨率」独立缩放 X/Y，竖屏/横屏旋转后请重新抓取 UI 树。
