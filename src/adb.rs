use std::os::windows::process::CommandExt as _;
use std::process::Command;

use anyhow::{anyhow, Result};

/// Stop spawned adb helpers from opening their own console window (the app is
/// a GUI subsystem on Windows, so child console programs would otherwise pop a
/// separate, stuck-looking cmd window). No-op off Windows.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hide_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Common settings/tables the left panel offers a one-tap deep link into.
/// (Only entries that open a real, reachable screen on most devices are kept —
/// Wi-Fi hotspot / storage are often no-ops so they are omitted.)
pub const SYSTEM_SETTINGS: &[(&str, &str)] = &[
    ("Wi-Fi", "android.settings.WIFI_SETTINGS"),
    ("蓝牙", "android.settings.BLUETOOTH_SETTINGS"),
    ("声音", "android.settings.SOUND_SETTINGS"),
    ("显示", "android.settings.DISPLAY_SETTINGS"),
    ("通知", "android.settings.NOTIFICATION_SETTINGS"),
    ("应用", "android.settings.APPLICATION_SETTINGS"),
    ("电池", "android.settings.BATTERY_SAVER_SETTINGS"),
    ("辅助功能", "android.settings.ACCESSIBILITY_SETTINGS"),
];

/// A single installed app entry, as shown in the left panel list.
#[derive(Debug, Clone, Default)]
pub struct AppInfo {
    pub package: String,
    /// True if it is a third-party (user-installed) package.
    pub third_party: bool,
    /// Best-effort: whether a process for this package is currently running.
    pub running: bool,
}

/// A summary of connected-device properties (brand/model/OS/resolution…).
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    pub brand: String,
    pub model: String,
    pub android: String,
    pub sdk: String,
    pub resolution: String,
    pub density: String,
    pub battery: String,
    pub serial: String,
    /// Human storage summary, e.g. `"可用 19G / 总 48G"`.
    pub storage: String,
    /// Build/firmware identifier, e.g. the incremental build number.
    pub build: String,
}

/// A shell-backed adb command (honours `-s <serial>` when non-empty) with no
/// separate console window on Windows.
fn shell(adb: &str, serial: &str, args: &[&str]) -> Command {
    let mut c = Command::new(adb);
    hide_console(&mut c);
    if !serial.is_empty() {
        c.arg("-s").arg(serial);
    }
    c.args(args);
    c
}

/// Run a shell command and return its stdout as UTF-8 (lossy, best effort).
fn sh_stdout(adb: &str, serial: &str, args: &[&str]) -> String {
    shell(adb, serial, args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// `adb shell getprop`, parsed into a `name -> value` map (`[k]: [v]` lines).
fn read_props(adb: &str, serial: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let text = sh_stdout(adb, serial, &["shell", "getprop"]);
    for line in text.lines() {
        let line = line.trim();
        // Format:  [key]: [value]
        if let Some(body) = line.strip_prefix('[') {
            if let Some(close) = body.find(']') {
                let key = &body[..close];
                if key.is_empty() {
                    continue;
                }
                let after = body[close + 1..].trim();
                if let Some(v) = after.strip_prefix(": [") {
                    let val = v.strip_suffix(']').unwrap_or(v).trim();
                    map.insert(key.to_string(), val.to_string());
                }
            }
        }
    }
    map
}

/// Gather device properties shown in the left panel's device-info card.
pub fn device_info(adb: &str, serial: &str) -> DeviceInfo {
    let p = read_props(adb, serial);
    let dpi = p.get("ro.sf.lcd_density").cloned().unwrap_or_default();
    let build = p
        .get("ro.build.display.id")
        .or_else(|| p.get("ro.build.version.incremental"))
        .cloned()
        .unwrap_or_default();
    DeviceInfo {
        brand: p.get("ro.product.brand").cloned().unwrap_or_default(),
        model: p.get("ro.product.model").cloned().unwrap_or_default(),
        android: p.get("ro.build.version.release").cloned().unwrap_or_default(),
        sdk: p.get("ro.build.version.sdk").cloned().unwrap_or_default(),
        resolution: sh_stdout(adb, serial, &["shell", "wm", "size"])
            .lines()
            .find_map(|l| l.split_once("Physical size:").map(|(_, s)| s.trim().to_string()))
            .unwrap_or_default(),
        density: if dpi.is_empty() {
            String::new()
        } else {
            format!("{dpi} dpi")
        },
        battery: battery_pct(adb, serial),
        serial: serial.to_string(),
        storage: storage_summary(adb, serial),
        build,
    }
}

/// Best-effort battery percentage, e.g. `"78%"`; empty if unknown.
fn battery_pct(adb: &str, serial: &str) -> String {
    let out = sh_stdout(adb, serial, &["shell", "dumpsys", "battery"]);
    for line in out.lines() {
        if let Some(lvl) = line.trim_start().strip_prefix("level:") {
            if let Ok(n) = lvl.trim().parse::<i32>() {
                return format!("{n}%");
            }
        }
    }
    String::new()
}

/// Best-effort internal storage summary from `df /data` (report  sizes in GB).
fn storage_summary(adb: &str, serial: &str) -> String {
    let out = sh_stdout(adb, serial, &["shell", "df", "/data"]);
    // Line 2 (after the header) holds: fs total used available use% mount.
    let line = out.lines().nth(1).unwrap_or_default();
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 4 {
        return String::new();
    }
    let to_gb = |blocks: &str| -> String {
        blocks
            .parse::<u64>()
            .ok()
            .map(|b| format!("{}G", b / 1024 / 1024))
            .unwrap_or_default()
    };
    let total = to_gb(cols[1]);
    let avail = to_gb(cols[3]);
    if total.is_empty() {
        String::new()
    } else if avail.is_empty() {
        format!("总 {total}")
    } else {
        format!("可用 {avail} / 总 {total}")
    }
}

/// List installed packages. `filter` restricts the section: "all", "system",
/// "third", or "running". Returns one entry per package.
pub fn list_apps(adb: &str, serial: &str, filter: &str) -> Vec<AppInfo> {
    // Third-party set (user-installed).
    let third: std::collections::HashSet<String> = sh_stdout(
        adb,
        serial,
        &["shell", "pm", "list", "packages", "-3"],
    )
    .lines()
    .filter_map(|l| l.strip_prefix("package:"))
    .map(|s| s.trim().to_string())
    .collect();

    // Running set: package becomes the process name; match by exact name, else
    // by the longest package that is a prefix of a process name (best effort).
    let running: std::collections::HashSet<String> = sh_stdout(
        adb,
        serial,
        &["shell", "ps", "-A", "-o", "NAME"],
    )
    .lines()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty() && s != "NAME")
    .collect();

    // The full package list.
    let mut apps: Vec<AppInfo> = sh_stdout(adb, serial, &["shell", "pm", "list", "packages"])
        .lines()
        .filter_map(|l| l.strip_prefix("package:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|pkg| {
            let tp = third.contains(&pkg);
            let running = running.iter().any(|pn| {
                pn == &pkg || pkg.starts_with(pn) || pn.starts_with(&pkg)
            });
            AppInfo {
                package: pkg,
                third_party: tp,
                running,
            }
        })
        .collect();

    apps.sort_by(|a, b| a.package.cmp(&b.package));
    match filter {
        "system" => apps.retain(|a| !a.third_party),
        "third" => apps.retain(|a| a.third_party),
        "running" => apps.retain(|a| a.running),
        _ => {}
    }
    apps
}

/// Read a package's version metadata via `dumpsys package`.
pub fn app_properties(adb: &str, serial: &str, pkg: &str) -> String {
    let out = sh_stdout(adb, serial, &["shell", "dumpsys", "package", pkg]);
    // `dumpsys package` prints these as part of a longer line, e.g.
    //   Package [com.foo] (abc): versionName=1.2.3 versionCode=123 targetSdk=34
    // so match the `key=` token anywhere on a line and read until the next space.
    let mut seen = std::collections::BTreeMap::new();
    for line in out.lines() {
        for key in ["versionName", "versionCode", "firstInstallTime", "lastUpdateTime"] {
            let pat = format!("{key}=");
            if let Some(pos) = line.find(&pat) {
                let val = line[pos + pat.len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('}')
                    .to_string();
                if !val.is_empty() {
                    seen.entry(key).or_insert_with(|| val.clone());
                }
            }
        }
    }
    let mut props: Vec<String> = seen
        .into_iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect();
    if props.is_empty() {
        format!("未查询到 {pkg} 的信息（包未安装？）")
    } else {
        props.insert(0, format!("包名: {pkg}"));
        props.join("\n")
    }
}

/// Uninstall a package (`pm uninstall --user 0`). Returns an adb status line.
pub fn uninstall_app(adb: &str, serial: &str, pkg: &str) -> String {
    let out = shell(adb, serial, &["shell", "pm", "uninstall", "--user", "0", pkg])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if out.is_empty() || out == "Success" {
        format!("已卸载 {pkg}")
    } else {
        out
    }
}

/// Inject a single key event (`input keyevent <code>`).
pub fn input_key(adb: &str, serial: &str, code: &str) {
    let _ = shell(adb, serial, &["shell", "input", "keyevent", code]).spawn();
}

/// Set screen brightness (0..255) and return the result string.
pub fn set_brightness(adb: &str, serial: &str, value: u16) -> String {
    let v = value.to_string();
    let out = shell(adb, serial, &["shell", "settings", "put", "system", "screen_brightness", &v])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if out.is_empty() {
        format!("亮度已设为 {}%", value as u32 * 100 / 255)
    } else {
        out
    }
}

/// Toggle automatic brightness (`screen_brightness_mode`: 1 auto, 0 manual).
pub fn set_auto_brightness(adb: &str, serial: &str, on: bool) -> String {
    let v = if on { "1" } else { "0" };
    let _ = shell(adb, serial, &["shell", "settings", "put", "system", "screen_brightness_mode", v])
        .spawn();
    if on {
        "已开启自动亮度".to_string()
    } else {
        "已关闭自动亮度".to_string()
    }
}

/// Install an APK (`adb install -r`). Returns the first non-empty adb line.
pub fn install_apk(adb: &str, serial: &str, path: &str) -> Result<String> {
    let mut c = shell(adb, serial, &["install", "-r", path]);
    let out = c
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", adb, e))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let sep = text
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| if out.status.success() { "Success".to_string() } else { "安装失败".to_string() });
    Ok(sep)
}

/// Launch an app's launcher entry. Resolves the activity through the package
/// manager, falling back to `monkey` when no LAUNCHER entry exists.
pub fn start_app(adb: &str, serial: &str, pkg: &str) {
    let resolve = sh_stdout(
        adb,
        serial,
        &[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-c",
            "android.intent.category.LAUNCHER",
            pkg,
        ],
    );
    if let Some(line) = resolve.lines().find(|l| l.contains('/')) {
        let target = line.trim();
        if !target.is_empty() && target != "null" && target != "No activity found" {
            let _ = shell(adb, serial, &["shell", "am", "start", "-n", target]).spawn();
            return;
        }
    }
    let _ = shell(
        adb,
        serial,
        &[
            "shell",
            "monkey",
            "-p",
            pkg,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
    )
    .spawn();
}

/// Force-stop a package (`am force-stop`).
pub fn force_stop(adb: &str, serial: &str, pkg: &str) {
    let _ = shell(adb, serial, &["shell", "am", "force-stop", pkg]).spawn();
}

/// Clear an app's data (`pm clear`).
pub fn clear_app(adb: &str, serial: &str, pkg: &str) -> String {
    let out = shell(adb, serial, &["shell", "pm", "clear", pkg])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if out == "Success" {
        "已清除数据".to_string()
    } else if out.ends_with("Failed") {
        format!("清除失败（{}，可能被系统保护）", pkg)
    } else {
        out
    }
}

/// Open the system settings page for an app (`APPLICATION_DETAILS_SETTINGS`).
pub fn open_app_settings(adb: &str, serial: &str, pkg: &str) {
    let uri = format!("package:{pkg}");
    let _ = shell(
        adb,
        serial,
        &[
            "shell",
            "am",
            "start",
            "-a",
            "android.settings.APPLICATION_DETAILS_SETTINGS",
            "-d",
            &uri,
        ],
    )
    .spawn();
}

/// Deep-link to a system settings screen by action string.
pub fn open_settings_action(adb: &str, serial: &str, action: &str) {
    let _ = shell(adb, serial, &["shell", "am", "start", "-a", action]).spawn();
}

/// Result of a full capture: raw PNG bytes + the uiautomator XML dump.
pub struct CaptureResult {
    pub screenshot: Vec<u8>,
    pub xml: String,
}

/// Capture the current screen as a PNG via `adb exec-out screencap -p`.
pub fn capture(adb: &str) -> Result<Vec<u8>> {
    capture_serial(adb, "")
}

/// Capture the screen of a specific device (`adb -s <serial> exec-out screencap -p`).
/// Pass an empty `serial` to target the default (single) device.
pub fn capture_serial(adb: &str, serial: &str) -> Result<Vec<u8>> {
    let mut c = Command::new(adb);
    hide_console(&mut c);
    if !serial.is_empty() {
        c.arg("-s").arg(serial);
    }
    let out = c
        .args(["exec-out", "screencap", "-p"])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}\n请确认 adb 已安装并在 PATH 中。", adb, e))?;

    if !out.status.success() {
        return Err(anyhow!(
            "screencap 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if out.stdout.is_empty() {
        return Err(anyhow!("screencap 返回空数据，请确认设备已连接。"));
    }
    Ok(out.stdout)
}

/// List connected, authorized devices (`adb devices`, state == "device").
/// Returns their serial numbers; an empty vec means nothing is connected.
pub fn list_devices(adb: &str) -> Result<Vec<String>> {
    let mut c = Command::new(adb);
    hide_console(&mut c);
    let out = c
        .arg("devices")
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", adb, e))?;
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
    Ok(devs)
}

/// Same as `dump_ui`, but targets a specific device serial (`adb -s <serial>`).
pub fn dump_ui_serial(adb: &str, serial: &str) -> Result<String> {
    let run = |args: &str| -> Result<std::process::Output> {
        let mut c = Command::new(adb);
        hide_console(&mut c);
        if !serial.is_empty() {
            c.arg("-s").arg(serial);
        }
        c.args(args.split_whitespace());
        c.output().map_err(|e| anyhow!("unable to run adb ({}): {}", adb, e))
    };

    let dump = run("shell uiautomator dump /data/local/tmp/window_dump.xml")?;
    if !dump.status.success() {
        return Err(anyhow!(
            "uiautomator dump failed: {}",
            String::from_utf8_lossy(&dump.stderr)
        ));
    }

    let cat = run("exec-out cat /data/local/tmp/window_dump.xml")?;
    if !cat.status.success() {
        return Err(anyhow!(
            "read dump file failed: {}",
            String::from_utf8_lossy(&cat.stderr)
        ));
    }

    // Best-effort cleanup of the temp file on device.
    let _ = run("shell rm -f /data/local/tmp/window_dump.xml");

    let s = String::from_utf8_lossy(&cat.stdout);
    let start = s
        .find("<?xml")
        .ok_or_else(|| anyhow!("dump output has no XML; is the device connected?"))?;
    let end = s
        .rfind("</hierarchy>")
        .ok_or_else(|| anyhow!("dump output is malformed; missing closing tag"))?
        + "</hierarchy>".len();
    Ok(s[start..end].to_string())
}

/// Dump the current UI hierarchy via `uiautomator dump`, reading the XML back.
///
/// We dump to `/data/local/tmp` (writable by shell without storage permission),
/// then `cat` it through `exec-out` (which avoids the pty line-ending munging that
/// `adb shell` would do on the raw XML).
pub fn dump_ui(adb: &str) -> Result<String> {
    let remote = "/data/local/tmp/window_dump.xml";

    let mut dump = Command::new(adb);
    hide_console(&mut dump);
    let dump = dump
        .args(["shell", "uiautomator", "dump", remote])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", adb, e))?;
    if !dump.status.success() {
        return Err(anyhow!(
            "uiautomator dump 失败: {}",
            String::from_utf8_lossy(&dump.stderr)
        ));
    }

    let mut cat = Command::new(adb);
    hide_console(&mut cat);
    let cat = cat
        .args(["exec-out", "cat", remote])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", adb, e))?;
    if !cat.status.success() {
        return Err(anyhow!(
            "读取 dump 文件失败: {}",
            String::from_utf8_lossy(&cat.stderr)
        ));
    }

    // Best-effort cleanup of the temp file on device.
    let mut rm = Command::new(adb);
    hide_console(&mut rm);
    let _ = rm.args(["shell", "rm", "-f", remote]).output();

    let s = String::from_utf8_lossy(&cat.stdout);
    let start = s
        .find("<?xml")
        .ok_or_else(|| anyhow!("dump 输出中未找到 XML（设备可能未连接或不支持 uiautomator）。"))?;
    let end = s
        .rfind("</hierarchy>")
        .ok_or_else(|| anyhow!("dump 输出格式异常（缺少 </hierarchy>）。"))?
        + "</hierarchy>".len();
    Ok(s[start..end].to_string())
}

/// Best-effort: return the foreground app's `(package, activity)`.
/// Used only to annotate recording steps with context.
///
/// Note: we run a bare `dumpsys window` and parse the `mCurrentFocus=` line
/// *locally* rather than relying on a device-side `| grep`, because many
/// devices lack `grep` in their shell or handle the pipe differently.
pub fn current_app(adb: &str, serial: &str) -> Option<(String, String)> {
    let mut c = Command::new(adb);
    hide_console(&mut c);
    if !serial.is_empty() {
        c.arg("-s").arg(serial);
    }
    c.args(["shell", "dumpsys", "window"]);
    let out = c.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(pos) = line.find("mCurrentFocus=") {
            let rest = &line[pos + "mCurrentFocus=".len()..];
            // Grab the first token containing a '/', e.g. com.pkg/.Activity
            for tok in rest.split([' ', '{', '}']) {
                if let Some(slash) = tok.find('/') {
                    let pkg = tok[..slash].to_string();
                    let act = tok[slash + 1..].trim_end_matches('}').to_string();
                    if !pkg.is_empty() {
                        return Some((pkg, act));
                    }
                }
            }
        }
    }
    None
}
