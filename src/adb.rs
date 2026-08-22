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
