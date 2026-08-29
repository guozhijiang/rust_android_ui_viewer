"""FastAPI web backend for the Android UI Viewer.

Two modes, both served from the same single-page frontend:

- *Capture & inspection*: screenshot (`screencap`) + UI hierarchy
  (`uiautomator dump`) + properties + two-way highlight + search.
- *Live (scrcpy)*: a scrcpy v4.0 session on the device streams H.264 over a
  WebSocket; the browser decodes it with WebCodecs and renders it on a
  canvas, sending touch/key/text/scroll control messages back through the
  same socket. UI-hierarchy overlay (capture) can be superimposed live.
"""

from __future__ import annotations

import asyncio
import base64
import os
import xml.etree.ElementTree as ET
from typing import Optional

from fastapi import FastAPI, HTTPException, WebSocket, WebSocketDisconnect, Request, File, UploadFile
from fastapi.responses import FileResponse, Response
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

import adb
from errors import AppError
import scrcpy
import uitree
import u2 as u2mod

HERE = os.path.dirname(os.path.abspath(__file__))
FRONTEND = os.path.join(HERE, "..", "frontend")
STATIC = os.path.join(FRONTEND, "static")

app = FastAPI(title="Android UI Viewer (Web)")

# Single live scrcpy session shared by all clients (only one at a time).
session = scrcpy.ScrcpySession()
_ws_lock = asyncio.Lock()
_active_ws: Optional[WebSocket] = None

# u2 (uiautomator2) fast-dump state. The jar is external (= %USERPROFILE%/.u2/u2_core.jar);
# capture falls back to `uiautomator dump` when the server is not started.
u2_state = {
    "jar": u2mod.default_jar(),
    "port": u2mod.DEFAULT_PORT,
    "started": False,
    "u2": None,          # U2 instance when running on a device
    "serial": "",        # device the server was started for
    "error": "",
}


# --------------------------------------------------------------------------- #
# Helpers
# --------------------------------------------------------------------------- #
def _png_size(data: bytes) -> tuple[int, int]:
    """Read width/height from a PNG IHDR chunk without external libs."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise AppError("截图不是有效的 PNG 文件。")
    try:
        w = int.from_bytes(data[16:20], "big")
        h = int.from_bytes(data[20:24], "big")
    except Exception:
        raise AppError("无法解析截图尺寸。")
    return w, h


def _to_payload(screenshot: bytes, xml: str) -> dict:
    """Build the JSON payload shared by capture and import."""
    w, h = _png_size(screenshot)
    try:
        root = uitree.parse(xml)
    except ET.ParseError as e:
        raise AppError(f"UI 层级 XML 解析失败: {e}")
    return {
        "image": "data:image/png;base64," + base64.b64encode(screenshot).decode("ascii"),
        "width": w,
        "height": h,
        "nodeCount": root.count(),
        "tree": root.to_dict(),
        # 保留原始 XML 文本，便于导出时原样保存而不依赖前端重建
        "raw_xml": xml,
    }


# --------------------------------------------------------------------------- #
# API
# --------------------------------------------------------------------------- #
@app.get("/api/devices")
def api_devices():
    try:
        return {"devices": adb.list_devices()}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))
    except Exception as e:  # adb not found etc.
        raise HTTPException(status_code=500, detail=f"无法运行 adb: {e}")


@app.get("/api/device-info")
def api_device_info(serial: str = ""):
    try:
        return adb.device_info(serial)
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.get("/api/device-info-full")
def api_device_info_full(serial: str = ""):
    """Device info including storage summary (left panel)."""
    try:
        return adb.device_info_full(serial)
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


# ---- App / device management (mirrors Rust left panel) ----
@app.get("/api/apps")
def api_apps(serial: str = "", filter: str = "all"):
    try:
        return {"apps": adb.list_apps(serial, filter)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.get("/api/app-props")
def api_app_props(serial: str = "", pkg: str = ""):
    if not pkg:
        raise HTTPException(status_code=400, detail="pkg is required")
    try:
        return {"text": adb.app_properties(serial, pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


class PkgBody(BaseModel):
    serial: str = ""
    pkg: str


@app.post("/api/app/start")
def api_app_start(b: PkgBody):
    try:
        return {"text": adb.start_app(b.serial, b.pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/app/stop")
def api_app_stop(b: PkgBody):
    try:
        return {"text": adb.force_stop(b.serial, b.pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/app/clear")
def api_app_clear(b: PkgBody):
    try:
        return {"text": adb.clear_app(b.serial, b.pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/app/uninstall")
def api_app_uninstall(b: PkgBody):
    try:
        return {"text": adb.uninstall_app(b.serial, b.pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/app/settings")
def api_app_settings(b: PkgBody):
    try:
        return {"text": adb.open_app_settings(b.serial, b.pkg)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/app/install")
async def api_app_install(serial: str = "", request: Request = None):
    """Install an APK uploaded as the raw request body (no multipart needed)."""
    import tempfile, os
    try:
        data = await request.body()
        if not data:
            raise AppError("未收到 APK 文件内容")
        suffix = ".apk"
        with tempfile.NamedTemporaryFile(delete=False, suffix=suffix) as f:
            f.write(data)
            tmp = f.name
        try:
            return {"text": adb.install_apk(serial, tmp)}
        finally:
            try:
                os.unlink(tmp)
            except OSError:
                pass
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"安装失败: {e}")


@app.get("/api/system-settings")
def api_system_settings():
    return {"items": [{"name": n, "action": a} for n, a in adb.SYSTEM_SETTINGS]}


class ActionBody(BaseModel):
    serial: str = ""
    action: str


@app.post("/api/settings-action")
def api_settings_action(b: ActionBody):
    try:
        return {"text": adb.open_settings_action(b.serial, b.action)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


class KeyBody(BaseModel):
    serial: str = ""
    code: str


@app.post("/api/input-key")
def api_input_key(b: KeyBody):
    try:
        return {"text": adb.input_key(b.serial, b.code)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


class BrightnessBody(BaseModel):
    serial: str = ""
    value: int


@app.post("/api/brightness")
def api_brightness(b: BrightnessBody):
    try:
        return {"text": adb.set_brightness(b.serial, b.value)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


class AutoBrightBody(BaseModel):
    serial: str = ""
    on: bool


@app.post("/api/auto-brightness")
def api_auto_brightness(b: AutoBrightBody):
    try:
        return {"text": adb.set_auto_brightness(b.serial, b.on)}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.get("/api/current-app")
def api_current_app(serial: str = ""):
    try:
        r = adb.current_app(serial)
        return {"pkg": r[0] if r else "", "activity": r[1] if r else ""}
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/capture")
def api_capture(serial: str = ""):
    """Capture screenshot + UI hierarchy from the device.

    UI hierarchy prefers the fast u2 server when it has been started; otherwise
    it transparently falls back to `adb uiautomator dump`.
    """
    try:
        shot = adb.capture_screen(serial)
        xml = u2mod.fetch_hierarchy(serial, u2_state["u2"], 3000)
        return _to_payload(shot, xml)
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"抓取失败: {e}")


# --------------------------------------------------------------------------- #
# XML download
#
# The browser saves the screenshot straight from a data: URL, but a second
# programmatic download in the same click is commonly blocked (blob:/data:
# downloads in particular). Serving the hierarchy from a real HTTP endpoint
# with Content-Disposition is reliable everywhere.
# --------------------------------------------------------------------------- #
_xml_store: dict[str, str] = {}
_XML_STORE_MAX = 32


class XmlSaveBody(BaseModel):
    xml: str


@app.post("/api/save-xml")
def api_save_xml(body: XmlSaveBody):
    if not body.xml:
        raise HTTPException(status_code=400, detail="XML 为空")
    import uuid

    token = uuid.uuid4().hex[:12]
    _xml_store[token] = body.xml
    # Keep memory bounded to the most recent dumps.
    while len(_xml_store) > _XML_STORE_MAX:
        _xml_store.pop(next(iter(_xml_store)), None)
    return {"url": f"/api/download-xml/{token}"}


@app.get("/api/download-xml/{token}")
def api_download_xml(token: str):
    xml = _xml_store.pop(token, None)
    if xml is None:
        raise HTTPException(status_code=404, detail="XML 已过期，请重新抓取后再保存")
    return Response(
        content=xml.encode("utf-8"),
        media_type="text/xml; charset=utf-8",
        headers={"Content-Disposition": f'attachment; filename="hierarchy_{token}.xml"'},
    )


class U2Config(BaseModel):
    serial: str = ""
    jar: str = ""


@app.post("/api/u2/start")
def api_u2_start(cfg: U2Config):
    """Push the u2 jar and boot the on-device server for the given device."""
    try:
        jar = cfg.jar or u2_state["jar"]
        ok = u2mod.start(cfg.serial, jar, u2_state["port"])
        u2_state["jar"] = jar
        u2_state["serial"] = cfg.serial
        u2_state["started"] = ok
        u2_state["u2"] = u2mod.U2(u2_state["port"]) if ok else None
        u2_state["error"] = "" if ok else "u2 启动后未能响应，已回退 uiautomator dump"
        return {
            "started": ok,
            "message": "u2 已启动并可用" if ok else "u2 启动失败（将回退 uiautomator dump）",
        }
    except AppError as e:
        u2_state["error"] = str(e)
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/api/u2/stop")
def api_u2_stop(cfg: U2Config):
    u2mod.stop(cfg.serial or u2_state["serial"], u2_state["port"])
    u2_state["started"] = False
    u2_state["u2"] = None
    return {"stopped": True}


@app.get("/api/u2/status")
def api_u2_status():
    return {
        "jar": u2_state["jar"],
        "port": u2_state["port"],
        "started": u2_state["started"],
        "serial": u2_state["serial"],
        "error": u2_state["error"],
    }


@app.post("/api/u2/config")
def api_u2_config(cfg: U2Config):
    """Just update the configured jar path (no push)."""
    u2_state["jar"] = cfg.jar or u2_state["jar"]
    return {"jar": u2_state["jar"]}



class ImportResponse(BaseModel):
    image: str
    width: int
    height: int
    nodeCount: int
    tree: dict


@app.post("/api/import")
async def api_import(
    screenshot: UploadFile = File(...),
    ui_xml: UploadFile = File(...),
):
    """Import a local screenshot (png/jpg) + uiautomator XML file."""
    try:
        shot = await screenshot.read()
        xml = (await ui_xml.read()).decode("utf-8", "replace")
        return _to_payload(shot, xml)
    except AppError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"导入失败: {e}")


# --------------------------------------------------------------------------- #
# scrcpy live session
# --------------------------------------------------------------------------- #
class ScrcpyStart(BaseModel):
    serial: str = ""
    max_size: int = 0          # 0 = device-native resolution
    bitrate: int = 8_000_000   # bits/second


@app.post("/api/scrcpy/start")
def api_scrcpy_start(cfg: ScrcpyStart):
    """Start a live scrcpy session on the device (synchronous handshake)."""
    try:
        return session.start(cfg.serial, cfg.max_size, cfg.bitrate)
    except AppError as e:
        raise HTTPException(status_code=500, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"启动 scrcpy 失败: {e}")


@app.post("/api/scrcpy/stop")
def api_scrcpy_stop():
    session.stop()
    return {"stopped": True}


@app.get("/api/scrcpy/status")
def api_scrcpy_status():
    return {
        "running": session.running,
        "serial": session.serial,
        "width": session.width,
        "height": session.height,
        "deviceName": session.device_name,
        "error": session.error,
    }


async def _ws_sender(ws: WebSocket, sess: scrcpy.ScrcpySession) -> None:
    """Forward parsed video items to the browser until the stream closes."""
    codec_sent = False
    while True:
        # The codec string is parsed from the first SPS, which arrives just
        # before the first media frame — deliver it as soon as it's ready.
        if not codec_sent and sess.codec_str:
            await ws.send_json({"type": "codec", "codec": sess.codec_str})
            codec_sent = True
        item = await asyncio.to_thread(sess.video_queue.get)
        kind = item[0]
        if kind == "media":
            await ws.send_bytes(item[1])
        elif kind == "size":
            await ws.send_json(
                {"type": "size", "width": item[1], "height": item[2]}
            )
        else:  # closed
            if sess.recovering:
                # Abnormal drop with auto-recovery in flight: keep the
                # socket open; the new session's frames arrive right after.
                await ws.send_json({"type": "recovering"})
                continue
            await ws.send_json({"type": "closed", "error": item[1]})
            return


async def _ws_receiver(ws: WebSocket, sess: scrcpy.ScrcpySession) -> None:
    """Handle control messages from the browser (touch/key/text/scroll)."""
    while True:
        msg = await ws.receive_json()
        kind = msg.get("type")
        try:
            if kind == "touch":
                sess.touch(
                    msg["action"], msg["x"], msg["y"],
                    msg.get("pressure", scrcpy.PRESSURE_MAX),
                    msg.get("pointerId", 0),
                )
            elif kind == "key":
                sess.key(msg["action"], msg["keycode"], msg.get("meta", 0))
            elif kind == "text":
                sess.text(msg["text"])
            elif kind == "scroll":
                sess.scroll(msg["x"], msg["y"], msg.get("h", 0.0), msg.get("v", 0.0))
        except (KeyError, TypeError):
            pass  # malformed control frame: ignore


@app.websocket("/ws/scrcpy")
async def ws_scrcpy(ws: WebSocket):
    """Bidirectional live channel.

    Downlink: binary H.264 Annex-B access units + JSON {type:size|closed}.
    Uplink:   JSON {type:touch|key|text|scroll, ...}.
    Only one client at a time; closing the socket stops the session
    (page refresh == release device).
    """
    global _active_ws
    await ws.accept()
    sess = session
    if not sess.running:
        await ws.send_json(
            {"type": "closed", "error": "scrcpy 会话未启动，请先点击「连接」。"}
        )
        await ws.close()
        return

    async with _ws_lock:
        if _active_ws is not None:
            await ws.send_json(
                {"type": "closed", "error": "已有客户端连接，请先断开旧连接。"}
            )
            await ws.close()
            return
        _active_ws = ws
    sess._ws_connected = True
    try:
        sender = asyncio.create_task(_ws_sender(ws, sess))
        receiver = asyncio.create_task(_ws_receiver(ws, sess))
        done, pending = await asyncio.wait(
            {sender, receiver}, return_when=asyncio.FIRST_COMPLETED
        )
        for t in pending:
            t.cancel()
        if sender in done:
            sess.stop()
    except WebSocketDisconnect:
        pass
    finally:
        for t in (sender, receiver):
            t.cancel()
        sess.stop()
        sess._ws_connected = False
        _active_ws = None


# --------------------------------------------------------------------------- #
# Frontend serving
# --------------------------------------------------------------------------- #
class NoCacheStaticFiles(StaticFiles):
    """Static files with aggressive no-cache headers during development."""
    async def get_response(self, path: str, scope):
        resp = await super().get_response(path, scope)
        resp.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
        resp.headers["Pragma"] = "no-cache"
        resp.headers["Expires"] = "0"
        return resp


@app.get("/")
def index():
    return FileResponse(
        os.path.join(FRONTEND, "index.html"),
        headers={
            "Cache-Control": "no-cache, no-store, must-revalidate",
            "Pragma": "no-cache",
            "Expires": "0",
        },
    )


if os.path.isdir(STATIC):
    app.mount("/static", NoCacheStaticFiles(directory=STATIC), name="static")


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=8000)
