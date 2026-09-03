"""Python port of the Rust `adb.rs` capture/dump/device helpers.

Keeps the same adb command shapes used by the original desktop app:
  - screenshot via `adb exec-out screencap -p`
  - UI hierarchy via `adb shell uiautomator dump ...` + `exec-out cat`
  - device discovery via `adb devices`

All functions are Windows/Unix safe and honour an explicit device serial.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import time
from typing import Optional

from errors import err

# `adb` binary: prefer an explicit override, else rely on PATH.
ADB = os.environ.get("ADB_PATH") or shutil.which("adb") or "adb"


def _run(args: list[str], serial: str = "", timeout: float = 60.0) -> subprocess.CompletedProcess:
    """Run an adb invocation, optionally scoped to `-s <serial>`."""
    cmd = [ADB]
    if serial:
        cmd += ["-s", serial]
    cmd += args
    # Use CREATE_NO_WINDOW on Windows to avoid a popping console (mirrors Rust).
    creationflags = 0x08000000 if os.name == "nt" else 0
    return subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        creationflags=creationflags,
    )


def list_devices() -> list[str]:
    """Return serials of connected, authorized devices (state == "device")."""
    out = _run(["devices"])
    text = out.stdout.decode("utf-8", "replace")
    devs: list[str] = []
    for line in text.splitlines()[1:]:
        line = line.strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) >= 2 and parts[1] == "device":
            devs.append(parts[0])
    return devs


def capture_screen(serial: str = "") -> bytes:
    """Capture the current screen as a PNG via `adb exec-out screencap -p`."""
    out = _run(["exec-out", "screencap", "-p"], serial=serial)
    if not out.returncode == 0 or not out.stdout:
        raise err(
            "screencap 失败: " + (out.stderr.decode("utf-8", "replace") or "无输出")
        )
    if not out.stdout:
        raise err("screencap 返回空数据，请确认设备已连接。")
    return out.stdout


def wake_screen(serial: str = "") -> None:
    """Best-effort: turn the display on before dumping.

    `uiautomator dump` (and a meaningful screencap) fail when the screen is
    off, so we send KEYCODE_WAKEUP (224) — it only wakes, never toggles the
    screen off. Ignored if it errors (e.g. already on, or no input service).
    """
    _run(["shell", "input", "keyevent", "224"], serial=serial)


def dump_ui(serial: str = "") -> str:
    """Dump the current UI hierarchy via `uiautomator dump` and read it back.

    Mirrors the original: dump to a writable temp path, `cat` it through
    `exec-out` (avoids pty line-ending munging), then best-effort cleanup.

    Some ROMs can't write to /data/local/tmp via uiautomator, so we fall back
    to the default sdcard dump location when the explicit path fails.
    """
    remotes = ["/data/local/tmp/window_dump.xml", "/sdcard/window_dump.xml"]
    raw = ""
    used = ""
    last_err = ""
    for remote in remotes:
        dump = _run(["shell", "uiautomator", "dump", remote], serial=serial, timeout=30)
        if dump.returncode != 0:
            last_err = dump.stderr.decode("utf-8", "replace") or "非零退出"
            continue
        cat = _run(["exec-out", "cat", remote], serial=serial)
        if cat.returncode != 0:
            last_err = cat.stderr.decode("utf-8", "replace") or "读取失败"
            continue
        text = cat.stdout.decode("utf-8", "replace")
        if "<?xml" in text:
            raw = text
            used = remote
            break
        last_err = "dump 输出中未找到 XML"

    if not raw:
        raise err("uiautomator dump 失败: " + (last_err or "无输出"))

    _run(["shell", "rm", "-f", used], serial=serial)

    s = raw
    start = s.find("<?xml")
    end = s.rfind("</hierarchy>")
    if start == -1:
        raise err("dump 输出中未找到 XML（设备可能未连接或不支持 uiautomator）。")
    if end == -1:
        raise err("dump 输出格式异常（缺少 </hierarchy>）。")
    return s[start : end + len("</hierarchy>")]


def device_info(serial: str = "") -> dict:
    """Gather a small device-properties summary for display."""
    out = _run(["shell", "getprop"], serial=serial).stdout.decode("utf-8", "replace")
    props: dict[str, str] = {}
    for line in out.splitlines():
        line = line.strip()
        if not line.startswith("["):
            continue
        close = line.find("]")
        if close == -1:
            continue
        key = line[1:close]
        after = line[close + 1 :].strip()
        if after.startswith(": ["):
            val = after[3:].rstrip("]").strip()
            props[key] = val

    size_out = _run(["shell", "wm", "size"], serial=serial).stdout.decode("utf-8", "replace")
    resolution = ""
    for line in size_out.splitlines():
        if "Physical size:" in line:
            resolution = line.split("Physical size:")[1].strip()

    dpi = props.get("ro.sf.lcd_density", "")
    battery = _battery_pct(serial)

    return {
        "brand": props.get("ro.product.brand", ""),
        "model": props.get("ro.product.model", ""),
        "android": props.get("ro.build.version.release", ""),
        "sdk": props.get("ro.build.version.sdk", ""),
        "resolution": resolution,
        "density": f"{dpi} dpi" if dpi else "",
        "battery": battery,
        "serial": serial,
        "build": props.get("ro.build.display.id", "")
        or props.get("ro.build.version.incremental", ""),
    }


def _battery_pct(serial: str) -> str:
    out = _run(["shell", "dumpsys", "battery"], serial=serial).stdout.decode("utf-8", "replace")
    for line in out.splitlines():
        line = line.lstrip()
        if line.startswith("level:"):
            try:
                return f"{int(line[len('level:'):].strip())}%"
            except ValueError:
                return ""
    return ""


def current_app(serial: str = "") -> Optional[tuple[str, str]]:
    """Best-effort: return the foreground app's (package, activity)."""
    out = _run(["shell", "dumpsys", "window"], serial=serial).stdout.decode("utf-8", "replace")
    for line in out.splitlines():
        pos = line.find("mCurrentFocus=")
        if pos == -1:
            continue
        rest = line[pos + len("mCurrentFocus=") :]
        for tok in re.split(r"[ {}]", rest):
            slash = tok.find("/")
            if slash != -1:
                pkg = tok[:slash]
                act = tok[slash + 1 :].strip("}")
                if pkg:
                    return pkg, act
    return None


# --------------------------------------------------------------------------- #
# App management (mirrors adb.rs list_apps / app_properties / install / ...)
# --------------------------------------------------------------------------- #
def _sh_stdout(args: list[str], serial: str = "") -> str:
    return _run(args, serial=serial).stdout.decode("utf-8", "replace")


def list_apps(serial: str = "", filter: str = "all") -> list[dict]:
    """List installed packages; filter: all | system | third | running."""
    third = {
        line[len("package:") :].strip()
        for line in _sh_stdout(["shell", "pm", "list", "packages", "-3"], serial).splitlines()
        if line.startswith("package:")
    }
    running = {
        line.strip()
        for line in _sh_stdout(["shell", "ps", "-A", "-o", "NAME"], serial).splitlines()
        if line.strip() and line.strip() != "NAME"
    }
    apps: list[dict] = []
    for line in _sh_stdout(["shell", "pm", "list", "packages"], serial).splitlines():
        if not line.startswith("package:"):
            continue
        pkg = line[len("package:") :].strip()
        if not pkg:
            continue
        tp = pkg in third
        run = any(pn == pkg or pkg.startswith(pn) or pn.startswith(pkg) for pn in running)
        apps.append({"package": pkg, "thirdParty": tp, "running": run})
    apps.sort(key=lambda a: a["package"])
    if filter == "system":
        apps = [a for a in apps if not a["thirdParty"]]
    elif filter == "third":
        apps = [a for a in apps if a["thirdParty"]]
    elif filter == "running":
        apps = [a for a in apps if a["running"]]
    return apps


def app_properties(serial: str, pkg: str) -> str:
    """Read version/install metadata via `dumpsys package` (mirrors Rust)."""
    out = _sh_stdout(["shell", "dumpsys", "package", pkg], serial)
    seen: dict[str, str] = {}
    for line in out.splitlines():
        for key in ("versionName", "versionCode", "firstInstallTime", "lastUpdateTime"):
            pat = f"{key}="
            pos = line.find(pat)
            if pos == -1:
                continue
            val = line[pos + len(pat) :].split()[0].rstrip("}") if line[pos + len(pat) :].split() else ""
            if val and key not in seen:
                seen[key] = val
    if not seen:
        return f"未查询到 {pkg} 的信息（包未安装？）"
    lines = [f"包名: {pkg}"] + [f"{k}: {v}" for k, v in seen.items()]
    return "\n".join(lines)


def start_app(serial: str, pkg: str) -> str:
    """Launch an app's launcher entry (resolve-activity, fallback to monkey)."""
    resolve = _sh_stdout(
        ["shell", "cmd", "package", "resolve-activity", "--brief",
         "-c", "android.intent.category.LAUNCHER", pkg], serial
    )
    for line in resolve.splitlines():
        if "/" in line:
            target = line.strip()
            if target and target not in ("null", "No activity found"):
                _run(["shell", "am", "start", "-n", target], serial=serial)
                return f"已启动 {pkg}"
    _run(["shell", "monkey", "-p", pkg, "-c",
          "android.intent.category.LAUNCHER", "1"], serial=serial)
    return f"已启动 {pkg}（monkey）"


def force_stop(serial: str, pkg: str) -> str:
    _run(["shell", "am", "force-stop", pkg], serial=serial)
    return f"已停止 {pkg}"


def clear_app(serial: str, pkg: str) -> str:
    out = _run(["shell", "pm", "clear", pkg], serial=serial).stdout.decode("utf-8", "replace").strip()
    if out == "Success":
        return "已清除数据"
    if out.endswith("Failed"):
        return f"清除失败（{pkg}，可能被系统保护）"
    return out


def uninstall_app(serial: str, pkg: str) -> str:
    out = _run(["shell", "pm", "uninstall", "--user", "0", pkg], serial=serial).stdout.decode("utf-8", "replace").strip()
    if not out or out == "Success":
        return f"已卸载 {pkg}"
    return out


def install_apk(serial: str, path: str) -> str:
    """`adb install -r <path>` — path must be host-local (uploaded first)."""
    out = _run(["install", "-r", path], serial=serial)
    text = out.stdout.decode("utf-8", "replace")
    for line in reversed(text.splitlines()):
        if line.strip():
            return line.strip()
    return "Success" if out.returncode == 0 else "安装失败"


def open_app_settings(serial: str, pkg: str) -> str:
    uri = f"package:{pkg}"
    _run(["shell", "am", "start", "-a",
          "android.settings.APPLICATION_DETAILS_SETTINGS", "-d", uri], serial=serial)
    return f"已打开 {pkg} 应用信息"


# System settings deep-links (mirrors adb.rs SYSTEM_SETTINGS).
SYSTEM_SETTINGS = [
    ("Wi-Fi", "android.settings.WIFI_SETTINGS"),
    ("蓝牙", "android.settings.BLUETOOTH_SETTINGS"),
    ("声音", "android.settings.SOUND_SETTINGS"),
    ("显示", "android.settings.DISPLAY_SETTINGS"),
    ("通知", "android.settings.NOTIFICATION_SETTINGS"),
    ("应用", "android.settings.APPLICATION_SETTINGS"),
    ("电池", "android.settings.BATTERY_SAVER_SETTINGS"),
    ("辅助功能", "android.settings.ACCESSIBILITY_SETTINGS"),
]


def open_settings_action(serial: str, action: str) -> str:
    _run(["shell", "am", "start", "-a", action], serial=serial)
    return f"已打开 {action}"


def input_key(serial: str, code: str) -> str:
    _run(["shell", "input", "keyevent", code], serial=serial)
    return f"已发送 keyevent {code}"


# ---- adb input injection (replay recordings in capture mode too) ----
# Mirrors the Rust `record.rs` replay path, which drives the device through
# `adb shell input` regardless of whether a live scrcpy session is active.
def input_tap(serial: str, x: int, y: int, long: bool = False) -> str:
    if long:
        # long-press == stationary swipe with a long duration (same as Rust)
        _run(["shell", "input", "swipe", str(x), str(y), str(x), str(y), "600"],
             serial=serial)
        return f"长按 {x},{y}"
    _run(["shell", "input", "tap", str(x), str(y)], serial=serial)
    return f"tap {x},{y}"


def input_swipe(serial: str, x1: int, y1: int, x2: int, y2: int, ms: int = 200) -> str:
    _run(["shell", "input", "swipe", str(x1), str(y1), str(x2), str(y2), str(ms)],
         serial=serial)
    return "swipe"


def input_text(serial: str, text: str) -> str:
    # Escape '%' and spaces the way `adb shell input text` expects.
    esc = text.replace("%", "%%").replace(" ", "%s")
    _run(["shell", "input", "text", esc], serial=serial)
    return "text"


def set_brightness(serial: str, value: int) -> str:
    _run(["shell", "settings", "put", "system", "screen_brightness", str(value)], serial=serial)
    return f"亮度已设为 {value * 100 // 255}%"


def set_auto_brightness(serial: str, on: bool) -> str:
    _run(["shell", "settings", "put", "system", "screen_brightness_mode", "1" if on else "0"], serial=serial)
    return "已开启自动亮度" if on else "已关闭自动亮度"


def storage_summary(serial: str) -> str:
    out = _sh_stdout(["shell", "df", "/data"], serial)
    lines = out.splitlines()
    if len(lines) < 2:
        return ""
    cols = lines[1].split()
    if len(cols) < 4:
        return ""
    def to_gb(blocks: str) -> str:
        try:
            return f"{int(blocks) // 1024 // 1024}G"
        except ValueError:
            return ""
    total = to_gb(cols[1])
    avail = to_gb(cols[3])
    if not total:
        return ""
    if not avail:
        return f"总 {total}"
    return f"可用 {avail} / 总 {total}"


def device_info_full(serial: str = "") -> dict:
    """Device info including storage (used by the web device panel)."""
    info = device_info(serial)
    info["storage"] = storage_summary(serial)
    return info


def screen_size(serial: str = "") -> tuple[int, int]:
    """Best-effort physical screen size via `wm size`; falls back to 1080x1920.

    Mirrors record.rs `screen_size` so replay coordinate fallback matches.
    """
    try:
        out = _run(["shell", "wm", "size"], serial=serial)
        text = out.stdout.decode("utf-8", "replace")
        for line in text.splitlines():
            rest = line.split("size:", 1)
            if len(rest) == 2:
                parts = rest[1].strip().split("x")
                if len(parts) == 2:
                    w, h = int(parts[0].strip()), int(parts[1].strip())
                    if w > 0 and h > 0:
                        return w, h
    except Exception:
        pass
    return (1080, 1920)
