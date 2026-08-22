//! scrcpy live session: runs the scrcpy server on the device, reads the raw
//! H.264 stream and decodes it in-process (via FFmpeg DLLs shipped with
//! scrcpy) before handing RGBA frames to the UI thread.
//!
//! Protocol (reverse tunnel, video only):
//!  1. 64-byte device name
//!  2. 4-byte big-endian codec id      ("h264" = 0x68323634)
//!  3. 12-byte session header          (MSB of byte 0 set, then width/height BE32)
//!  4. repeating 12-byte frame header followed by <len> bytes of H.264
//!     (Annex-B). Config packets (SPS/PPS) are merged with the next media packet.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// Prevent spawned helper processes (adb, scrcpy, powershell) from popping up
/// their own console window. The app itself runs as a GUI subsystem, so child
/// console programs would otherwise get a separate, "stuck-looking" cmd window
/// on Windows. No-op on non-Windows targets.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hide_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use crate::scrcpy::{H264Decoder, RgbaFrame};
use crate::log::{error, info};

const DEVICE_NAME_LEN: usize = 64;
const HEADER_LEN: usize = 12;
const CODEC_ID_H264: u32 = 0x6832_3634; // "h264"

const FLAG_CONFIG: u64 = 1 << 62;

// Local abstract socket name prefix; the hex suffix must match the scid we
// pass to the server so STOPS/SESSION are consistent.
fn socket_name(scid: u32) -> String {
    format!("scrcpy_{scid:08x}")
}

pub enum LiveEvent {
    Connected {
        width: u32,
        height: u32,
        device_name: String,
        serial: String,
        /// Real-time control channel. `None` means the control connection
        /// failed and the UI should fall back to `adb shell input`.
        control: Option<LiveControl>,
    },
    Frame(RgbaFrame),
    Status(String),
    Error(String),
    Stopped,
}

// ---- scrcpy control protocol (client -> device) ----

const MSG_INJECT_KEYCODE: u8 = 0;
const MSG_INJECT_TEXT: u8 = 1;
const MSG_INJECT_TOUCH_EVENT: u8 = 2;
const MSG_INJECT_SCROLL_EVENT: u8 = 3;

const ACTION_DOWN: u8 = 0;
const ACTION_UP: u8 = 1;
const ACTION_MOVE: u8 = 2;

const PRESSURE_MAX: u16 = 0xFFFF;

/// Real-time control channel over the scrcpy control socket. All methods are
/// fire-and-forget; the underlying write is best-effort.
#[derive(Clone)]
pub struct LiveControl {
    writer: Arc<Mutex<TcpStream>>,
    /// Latest decoded video frame size, used as the coordinate space for
    /// injected touch events (the server scales these to the real device).
    size: Arc<Mutex<(u32, u32)>>,
    pointer_id: u64,
}

impl LiveControl {
    fn send(&self, buf: &[u8]) {
        if let Ok(mut s) = self.writer.lock() {
            let _ = s.write_all(buf);
        }
    }

    /// Update the coordinate space used for touch events (called per frame).
    pub fn set_size(&self, w: u32, h: u32) {
        if let Ok(mut s) = self.size.lock() {
            *s = (w, h);
        }
    }

    /// Build a control handle around an already-established control socket.
    pub fn from_stream(stream: TcpStream, pointer_id: u64) -> Self {
        Self {
            writer: Arc::new(Mutex::new(stream)),
            size: Arc::new(Mutex::new((0, 0))),
            pointer_id,
        }
    }

    fn touch(&self, pid: u64, action: u8, x: i32, y: i32, pressure: u16) {
        let (w, h) = self.size.lock().map(|s| *s).unwrap_or((0, 0));
        if w == 0 || h == 0 {
            return;
        }
        let mut buf = [0u8; 32];
        buf[0] = MSG_INJECT_TOUCH_EVENT;
        buf[1] = action;
        buf[2..10].copy_from_slice(&pid.to_be_bytes());
        // scrcpy v4.0 encodes touch position as **signed 32-bit integers** in
        // the video frame's pixel space (NOT float). The server scales x/y by
        // (real device size / screenW,screenH). Sending float bits here made
        // every tap land at a bogus coordinate → no response on the device.
        buf[10..14].copy_from_slice(&x.to_be_bytes());
        buf[14..18].copy_from_slice(&y.to_be_bytes());
        buf[18..20].copy_from_slice(&(w as u16).to_be_bytes());
        buf[20..22].copy_from_slice(&(h as u16).to_be_bytes());
        buf[22..24].copy_from_slice(&pressure.to_be_bytes());
        // actionButton (u32) and buttons (u32) stay 0 — a bare finger touch.
        self.send(&buf);
    }

    /// Primary (pointer id 0) touch events. Coordinates are integer pixels in
    /// the decoded video frame's space.
    pub fn touch_down(&self, x: i32, y: i32) {
        self.touch(self.pointer_id, ACTION_DOWN, x, y, PRESSURE_MAX);
    }

    pub fn touch_move(&self, x: i32, y: i32) {
        self.touch(self.pointer_id, ACTION_MOVE, x, y, PRESSURE_MAX);
    }

    pub fn touch_up(&self, x: i32, y: i32) {
        self.touch(self.pointer_id, ACTION_UP, x, y, 0);
    }

    /// Multi-touch: a second finger uses a distinct pointer id.
    pub fn touch_down_pid(&self, pid: u64, x: i32, y: i32) {
        self.touch(pid, ACTION_DOWN, x, y, PRESSURE_MAX);
    }

    pub fn touch_move_pid(&self, pid: u64, x: i32, y: i32) {
        self.touch(pid, ACTION_MOVE, x, y, PRESSURE_MAX);
    }

    pub fn touch_up_pid(&self, pid: u64, x: i32, y: i32) {
        self.touch(pid, ACTION_UP, x, y, 0);
    }

    fn key(&self, action: u8, code: u32, meta: u32) {
        let mut buf = [0u8; 14];
        buf[0] = MSG_INJECT_KEYCODE;
        buf[1] = action;
        buf[2..6].copy_from_slice(&code.to_be_bytes());
        // buf[6..10] repeat (u32) stays 0.
        buf[10..14].copy_from_slice(&meta.to_be_bytes());
        self.send(&buf);
    }

    pub fn key_down(&self, code: u32) {
        self.key(ACTION_DOWN, code, 0);
    }

    pub fn key_up(&self, code: u32) {
        self.key(ACTION_UP, code, 0);
    }

    /// Key with Android meta-state flags (shift/alt/ctrl/meta) for shortcuts.
    pub fn key_down_meta(&self, code: u32, meta: u32) {
        self.key(ACTION_DOWN, code, meta);
    }

    pub fn key_up_meta(&self, code: u32, meta: u32) {
        self.key(ACTION_UP, code, meta);
    }

    /// Press + release a keycode (a full key tap).
    pub fn press_key(&self, code: u32) {
        self.key_down(code);
        self.key_up(code);
    }

    /// Inject UTF-8 text (truncated to scrcpy's 300-byte limit).
    pub fn text(&self, text: &str) {
        let bytes = text.as_bytes();
        let b: &[u8] = if bytes.len() > 300 { &bytes[..300] } else { bytes };
        let mut buf = Vec::with_capacity(5 + b.len());
        buf.push(MSG_INJECT_TEXT);
        buf.extend_from_slice(&(b.len() as u32).to_be_bytes());
        buf.extend_from_slice(b);
        self.send(&buf);
    }

    /// Mouse-wheel scroll at a position (video-frame pixel coordinates).
    ///
    /// Wire format (scrcpy v4.0, 21 bytes):
    /// `type(1)=3, x(i32), y(i32), screenW(u16), screenH(u16),
    ///  hScroll(i16 fixed-point), vScroll(i16 fixed-point), buttons(u32)=0`.
    /// The server decodes each i16 fixed-point value as `v/32768*16` and
    /// dispatches an `AMOTION_EVENT_AXIS_HSCROLL/VSCROLL` of that amount, so
    /// we encode `units*2048` (since `units = encoded/2048`).
    pub fn scroll(&self, x: i32, y: i32, h_units: f32, v_units: f32) {
        let (w, h) = self.size.lock().map(|s| *s).unwrap_or((0, 0));
        if w == 0 || h == 0 {
            return;
        }
        let to_i16 = |v: f32| -> i16 {
            let e = (v * 2048.0).round() as i32;
            e.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        };
        let mut buf = [0u8; 21];
        buf[0] = MSG_INJECT_SCROLL_EVENT;
        buf[1..5].copy_from_slice(&x.to_be_bytes());
        buf[5..9].copy_from_slice(&y.to_be_bytes());
        buf[9..11].copy_from_slice(&(w as u16).to_be_bytes());
        buf[11..13].copy_from_slice(&(h as u16).to_be_bytes());
        buf[13..15].copy_from_slice(&to_i16(h_units).to_be_bytes());
        buf[15..17].copy_from_slice(&to_i16(v_units).to_be_bytes());
        // buttons (u32) stay 0.
        self.send(&buf);
    }
}

/// Drain device -> client messages on the control socket so the TCP receive
/// buffer never backs up and stalls the sender. We don't use clipboard
/// autosync, so these are simply discarded.
fn spawn_drainer(mut stream: TcpStream, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let mut buf = [0u8; 4096];
        while !stop.load(Ordering::Relaxed) {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    continue;
                }
                Err(_) => break,
            }
        }
    });
}

/// Auto-download the official scrcpy v4.0 Windows bundle and extract only the
/// files this tool needs (`scrcpy-server` + `avcodec`/`avutil` DLLs) into a
/// cache directory. Reuses the cache on subsequent runs so the download only
/// happens once.
fn auto_fetch_scrcpy_bundle() -> Result<PathBuf> {
    const URL: &str =
        "https://github.com/Genymobile/scrcpy/releases/download/v4.0/scrcpy-win64-v4.0.zip";
    const NEEDED: [&str; 3] = ["scrcpy-server", "avcodec-62.dll", "avutil-60.dll"];

    // Cache next to the executable; fall back to LOCALAPPDATA if that is not
    // writable (e.g. installed under Program Files).
    let exe = std::env::current_exe().context("无法定位当前可执行文件")?;
    let base = exe
        .parent()
        .ok_or_else(|| anyhow!("无法获取可执行文件所在目录"))?;
    let cache = if std::fs::create_dir_all(base.join("scrcpy-bundle")).is_ok() {
        base.join("scrcpy-bundle")
    } else {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\Windows\\Temp".into());
        PathBuf::from(local)
            .join("android-ui-viewer")
            .join("scrcpy")
    };
    std::fs::create_dir_all(&cache).ok();

    if NEEDED.iter().all(|n| cache.join(n).is_file()) {
        return Ok(cache);
    }

    let zip = cache.join("scrcpy-win64-v4.0.zip");
    let script = SCRCPY_FETCH_PS1
        .replace("{URL}", URL)
        .replace("{ZIP}", &zip.to_string_lossy())
        .replace("{DIR}", &cache.to_string_lossy());
    let ps1 = cache.join("fetch_scrcpy.ps1");
    std::fs::write(&ps1, script).context("写入 scrcpy 下载脚本失败")?;

    let mut ps = Command::new("powershell");
    hide_console(&mut ps);
    let status = ps
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &ps1.to_string_lossy(),
        ])
        .status()
        .context("启动 PowerShell 下载 scrcpy 失败")?;
    let _ = std::fs::remove_file(&ps1);

    if !status.success() {
        bail!("下载/解压 scrcpy 包失败（PowerShell 退出码 {status}）");
    }
    if !NEEDED.iter().all(|n| cache.join(n).is_file()) {
        bail!("scrcpy 包解压后缺少必要文件（可能被网络或杀软拦截）");
    }
    Ok(cache)
}

/// PowerShell script: download the zip (if missing) and extract only the
/// entries this tool needs into `{DIR}`.
const SCRCPY_FETCH_PS1: &str = r#"
$ErrorActionPreference = 'Stop'
$url = '{URL}'
$zip = '{ZIP}'
$dir = '{DIR}'
if (-not (Test-Path $zip)) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zf = [System.IO.Compression.ZipFile]::OpenRead($zip)
$names = @('scrcpy-server', 'avcodec-62.dll', 'avutil-60.dll')
foreach ($n in $names) {
    $e = $zf.Entries | Where-Object { $_.Name -eq $n } | Select-Object -First 1
    if ($e) {
        $t = Join-Path $dir $n
        if (-not (Test-Path $t)) {
            [System.IO.Compression.ZipFile]::ExtractToFile($e, $t, $true)
        }
    }
}
$zf.Dispose()
"#;

/// Detect the scrcpy bundle directory (holds scrcpy-server and the FFmpeg DLLs
/// used for decoding). Falls back to an automatic download of the official
/// v4.0 bundle when no local install is found.
pub fn detect_scrcpy_dir(user_hint: &str) -> Result<PathBuf> {
    if !user_hint.trim().is_empty() {
        let p = PathBuf::from(user_hint.trim());
        if p.join("scrcpy-server").is_file() {
            return Ok(p);
        }
        bail!("指定的 scrcpy 目录中未找到 scrcpy-server: {}", p.display());
    }

    if let Ok(v) = std::env::var("SCRCPY_SERVER_PATH") {
        let p = PathBuf::from(v);
        if p.is_file() {
            if let Some(dir) = p.parent() {
                if dir.join("avcodec-62.dll").is_file() {
                    return Ok(dir.to_path_buf());
                }
            }
        }
    }

    let mut where_cmd = Command::new("where");
    hide_console(&mut where_cmd);
    if let Ok(out) = where_cmd.arg("scrcpy").output() {
        if out.status.success() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                for line in text.lines() {
                    let s = line.trim();
                    if s.is_empty() {
                        continue;
                    }
                    let p = PathBuf::from(s);
                    if let Some(dir) = p.parent() {
                        if dir.join("scrcpy-server").is_file() {
                            return Ok(dir.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    // No local scrcpy install: try to auto-fetch the official v4.0 bundle.
    match auto_fetch_scrcpy_bundle() {
        Ok(dir) => Ok(dir),
        Err(e) => bail!(
            "未找到 scrcpy 目录，且自动下载失败：{e}\n\
             请手动下载 https://github.com/Genymobile/scrcpy/releases 的 scrcpy-win64-v4.0.zip 并解压，\n\
             然后在界面填写 scrcpy 目录，例如 D:\\scrcpy-win64-v4.0"
        ),
    }
}

fn detect_serial(adb: &str, hint: &str) -> Result<Option<String>> {
    if !hint.trim().is_empty() {
        return Ok(Some(hint.trim().to_string()));
    }
    let out = Command::new(adb)
        .arg("devices")
        .output()
        .map_err(|e| anyhow!("无法运行 adb: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut devs = Vec::new();
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let serial = it.next().unwrap_or("");
        let state = it.next().unwrap_or("");
        if !serial.is_empty() && state == "device" {
            devs.push(serial.to_string());
        }
    }
    match devs.len() {
        0 => bail!("未检测到已连接的设备（adb devices 无已授权设备）"),
        1 => Ok(Some(devs[0].clone())),
        n => bail!("检测到 {n} 台设备，请在上方输入目标设备序列号"),
    }
}

fn adb_run(adb: &str, serial: &str, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new(adb);
    hide_console(&mut cmd);
    if !serial.is_empty() {
        cmd.arg("-s").arg(serial);
    }
    cmd.args(args);
    cmd.output().map_err(|e| anyhow!("adb 命令失败 ({args:?}): {e}"))
}

fn adb_run_ok(adb: &str, serial: &str, args: &[&str]) -> Result<()> {
    let out = adb_run(adb, serial, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("adb {args:?} 失败: {}", stderr.trim());
    }
    Ok(())
}

/// Start a live session thread. Frames/status are delivered on `tx`.
pub fn start(
    adb: String,
    serial_hint: String,
    scrcpy_dir_hint: String,
    max_video_size: u32,
    bitrate: u32,
    stop: Arc<AtomicBool>,
    tx: Sender<LiveEvent>,
) {
    thread::spawn(move || {
        info!("live 工作线程启动");
        let result = run(
            &adb,
            &serial_hint,
            &scrcpy_dir_hint,
            max_video_size,
            bitrate,
            &stop,
            &tx,
        );
        match result {
            Ok(()) => {
                info!("live 会话正常结束");
                let _ = tx.send(LiveEvent::Stopped);
            }
            Err(e) => {
                error!("live 会话失败: {e}");
                let _ = tx.send(LiveEvent::Error(format!("{e}")));
            }
        }
    });
}

fn pkt_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn pkt_u64(buf: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&buf[off..off + 8]);
    u64::from_be_bytes(v)
}

fn cleanup_tunnel(adb: &str, serial: &str, scid: u32, port: u16, forward: bool) {
    let name = socket_name(scid);
    if forward {
        let _ = adb_run_ok(adb, serial, &["forward", "--remove", &format!("tcp:{port}")]);
    } else {
        let _ = adb_run_ok(adb, serial, &["reverse", "--remove", &name]);
    }
}

/// Kill the adb client that hosts the server process.
fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn run(
    adb: &str,
    serial_hint: &str,
    scrcpy_dir_hint: &str,
    max_video_size: u32,
    bitrate: u32,
    stop: &Arc<AtomicBool>,
    tx: &Sender<LiveEvent>,
) -> Result<()> {
    let _ = tx.send(LiveEvent::Status("正在启动 scrcpy 会话…".to_string()));

    let serial = detect_serial(adb, serial_hint)?;
    let serial_ref = serial.as_deref().unwrap_or("");
    info!("检测到设备 serial={:?}", serial_ref);

    let scrcpy_dir = detect_scrcpy_dir(scrcpy_dir_hint)?;
    info!("scrcpy 目录: {}", scrcpy_dir.display());
    let server_path = scrcpy_dir.join("scrcpy-server");
    if !server_path.is_file() {
        bail!("未找到 scrcpy-server: {}", server_path.display());
    }
    let dll_dir = scrcpy_dir;

    // Local listening socket for the tunnel.
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("绑定本地端口失败")?;
    let local_port = listener.local_addr()?.port();
    let _ = listener.set_nonblocking(true);

    let scid = (Instant::now().elapsed().as_nanos() as u32) ^ 0x55aa_55aa;
    let sock_name = socket_name(scid);

    // Prefer adb reverse; fall back to adb forward.
    let mut tunnel_forward = false;
    let reverse_ok = adb_run_ok(
        adb,
        serial_ref,
        &[
            "reverse",
            &format!("localabstract:{sock_name}"),
            &format!("tcp:{local_port}"),
        ],
    )
    .is_ok();
    if reverse_ok {
        let _ = tx.send(LiveEvent::Status("已创建 adb reverse 隧道".to_string()));
    } else {
        if adb_run_ok(
            adb,
            serial_ref,
            &[
                "forward",
                &format!("tcp:{local_port}"),
                &format!("localabstract:{sock_name}"),
            ],
        )
        .is_err()
        {
            bail!("无法建立 adb reverse/forward 隧道（设备是否已授权 adb？）");
        }
        tunnel_forward = true;
        let _ = tx.send(LiveEvent::Status("已创建 adb forward 隧道".to_string()));
    }

    // Push the server jar onto the device.
    if let Err(e) = adb_run_ok(
        adb,
        serial_ref,
        &[
            "push",
            server_path.to_str().unwrap_or(""),
            "/data/local/tmp/scrcpy-server.jar",
        ],
    ) {
        cleanup_tunnel(adb, serial_ref, scid, local_port, tunnel_forward);
        return Err(e);
    }
    let _ = tx.send(LiveEvent::Status("已推送 scrcpy-server".to_string()));

    // Launch the scrcpy server on the device.
    let mut cmd_args: Vec<String> = vec![
        "shell".into(),
        "CLASSPATH=/data/local/tmp/scrcpy-server.jar".into(),
        "app_process".into(),
        "/".into(),
        "com.genymobile.scrcpy.Server".into(),
        "4.0".into(),
        format!("scid={scid:08x}"),
        "log_level=info".into(),
        "video=true".into(),
        "audio=false".into(),
        "video_codec=h264".into(),
        "control=true".into(),
        "cleanup=true".into(),
    ];
    if tunnel_forward {
        cmd_args.push("tunnel_forward=true".into());
    }
    if max_video_size > 0 {
        cmd_args.push(format!("max_size={max_video_size}"));
    }
    if bitrate > 0 {
        cmd_args.push(format!("video_bitrate={bitrate}"));
    }

    let mut child = Command::new(adb);
    hide_console(&mut child);
    let mut child: Child = child
        .args(&cmd_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("启动 scrcpy server 失败: {e}"))?;

    // The scrcpy server opens TWO connections to the same abstract socket:
    // first the video stream, then the control channel. So we take two
    // connections from the single listener (reverse) or make two localhost
    // connects (forward). The video connection carries a 1-byte dummy in
    // forward mode so we can detect a bad tunnel.
    let mut video_sock: Option<TcpStream> = None;
    let mut control_sock: Option<TcpStream> = None;

    if tunnel_forward {
        while !stop.load(Ordering::Relaxed) {
            match TcpStream::connect(("127.0.0.1", local_port)) {
                Ok(s) => {
                    video_sock = Some(s);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
        // Second connect reaches the control connection the server accepts next.
        while video_sock.is_some() && !stop.load(Ordering::Relaxed) {
            match TcpStream::connect(("127.0.0.1", local_port)) {
                Ok(s) => {
                    control_sock = Some(s);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
    } else {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((sock, _)) => {
                    video_sock = Some(sock);
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(e) => bail!("accept 失败: {e}"),
            }
        }
        while video_sock.is_some() && !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((sock, _)) => {
                    control_sock = Some(sock);
                    break;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(e) => bail!("accept(control) 失败: {e}"),
            }
        }
    }

    let Some(sock) = video_sock else {
        kill_child(&mut child);
        cleanup_tunnel(adb, serial_ref, scid, local_port, tunnel_forward);
        stop.store(true, Ordering::Relaxed);
        bail!("已取消");
    };
    let _ = sock.set_nodelay(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
    let mut stream = sock;
    info!(
        "视频连接已建立 (tunnel_forward={}, local_port={})",
        tunnel_forward, local_port
    );

    if tunnel_forward {
        // Server writes one dummy byte so the client can detect a bad tunnel.
        let mut dummy = [0u8; 1];
        read_exact_timeout(&mut stream, &mut dummy, stop)?;
    }

    // Device name + codec id.
    let mut devname = [0u8; DEVICE_NAME_LEN];
    read_exact_timeout(&mut stream, &mut devname, stop)?;
    let device_name = String::from_utf8_lossy(&devname)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let mut codec = [0u8; 4];
    read_exact_timeout(&mut stream, &mut codec, stop)?;
    let codec_id = pkt_u32(&codec, 0);
    if codec_id == 0 {
        bail!("设备端无法启动视频流（可能被其他 scrcpy 会话占用）");
    }
    if codec_id != CODEC_ID_H264 {
        // 0x0? Means error; raw_codec_id==1 means config error on the device.
        bail!("设备端视频编码不是 h264 (codec=0x{codec_id:08x})");
    }

    // Session header (initial dimensions).
    let mut hdr = [0u8; HEADER_LEN];
    read_exact_timeout(&mut stream, &mut hdr, stop)?;
    if hdr[0] & 0x80 == 0 {
        bail!("协议错误：未收到会话头");
    }
    let mut vw = pkt_u32(&hdr, 4);
    let mut vh = pkt_u32(&hdr, 8);

    // Decoder (loads avcodec from the scrcpy bundle).
    let mut dec = match H264Decoder::try_new(&dll_dir) {
        Ok(d) => d,
        Err(e) => {
            error!("H264 解码器初始化失败: {e}");
            kill_child(&mut child);
            cleanup_tunnel(adb, serial_ref, scid, local_port, tunnel_forward);
            stop.store(true, Ordering::Relaxed);
            return Err(e);
        }
    };
    info!("H264 解码器初始化成功");

    // Set up the control channel (real-time touch/keys over scrcpy). If it
    // failed to connect we keep running video-only and fall back to adb input.
    let mut control: Option<LiveControl> = None;
    if let Some(csk) = control_sock {
        let _ = csk.set_nodelay(true);
        if let Ok(reader) = csk.try_clone() {
            spawn_drainer(reader, stop.clone());
        }
        let ctrl = LiveControl::from_stream(csk, 0);
        ctrl.set_size(vw, vh);
        control = Some(ctrl);
        let _ = tx.send(LiveEvent::Status("控制通道已建立（scrcpy 实时操作）".to_string()));
    } else {
        let _ = tx.send(LiveEvent::Status("未建立控制通道，回退到 adb 输入".to_string()));
    }

    let _ = tx.send(LiveEvent::Connected {
        width: vw,
        height: vh,
        device_name,
        serial: serial_ref.to_string(),
        control: control.clone(),
    });
    let _ = tx.send(LiveEvent::Status(format!("已连接 {vw}x{vh}")));

    // Frame loop.
    let mut pending_config: Option<Vec<u8>> = None;
    let mut last_frame_at = Instant::now() - Duration::from_secs(1);

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if read_exact_timeout(&mut stream, &mut hdr, stop).is_err() {
            // Timeout / disconnect / user stop.
            break;
        }

        if hdr[0] & 0x80 != 0 {
            // Size / orientation change.
            let new_w = pkt_u32(&hdr, 4);
            let new_h = pkt_u32(&hdr, 8);
            if new_w != vw || new_h != vh {
                vw = new_w;
                vh = new_h;
                dec.flush();
                info!("视频方向/尺寸变化 → {}x{}", vw, vh);
                let _ = tx.send(LiveEvent::Status(format!("方向/尺寸变化 → {vw}x{vh}")));
            }
            continue;
        }

        let pts_flags = pkt_u64(&hdr, 0);
        let len = pkt_u32(&hdr, 8) as usize;
        if len == 0 || len > (1 << 23) {
            continue;
        }
        let mut data = vec![0u8; len];
        if read_exact_timeout(&mut stream, &mut data, stop).is_err() {
            break;
        }

        // Config packets (SPS/PPS) must be merged into the following packet.
        let payload: Vec<u8> = if pts_flags & FLAG_CONFIG != 0 {
            pending_config = Some(data);
            continue;
        } else if let Some(mut cfg) = pending_config.take() {
            cfg.extend_from_slice(&data);
            cfg
        } else {
            data
        };

        if let Ok(Some(frame)) = dec.decode(&payload) {
            // Keep the control channel's coordinate space aligned with the
            // decoded frame so touch events map to the right device pixels.
            if let Some(c) = &control {
                c.set_size(frame.width, frame.height);
            }
            let now = Instant::now();
            if now.duration_since(last_frame_at).as_millis() >= 16 {
                last_frame_at = now;
                let _ = tx.send(LiveEvent::Frame(frame));
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    kill_child(&mut child);
    cleanup_tunnel(adb, serial_ref, scid, local_port, tunnel_forward);
    let _ = tx.send(LiveEvent::Status("会话已结束".to_string()));
    Ok(())
}

/// Read exactly `len` bytes, subject to the socket read timeout and stop flag.
fn read_exact_timeout(
    stream: &mut TcpStream,
    buf: &mut [u8],
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        if stop.load(Ordering::Relaxed) {
            bail!("已停止");
        }
        match stream.read(&mut buf[filled..]) {
            Ok(0) => bail!("设备连接已断开"),
            Ok(n) => filled += n,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(e) => bail!("读取视频流失败: {e}"),
        }
    }
    Ok(())
}