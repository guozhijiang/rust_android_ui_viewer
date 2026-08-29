"""u2 (uiautomator2) integration for fast UI-hierarchy dumping.

Port of the desktop backend's ``src/u2.rs`` to the web backend. The jar is an
external binary (``u2_core.jar``, entry class ``com.wetest.uia2.Main``) shipped
by ``openatx/android-uiautomator-server-jar`` (v0.4.0). It is NOT bundled — the
UI exposes a configurable host-path and falls back to ``adb.uiautomator dump``
when the server isn't reachable, so capture keeps working everywhere.
"""

from __future__ import annotations

import json
import os
import time
import urllib.request

import adb
from errors import err

DEFAULT_PORT = 7912
REMOTE_JAR = "/data/local/tmp/u2_core.jar"


def default_jar() -> str:
    """Default host-side jar location (mirrors the desktop app)."""
    return os.path.join(os.path.expanduser("~"), ".u2", "u2_core.jar")


class U2:
    """Handle to an on-device u2 server reached through an adb forward tunnel."""

    def __init__(self, port: int = DEFAULT_PORT):
        self.port = port

    def endpoint(self) -> str:
        return f"http://127.0.0.1:{self.port}/jsonrpc/0"

    def fetch_hierarchy(self, timeout_ms: int) -> str:
        """Fetch UI hierarchy XML via JSON-RPC ``dumpWindowHierarchy``.

        Returns the raw XML string, or raises on transport/parse/RPC failure.
        """
        body = json.dumps(
            {"jsonrpc": "2.0", "method": "dumpWindowHierarchy",
             "params": [False], "id": 1}
        ).encode("utf-8")
        req = urllib.request.Request(
            self.endpoint(),
            data=body,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout_ms / 1000.0) as resp:
                text = resp.read().decode("utf-8")
        except Exception as e:
            raise err(f"u2 请求失败 ({self.endpoint()}): {e}")
        try:
            v = json.loads(text)
        except Exception as e:
            raise err(f"u2 响应 JSON 解析失败: {e}")
        if "error" in v and v["error"]:
            raise err(f"u2 RPC 错误: {v['error']}")
        result = v.get("result")
        if not isinstance(result, str):
            raise err(f"u2 响应缺少字符串 result 字段: {text[:120]}")
        return result


def start(serial: str, host_jar: str, port: int = DEFAULT_PORT) -> bool:
    """Push the jar, forward the port, and boot the on-device server.

    Returns True if the server answers a probe dump, False if it started but
    did not respond in time (caller keeps using adb dumps).
    """
    # 1. Remove stale copy, then push the jar.
    adb._run(["shell", "rm", "-f", REMOTE_JAR], serial=serial)
    push = adb._run(["push", host_jar, REMOTE_JAR], serial=serial)
    if push.returncode != 0:
        raise err("推送 u2 jar 失败: " +
                  (push.stderr.decode("utf-8", "replace") or "未知错误"))

    # 2. Expose the device-side port on localhost. The spec must be written as
    #    `tcp:<port>` — a bare number is rejected by modern adb.
    spec = f"tcp:{port}"
    adb._run(["forward", "--remove", spec], serial=serial)
    fwd = adb._run(["forward", spec, spec], serial=serial)
    if fwd.returncode != 0:
        raise err("adb forward 失败: " +
                  (fwd.stderr.decode("utf-8", "replace") or "未知错误"))

    # 3. Bootstrap the server via app_process (CLASSPATH = pushed jar). The
    #    `setsid` runs on the device (Android is Linux), disowning the process
    #    so it survives the `adb shell` exit.
    shell_cmd = (
        f"setsid sh -c 'CLASSPATH={REMOTE_JAR} app_process / "
        f"com.wetest.uia2.Main -p {port}' </dev/null >/dev/null 2>&1 &"
    )
    adb._run(["shell", shell_cmd], serial=serial)

    # Give it a moment to bind, then probe with a quick hierarchy fetch.
    time.sleep(1.5)
    try:
        return bool(U2(port).fetch_hierarchy(1400))
    except Exception:
        return False


def stop(serial: str, port: int = DEFAULT_PORT) -> None:
    """Best-effort: drop the forward and kill the on-device server."""
    spec = f"tcp:{port}"
    adb._run(["forward", "--remove", spec], serial=serial)
    adb._run(["shell", "pkill", "-f", "com.wetest.uia2.Main"], serial=serial)


def fetch_hierarchy(serial: str, u2: "U2 | None", timeout_ms: int) -> str:
    """Get the UI hierarchy, preferring the fast u2 server; fall back to dump."""
    if u2 is not None:
        try:
            xml = u2.fetch_hierarchy(timeout_ms)
            if "<hierarchy" in xml:
                return xml
        except Exception:
            pass
    return adb.dump_ui(serial)
