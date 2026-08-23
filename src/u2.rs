//! On-device u2 (uiautomator2) integration.
//!
//! The `uiautomator2` project ships an on-device uiautomator server (a
//! `core-src.jar` / `app-uiautomator.apk`) that exposes a **fast** JSON-RPC
//! endpoint over local TCP. Querying the UI hierarchy through it is
//! substantially quicker than shelling out to `uiautomator dump` for every
//! frame, which is what keeps recording/selection responsive.
//!
//! The jar is an external binary and is NOT bundled with this crate. The UI
//! exposes a configurable host-side path. When it isn't configured, callers
//! transparently fall back to the regular `adb … uiautomator dump` path so
//! nothing breaks.

use std::os::windows::process::CommandExt as _;
use std::process::Command;

use anyhow::{anyhow, Result};

use crate::adb;

/// Conventional JSON-RPC port the uiautomator2 server listens on.
pub const DEFAULT_PORT: u16 = 7912;
/// Remote copy location for the jar on the device.
const REMOTE_JAR: &str = "/data/local/tmp/u2_core.jar";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A handle to a reachable on-device u2 server, reached through an
/// `adb forward tcp:<port> tcp:<port>` tunnel on localhost.
#[derive(Clone, Debug)]
pub struct U2 {
    pub port: u16,
}

fn adb(bin: &str, serial: &str, args: &[&str]) -> Command {
    let mut c = Command::new(bin);
    c.creation_flags(CREATE_NO_WINDOW);
    if !serial.is_empty() {
        c.arg("-s").arg(serial);
    }
    c.args(args);
    c
}

/// Copy the host-side u2 jar to the device and start the server.
///
/// Returns `Ok(true)` if the server later answers a hierarchy fetch, `Ok(false)`
/// if it started but did not answer in time (caller keeps using adb dumps).
///
/// Best-effort: the exact `app_process` bootstrap depends on the jar / Android
/// version, so the entry class may need adjusting to the specific build.
pub fn start(bin: &str, serial: &str, host_jar: &str, port: u16) -> Result<bool> {
    // 1. Remove any stale copy, then push the current jar.
    let _ = adb(bin, serial, &["shell", "rm", "-f", REMOTE_JAR]).output();
    let push = adb(bin, serial, &["push", host_jar, REMOTE_JAR])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", bin, e))?;
    if !push.status.success() {
        return Err(anyhow!(
            "推送 u2 jar 失败: {}",
            String::from_utf8_lossy(&push.stderr)
        ));
    }

    // 2. Expose the device-side port on localhost. The local socket spec must
    //    be written as `tcp:<port>` — a bare number is rejected by modern adb
    //    ("unknown socket specification: <port>").
    let spec = format!("tcp:{}", port);
    let _ = adb(bin, serial, &["forward", "--remove", &spec]).output();
    let fwd = adb(bin, serial, &["forward", &spec, &spec])
        .output()
        .map_err(|e| anyhow!("无法运行 adb ({}): {}", bin, e))?;
    if !fwd.status.success() {
        return Err(anyhow!(
            "adb forward 失败: {}",
            String::from_utf8_lossy(&fwd.stderr)
        ));
    }

    // 3. Bootstrap the server via app_process (classpath = the pushed jar).
    //    Entry class + args verified against `uiautomator2` 3.7.0 / the
    //    `android-uiautomator-server-jar` v0.4.0 jar: listening port is given
    //    after a `-p` flag and the app_process root dir is `/`. The server is
    //    disowned from the shell so it survives when `adb shell` exits; `sh -c`
    //    is required so the leading `CLASSPATH=…` assignment is applied.
    let shell_cmd = format!(
        "setsid sh -c 'CLASSPATH={REMOTE_JAR} app_process / com.wetest.uia2.Main -p {port}' </dev/null >/dev/null 2>&1 &"
    );
    let _ = adb(bin, serial, &["shell", &shell_cmd]).spawn();

    // Give it a moment to bind, then probe it with a quick hierarchy fetch.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    Ok(U2::new(port).fetch_hierarchy(1400).is_ok())
}

impl U2 {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// The URL we POST the JSON-RPC to (localhost, forwarded to the device).
    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/jsonrpc/0", self.port)
    }

    /// Fetch the current UI hierarchy XML through the u2 server. Much faster
    /// than `uiautomator dump` because it runs inside the instrumentation.
    ///
    /// The `dumpWindowHierarchy` RPC requires a positional boolean argument, so
    /// it is sent as an array payload `[false]` (the `false` = uncompressed).
    pub fn fetch_hierarchy(&self, timeout_ms: u32) -> Result<String> {
        let body = "{\"jsonrpc\":\"2.0\",\"method\":\"dumpWindowHierarchy\",\"params\":[false],\"id\":1}".to_string();
        self.call_json_raw(body, timeout_ms)
    }

    /// A single raw JSON-RPC POST; returns the string `result` field.
    fn call_json_raw(&self, body: String, timeout_ms: u32) -> Result<String> {
        let resp = minreq::post(self.endpoint())
            .with_body(body)
            .with_timeout(timeout_ms as u64)
            .send()
            .map_err(|e| anyhow!("u2 请求失败 ({}): {}", self.endpoint(), e))?;
        let text = resp.as_str().map_err(|e| anyhow!("u2 响应非文本: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| anyhow!("u2 响应 JSON 解析失败: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(anyhow!("u2 RPC 错误: {}", err));
        }
        v.get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("u2 响应缺少字符串 result 字段: {}", truncate(text, 120)))
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        s[..n].to_string() + "…"
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn truncate_long_gets_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hello…");
    }
}

/// Fetch the UI hierarchy, preferring the fast u2 server when available;
/// otherwise fall back to the (slower) `uiautomator dump`.
pub fn fetch_hierarchy(
    adb: &str,
    serial: &str,
    u2: Option<&U2>,
    timeout_ms: u32,
) -> Result<String> {
    if let Some(server) = u2 {
        if let Ok(xml) = server.fetch_hierarchy(timeout_ms) {
            if xml.contains("<hierarchy") {
                return Ok(xml);
            }
        }
    }
    adb::dump_ui_serial(adb, serial)
}