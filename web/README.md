# Android UI Viewer — Web 版

基于 **FastAPI** 的网页版 Android 界面查看器，功能与 Rust/egui 桌面版保持一致
（检视 + 实时 + 录制回放），并支持与桌面版**录制文件双向互通**。

## 检视模式（Capture & Inspect）

- **一键抓取**：通过 adb 同时获取截图（`screencap`）与 UI 层级
  （优先 u2 加速，详见下文；否则回退 `uiautomator dump`）；息屏时自动唤醒。
- **本地导入**：上传截图（png/jpg）与 uiautomator XML，无需连接设备。
- **三栏布局**（与桌面版一致：左 300px / 右 560px，可折叠）：
  - 左：选中节点的全部属性（class / resource-id / text / bounds 等）。
  - 中：截图，支持缩放（滚轮/工具条）、拖拽平移，叠加控件边界高亮。
  - 右：UI 层级结构树（可折叠）。
- **双向联动高亮**：点击截图任意位置自动选中**最内层**控件并定位到层级树；
  点击/悬停树节点，截图上对应控件同步高亮（并淡显祖先链）。
- **搜索过滤**：按任意属性关键字过滤层级树。
- **右键复制**（对齐桌面版）：右键树节点 → 复制 resource-id / text /
  content-desc / class / bounds / 全部属性；右键属性行 → 复制值 / 键值。
- **保存 / 导入 dump**：截图走 data: URL 保存，XML 经一次性令牌端点下载。

## 实时模式（Live / scrcpy）

浏览器无法直接解码裸 H.264，本模式利用 **Chrome/Edge 的 WebCodecs** 在浏览器端软解，
实现与桌面版一致的实时镜像与控制。**实时模式不含 UI 元素查看**（与桌面版一致，
元素检视请用检视模式）。

- **实时画面**：后端启动 scrcpy v4.0 server，H.264 Annex-B 流经 WebSocket 转发，
  前端 `VideoDecoder` 解码渲染到 canvas。
- **画质预设**：清晰（原始分辨率）/ 流畅（1024, ~4Mbps）/ 极速（720, ~2Mbps）。
- **实时操作**（走 scrcpy 控制通道，非 adb 模拟，与桌面版操作语义对齐）：
  - **左键**按下/拖动/松开 = 触摸（DOWN/MOVE/UP，拖动实时转发，单击即点按）
  - **右键** = 第二根手指（双指捏合/拖动）；右键单击（无拖动）= 返回键（BACK）
  - **滚轮** = 滚动（与桌面版同系数 0.04）
  - **屏幕外控件**：右侧竖排电源 / 音量+ / 音量−；画面下方导航栏返回 / 主页 / 最近任务
  - **键盘**（「键盘」开关开启后）：可打印字符 → 文本注入；方向键/Esc/Tab/Enter/
    Backspace/F1~F12 → keycode；Ctrl/Cmd 组合 → keycode+meta 直发；支持粘贴
  - 文本输入框回车 = 注入文本；「当前应用」按钮读取前台包名/Activity
- **连接徽章**：顶栏显示「已连接/未连接」（仅实时标签可见，与桌面版一致）。
- **断开/刷新页面即释放设备会话**；会话异常中断时后端自动重连恢复。

> 浏览器要求：Chrome / Edge（需支持 WebCodecs + H.264）。Firefox 不支持 H.264
> WebCodecs，无法使用实时模式。

## 录制 / 回放（与桌面版 record.rs 对齐）

- **录制**：实时模式下的触摸/滑动/滚轮、键盘、文本注入、屏幕外按键均记录为步骤；
  顶部「● 录制 / ■ 停止」控制。
  - 坐标存**分数值 0..1**（分辨率无关，与 Rust 同口径）。
  - 录制期间后台每 3s 静默刷新 UI 层级（对齐桌面版 `refresh_hierarchy_quiet`），
    点击步骤自动解析 **UiSelector**（resource-id / text / content-desc / class）。
  - 文本步骤复用上一次点击的目标（对齐桌面版 `last_tap_selector`）。
  - 每步注解前台 app / activity（1.5s 缓存，避免每步一次 dumpsys）。
  - 步骤列表显示中文描述：`点击 [id=…,text=…]`、`滑动 A→B`、`输入文本 "…"`、`按键 …`。
- **回放**：「加载…」或直接「▶ 回放」（再点一次中断）。
  - **selector 优先解析**：重抓 UI 层级等待目标界面出现（tap/text 重试 12 次、
    swipe 端点 6 次、400ms 间隔），找不到时回退分数坐标×物理分辨率，并把该步**标红**。
  - 有实时会话时走 WebSocket 控制通道（低延迟）；否则走 `adb shell input`
    （长按 = 600ms 静止 swipe；scroll 转 swipe），与桌面版同路径。
  - 按录制节奏回放（间隔÷速度，单段上限 30s，步后固定 600ms）；支持速度倍率与多轮循环。
- **文件互通**（字段名两端完全一致）：
  - 「保存」→ web JSON；「存 YAML」→ **Rust 桌面版可直接加载的 YAML**。
  - 「加载…」接受 `.json` / `.yml` / `.yaml`——**桌面版保存的 YAML 可直接在 web 回放**。
  - web 独有的 scroll 步骤在桌面版回放时被忽略（serde 忽略未知字段）。
  - 注意：早期 web 版的像素坐标 JSON 与新分数坐标格式**不兼容**，需重录。

## u2 加速（真机层级抓取的推荐方式）

顶部 u2 圆点展开配置面板。u2（`uiautomator2` on-device server）把层级抓取从
秒级压到毫秒级。**真机使用建议始终启动 u2**：部分场景（H5 容器前台、息屏）下
`uiautomator dump` 会被系统 SIGKILL（退出码 137），此时 u2 是唯一可靠路径；
u2 未启动时自动回退 `uiautomator dump`。首次启动偶发超时，重试一次即可。

## 环境要求

- Python 3.10+
- `adb` 已安装并在 `PATH` 中（Capture 与 Live 都需要）
- 已 `adb` 授权连接的 Android 设备
- 实时模式需要 `scrcpy-server`：优先使用 `SCRCPY_SERVER_PATH` 环境变量指定，
  否则在 `PATH` 中查找 `scrcpy`（同目录的 `scrcpy-server`），都没有则自动下载官方
  scrcpy v4.0 包。u2 需要 `~/.u2/u2_core.jar`（openatx uia2 jar v0.4.0，约 3.5MB）。

## 运行

```bash
cd web/backend
pip install -r requirements.txt   # fastapi / uvicorn / python-multipart / PyYAML

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

## API 速览（41 条路由，节选）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/devices` | 已连接设备序列号列表 |
| GET | `/api/device-info-full?serial=` | 设备属性（机型/分辨率/电量/存储等） |
| POST | `/api/capture?serial=` | 抓取截图 + 层级，返回 `{image, width, height, nodeCount, tree, raw_xml}` |
| GET | `/api/dump-ui?serial=` | 轻量层级抓取（不截屏），回放 selector 解析用 |
| GET | `/api/screen-size?serial=` | 物理分辨率（`wm size`，1080×1920 兜底） |
| GET | `/api/current-app?serial=` | 前台应用 `{pkg, activity}` |
| POST | `/api/import` | 上传 `screenshot` + `ui_xml` |
| POST | `/api/input/tap` `/api/input/swipe` `/api/input/text`、`/api/input-key` | adb 注入（回放非实时路径） |
| GET | `/api/apps?filter=` / `/api/app-props` | 应用列表（ third/system/all/running）/ 应用详情 |
| POST | `/api/app/start|stop|clear|uninstall|install|settings` | 应用操作 |
| GET | `/api/system-settings` / `POST /api/settings-action` | 系统设置深链 |
| POST | `/api/save-xml` → `GET /api/download-xml/{token}` | XML 一次性下载 |
| POST | `/api/load-recording` | 加载录制（自动识别 JSON / Rust YAML），返回 steps |
| POST | `/api/save-recording-yaml` | JSON steps → Rust 兼容 YAML 文本 |
| POST | `/api/u2/start|stop`、`GET /api/u2/status|config` | u2 on-device server 管理 |
| POST | `/api/scrcpy/start` / `stop`、`GET /api/scrcpy/status` | 实时会话 |
| WS | `/ws/scrcpy` | 实时通道：下行 H.264 帧 + JSON；上行控制消息（`touch`/`key`/`text`/`scroll`）。单客户端 |

`tree` 为嵌套 JSON：`{ id, attrs, bounds:{left,top,right,bottom}, children:[...] }`。
命中检测与搜索均在浏览器端完成。

## 测试

```bash
# 无头 E2E（puppeteer-core + 本机 Edge，需服务已启动）
node e2e_smoke.js     # 冒烟：标签/控件存在、6 类步骤回放、右键菜单、实时无 UI 树
node e2e_inspect.js   # 检视全链路：导入夹具→树/属性/叠加/搜索/缩放/保存→右键复制
node e2e_device.js    # 真机：capture→selector 解析→HOME 回放(adb)→live→WS 注入（无设备自动 SKIP）
node e2e_record.js    # 真机录制→回放全链路：录制点击→分数坐标+selector+注解→WS 回放

# 离线夹具（470×1024 合成屏 + uiautomator XML）
node tools/make_fixture.py   # 生成 fixture.png / fixture.xml 供 e2e_inspect / /api/import 使用
node shot.js                 # 布局截图 shot.png
```

## 项目结构

```
web/
├── backend/
│   ├── main.py      # FastAPI 应用、API 路由、WebSocket、前端托管
│   ├── adb.py       # adb 封装（screencap / dump / 设备信息 / input 注入 / wm size）
│   ├── scrcpy.py    # scrcpy v4.0 会话（隧道/握手/H.264 流解析/控制通道）
│   ├── u2.py        # u2 on-device server（jar 推送/启动/层级抓取，回退 uiautomator）
│   ├── uitree.py    # UI 层级 XML 解析 + hit_test
│   ├── errors.py    # 统一错误类型
│   └── requirements.txt
├── frontend/
│   ├── index.html
│   └── static/
│       ├── style.css
│       └── app.js   # 检视三栏 + 实时（WebCodecs）+ 录制回放 + 右键复制
├── tools/make_fixture.py  # 离线测试夹具生成
├── e2e_smoke.js / e2e_inspect.js / e2e_device.js / e2e_record.js
└── shot.js          # 布局截图脚本
```

## 已知注意事项

- 静态资源带 `?v=N` 缓存号，后端亦发 no-cache 头；改动 JS/CSS 后若不生效请硬刷新并递增版本号。
- 本机杀软可能间歇性干扰 `adb push` / `app_process` 启动，后端已做自动重试。
- 实时会话由后端进程拉起 scrcpy-server；异常退出后下次启动自动清理设备端残留进程与 adb 隧道。
- 息屏抓取会先 `input keyevent 224` 唤醒（只点亮，不解锁）。
- 回放坐标为分数值：录制与回放时设备的旋转/分辨率变化由 selector 解析兜底，
  纯坐标回退在旋转后会偏移（与桌面版一致）。
