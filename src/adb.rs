use std::process::Command;

use anyhow::{anyhow, Result};

/// Result of a full capture: raw PNG bytes + the uiautomator XML dump.
pub struct CaptureResult {
    pub screenshot: Vec<u8>,
    pub xml: String,
}

/// Capture the current screen as a PNG via `adb exec-out screencap -p`.
pub fn capture(adb: &str) -> Result<Vec<u8>> {
    let out = Command::new(adb)
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

/// Dump the current UI hierarchy via `uiautomator dump`, reading the XML back.
///
/// We dump to `/data/local/tmp` (writable by shell without storage permission),
/// then `cat` it through `exec-out` (which avoids the pty line-ending munging that
/// `adb shell` would do on the raw XML).
pub fn dump_ui(adb: &str) -> Result<String> {
    let remote = "/data/local/tmp/window_dump.xml";

    let dump = Command::new(adb)
        .args(["shell", "uiautomator", "dump", remote])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", adb, e))?;
    if !dump.status.success() {
        return Err(anyhow!(
            "uiautomator dump 失败: {}",
            String::from_utf8_lossy(&dump.stderr)
        ));
    }

    let cat = Command::new(adb)
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
    let _ = Command::new(adb).args(["shell", "rm", "-f", remote]).output();

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
