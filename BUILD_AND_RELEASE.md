# 构建与发布（Build & Release）

本文档说明 `android-ui-viewer` 的两种产出方式：

1. **本地手动构建** —— 在你自己的电脑上编译 exe。
2. **CI 自动构建 + 发布** —— 推一个 tag，GitHub 云端自动编译并把 exe 发布到 Releases 页面。

核心原则：**源码进 git，编译产物（exe / zip）不进 git，统一通过 GitHub Releases 分发。**

---

## 0. 前置知识：GitHub 自带打包服务器

GitHub 自带 **GitHub Actions**，它提供**免费的云端构建机（runner）**，不用连接你自己的服务器：

- 推送代码 / 打 tag 时，GitHub 在它的云上临时开一台虚拟机跑你的构建脚本，跑完即释放。
- Windows 构建选 `windows-latest`（已预装 Git、PowerShell、部分 SDK；Rust 工具链用 action 装上，几秒搞定）。
- 公开仓库（本仓库即为 public）构建**完全免费、无限分钟**；私有仓库每月有免费额度。
- 整个过程你的电脑可以关机，构建在 GitHub 云端执行。

---

## 1. 本地手动构建

### 1.1 环境要求
- Rust 工具链（stable）：<https://rustup.rs>
- Windows 10/11（x64）
- 仅「Capture」功能需要 adb + 已授权连接的 Android 设备；「操作模式」需要 scrcpy（见下方说明）

### 1.2 编译
```bash
# 调试运行（带调试符号，启动快，产物在 target/debug）
cargo run

# 发布构建（优化、无调试符号，产物在 target/release）
cargo build --release
```

编译完成后，exe 位于：
```
target/release/android-ui-viewer.exe
```

### 1.3 关于 scrcpy 依赖（重要）
exe **不包含** scrcpy 的 `scrcpy-server` 与 FFmpeg 的 `avcodec-62.dll` / `avutil-60.dll`。
「操作模式」运行时按以下顺序定位它们：

1. 界面里填写的「scrcpy 目录」
2. 环境变量 `SCRCPY_SERVER_PATH`
3. `where scrcpy` 自动探测本机已安装的 scrcpy
4. **都找不到 → 首次运行时自动从官方下载 `scrcpy-win64-v4.0.zip`，只抽取所需 3 个文件，缓存到 exe 旁的 `scrcpy-bundle/` 目录（仅下载一次，之后复用）**

因此**发布的 exe 包无需附带 scrcpy**，保持轻量，用户首次用操作模式时自动补齐。

手动装 scrcpy 也可：<https://github.com/Genymobile/scrcpy/releases>（取 `scrcpy-win64-v4.0.zip` 解压，记下目录填到界面即可）。

---

## 2. CI 自动构建并发布（推荐）

仓库里已包含 `.github/workflows/release.yml`，实现「打 tag → 云端编译 → 发布到 Releases」。

### 2.1 工作流逻辑
| 步骤 | 动作 | 说明 |
|------|------|------|
| 触发 | 推送 `v*` tag（如 `v1.0.0`） | 不推 tag 不构建，避免无谓消耗 |
| 1 | `actions/checkout@v4` | 拉取源码 |
| 2 | `dtolnay/rust-toolchain@stable` | 装 Rust 稳定版（target: `x86_64-pc-windows-msvc`）|
| 3 | `actions/cache@v4` | 缓存 cargo registry / git / `target`，加速重复构建 |
| 4 | `cargo build --release --target x86_64-pc-windows-msvc` | 编译 release exe |
| 5 | `Compress-Archive` | 把 exe 打包成 `android-ui-viewer-windows-x64.zip` |
| 6 | `softprops/action-gh-release@v2` | 上传 zip 到该 tag 的 Release，并自动生成更新说明 |

权限：`permissions: contents: write` —— 让 workflow 有写 Release 的权限。

### 2.2 发布一个新版（操作步骤）
```bash
# 1. 确保 main 分支已包含所有要发布的改动，并已推送到 GitHub
git push origin main

# 2. 打 tag（版本号自定，必须以 v 开头才会触发构建）
git tag v1.0.0

# 3. 推送 tag —— 这一步会触发 GitHub Actions 自动构建 + 发布
git push origin v1.0.0
```

推送 tag 后：
- 打开仓库 **Actions** 标签页，能看到 `Release Windows EXE` 工作流正在运行（通常 3~8 分钟，取决于依赖编译）。
- 构建完成后，打开仓库 **Releases** 页面，会出现一个名为 `v1.0.0` 的 Release，里面附带 `android-ui-viewer-windows-x64.zip`。
- 用户直接下载该 zip，解压即得 exe，**无需本地装 Rust**。

### 2.3 多平台
当前只编 Windows。如需同时出 Linux / macOS，在 `release.yml` 里再加 `build-linux` / `build-macos` 两个 job（分别用 `ubuntu-latest` / `macos-latest`），最后各自 `Compress-Archive` / `tar` 上传即可。本工具 GUI 基于 glow/egui，跨平台编译基本无碍，但 Linux/macOS 的 scrcpy 自动补齐路径需另写（目前仅实现 Windows 自动下载）。

---

## 3. 为什么这样做（设计取舍）

- **exe 不进 git**：二进制跨平台不通用、体积大、每次构建都变，塞进 git 会让仓库膨胀、diff 无意义。
- **用 Releases 而非源码目录分发**：用户下载的是构建好的成品，不用自己配 Rust 环境。
- **用 CI 而非手动上传**：每次发版产物可复现、可审计；不用每人本地配环境；多平台一次出齐。
- **scrcpy 走运行时下载而非打进 exe**：让 exe 保持几 MB 轻量，scrcpy 升级时也不用重打整个发布包。若希望 Release 自带 scrcpy，可把官方 `scrcpy-win64-v4.0.zip` 也作为 Release asset 一起上传，并让 `live.rs` 优先使用 exe 相邻目录里的 scrcpy。

---

## 4. 验证清单（发布前建议）

- [ ] `cargo build --release` 本地通过（无编译错误 / 无警告）
- [ ] 离线协议测试通过：`cargo run --example control_protocol_test`（输出 `RESULT: PASS`）
- [ ] 真机冒烟测试通过：`cargo run --example smoke`（输出 `RESULT: PASS`，确认视频+控制双连接）
- [ ] 仓库已 `git push origin main` 到最新
- [ ] 版本 tag 命名规范（`vX.Y.Z`）
