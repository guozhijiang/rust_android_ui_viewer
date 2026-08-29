"""Python port of the Rust `live.rs` scrcpy live session.

Runs the scrcpy server on the device, parses the raw H.264 video stream and
exposes it through a thread-safe queue, plus a real-time control channel
(touch / key / text / scroll) exactly matching the wire format verified
against the Rust desktop app (scrcpy v4.0).

Video stream layout (reverse tunnel):
  1. 64-byte device name
  2. 4-byte big-endian codec id      ("h264" = 0x68323634)
  3. 12-byte session header          (MSB of byte 0 set, then width/height BE32)
  4. repeating 12-byte frame header followed by <len> bytes of H.264
     (Annex-B). Config packets (SPS/PPS) are merged with the next media packet.
     A header with the MSB of byte 0 set announces a size/orientation change.

The browser cannot decode raw H.264, but modern Chrome/Edge can via
WebCodecs. The backend therefore forwards Annex-B access units over a
WebSocket; the frontend decodes them with `VideoDecoder` and renders to a
canvas, sending control messages back through the same socket.
"""

from __future__ import annotations

import os
import queue
import shutil
import socket
import struct
import subprocess
import threading
import time
import urllib.request
import zipfile
from pathlib import Path
from typing import Optional

from errors import AppError

ADB = os.environ.get("ADB_PATH") or shutil.which("adb") or "adb"

# ---- scrcpy control protocol (client -> device), scrcpy v4.0 ----
MSG_INJECT_KEYCODE = 0
MSG_INJECT_TEXT = 1
MSG_INJECT_TOUCH_EVENT = 2
MSG_INJECT_SCROLL_EVENT = 3

ACTION_DOWN = 0
ACTION_UP = 1
ACTION_MOVE = 2

PRESSURE_MAX = 0xFFFF

# ---- video stream constants ----
CODEC_ID_H264 = 0x68323634  # "h264"
FLAG_CONFIG = 1 << 62
DEVICE_NAME_LEN = 64
HEADER_LEN = 12
MAX_PACKET = 1 << 23

# Keep the queue small so a slow WebSocket client drops frames instead of
# accumulating latency (real-time mirror semantics).
VIDEO_QUEUE_MAX = 30

# Automatic bundle download (mirrors the desktop app's auto-fetch).
BUNDLE_URL = "https://github.com/Genymobile/scrcpy/releases/download/v4.0/scrcpy-win64-v4.0.zip"
BUNDLE_NEEDED = ("scrcpy-server", "avcodec-62.dll", "avutil-60.dll")


def socket_name(scid: int) -> str:
    return f"scrcpy_{scid:08x}"


def h264_codec_string(data: bytes) -> Optional[str]:
    """Derive an `avc1.<profile><constraint><level>` string from an SPS NAL.

    The browser's `VideoDecoder.configure` needs a codec string; Chrome is
    picky about the profile/level, so we parse them out of the first SPS we
    see (nal_unit_type == 7) instead of hardcoding a guess.
    """
    n = len(data)
    for i in range(n - 6):
        if data[i] == 0 and data[i + 1] == 0 and data[i + 2] == 1:
            if data[i + 3] & 0x1F == 7:  # SPS
                profile = data[i + 4]
                constraint = data[i + 5]
                level = data[i + 6]
                return f"avc1.{profile:02x}{constraint:02x}{level:02x}"
    return None


# --------------------------------------------------------------------------- #
# adb helpers
# --------------------------------------------------------------------------- #
def _popen(serial: str, args: list[str]) -> subprocess.Popen:
    """Spawn a long-running adb command without a console window."""
    cmd = [ADB]
    if serial:
        cmd += ["-s", serial]
    cmd += args
    flags = 0x08000000 if os.name == "nt" else 0  # CREATE_NO_WINDOW
    return subprocess.Popen(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=flags,
    )


def _adb(serial: str, args: list[str], timeout: float = 20.0) -> subprocess.CompletedProcess:
    cmd = [ADB]
    if serial:
        cmd += ["-s", serial]
    cmd += args
    flags = 0x08000000 if os.name == "nt" else 0
    return subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        creationflags=flags,
    )


def _adb_ok(serial: str, args: list[str], timeout: float = 20.0) -> bool:
    try:
        return _adb(serial, args, timeout).returncode == 0
    except Exception:
        return False


def _resolve_serial(hint: str) -> str:
    """Return the serial to use: the hint if given, else the sole connected one."""
    if hint.strip():
        return hint.strip()
    out = _adb("", ["devices"])
    text = out.stdout.decode("utf-8", "replace")
    devs: list[str] = []
    for line in text.splitlines()[1:]:
        line = line.strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) >= 2 and parts[1] == "device":
            devs.append(parts[0])
    if not devs:
        raise AppError("未检测到已连接的设备（adb devices 无已授权设备）")
    if len(devs) > 1:
        raise AppError(f"检测到 {len(devs)} 台设备，请指定目标设备序列号")
    return devs[0]


# --------------------------------------------------------------------------- #
# scrcpy-server discovery (local install -> auto download)
# --------------------------------------------------------------------------- #
def _find_scrcpy_server() -> Path:
    env = os.environ.get("SCRCPY_SERVER_PATH")
    if env and Path(env).is_file():
        return Path(env)

    env_dir = os.environ.get("SCRCPY_DIR")
    if env_dir:
        p = Path(env_dir) / "scrcpy-server"
        if p.is_file():
            return p

    where = shutil.which("scrcpy")
    if where:
        p = Path(where).resolve().parent / "scrcpy-server"
        if p.is_file():
            return p

    return _auto_fetch_server()


def _auto_fetch_server() -> Path:
    """Download the official scrcpy v4.0 Windows bundle once and reuse it."""
    base = Path(
        os.environ.get("LOCALAPPDATA", str(Path.home()))
    ) / "android-ui-viewer" / "scrcpy"
    base.mkdir(parents=True, exist_ok=True)
    zip_path = base / "scrcpy-win64-v4.0.zip"

    if not all((base / n).is_file() for n in BUNDLE_NEEDED):
        if not zip_path.is_file():
            try:
                print(f"正在下载 scrcpy v4.0 包（首次运行，约 30MB）…")
                urllib.request.urlretrieve(BUNDLE_URL, zip_path)
            except Exception as e:
                raise AppError(
                    f"未找到 scrcpy-server，且自动下载失败: {e}\n"
                    f"请手动下载 {BUNDLE_URL} 解压后设置 SCRCPY_SERVER_PATH 环境变量。"
                )
        try:
            with zipfile.ZipFile(zip_path) as zf:
                for name in BUNDLE_NEEDED:
                    dest = base / name
                    if dest.is_file():
                        continue
                    found = False
                    for info in zf.infolist():
                        if info.filename.replace("\\", "/").endswith("/" + name):
                            with zf.open(info) as src, open(dest, "wb") as out:
                                out.write(src.read())
                            found = True
                            break
                    if not found:
                        raise AppError(f"scrcpy 压缩包中缺少 {name}")
        except AppError:
            raise
        except Exception as e:
            raise AppError(f"解压 scrcpy 包失败: {e}")

    return base / "scrcpy-server"


# --------------------------------------------------------------------------- #
# socket helpers
# --------------------------------------------------------------------------- #
def _read_exact(sock: socket.socket, n: int, stop: threading.Event) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        if stop.is_set():
            raise OSError("已停止")
        try:
            chunk = sock.recv(n - len(buf))
        except socket.timeout:
            continue
        if not chunk:
            raise OSError("连接已断开")
        buf += chunk
    return bytes(buf)


def _connect_retry(port: int, stop: threading.Event) -> socket.socket:
    deadline = time.time() + 12
    while time.time() < deadline:
        if stop.is_set():
            raise OSError("已停止")
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=2)
        except OSError:
            time.sleep(0.05)
    raise OSError("无法连接到 scrcpy 隧道端口")


def _accept(listener: socket.socket, stop: threading.Event) -> socket.socket:
    listener.settimeout(0.25)
    deadline = time.time() + 12
    while time.time() < deadline:
        if stop.is_set():
            raise OSError("已停止")
        try:
            sock, _ = listener.accept()
            return sock
        except socket.timeout:
            continue
        except OSError:
            time.sleep(0.05)
    raise OSError("等待 scrcpy 连接超时")


def _cleanup_stale_tunnels(serial: str) -> None:
    """Remove leftover `scrcpy_*` adb reverse/forward rules.

    Crashed sessions can leave stale tunnel rules that interfere with new
    sessions (symptom: the server never connects). Best-effort scan of the
    `adb reverse --list` / `adb forward --list` output.
    """
    try:
        out = _adb(serial, ["reverse", "--list"])
        for line in out.stdout.decode("utf-8", "replace").splitlines():
            for p in line.split():
                if p.startswith("localabstract:scrcpy_"):
                    _adb_ok(serial, ["reverse", "--remove", p])
                    break
        out = _adb(serial, ["forward", "--list"])
        for line in out.stdout.decode("utf-8", "replace").splitlines():
            if "scrcpy_" not in line:
                continue
            for p in line.split():
                if p.startswith("tcp:"):
                    _adb_ok(serial, ["forward", "--remove", p])
                    break
    except Exception:
        pass


def _drain(sock: socket.socket, stop: threading.Event) -> None:
    """Discard device -> client messages so the control socket never stalls."""
    while not stop.is_set():
        try:
            data = sock.recv(4096)
            if not data:
                break
        except socket.timeout:
            continue
        except OSError:
            break


# --------------------------------------------------------------------------- #
# Session
# --------------------------------------------------------------------------- #
class ScrcpySession:
    """One live scrcpy session per device, driven from background threads.

    Video frames land in `video_queue` as:
      ("size", w, h)         – resolution/orientation change
      ("media", bytes)       – one Annex-B access unit (config merged)
      ("closed", err|None)   – stream ended; `err` set when abnormal
    """

    def __init__(self) -> None:
        self.video_queue: queue.Queue = queue.Queue(maxsize=VIDEO_QUEUE_MAX)
        self._stop = threading.Event()
        self._lock = threading.Lock()
        # Serialises start/stop so concurrent triggers (page refresh, a
        # second tab, the WS handler's auto-stop) cannot interleave: a
        # parallel start's stale-tunnel sweep could otherwise kill the
        # active session's reverse rule and drop its video stream.
        # RLock so start() -> stop() re-entry works.
        self._lifecycle_lock = threading.RLock()
        self._control: Optional[socket.socket] = None
        self._child: Optional[subprocess.Popen] = None
        self._tunnel: Optional[tuple[str, int, str]] = None  # (kind, port, name)
        self._thread: Optional[threading.Thread] = None
        self.width = 0
        self.height = 0
        self.device_name = ""
        self.serial = ""
        self.error: Optional[str] = None
        self.codec_str: Optional[str] = None
        # Auto-recovery state: the machine's AV intermittently kills the
        # device-side server, dropping the video stream a few seconds in.
        # We transparently restart the session once (kept invisible to the
        # browser: the WS stays open and streams the new session).
        self.recovering = False
        self._user_stopped = False
        self._ws_connected = False  # set by the WS handler while a client is attached
        self._last_max_size = 0
        self._last_bitrate = 8_000_000

    # -- public ----------------------------------------------------------- #
    @property
    def running(self) -> bool:
        return self._thread is not None and self._thread.is_alive()

    def _kill_child(self) -> None:
        if self._child is not None:
            try:
                self._child.kill()
            except Exception:
                pass
            self._child = None

    def start(self, serial: str = "", max_size: int = 0, bitrate: int = 8_000_000) -> dict:
        """Synchronously bring up the session and return the video dimensions.

        Idempotent: any already-running session is stopped first, so a page
        refresh / re-click just restarts cleanly. Retries once on failure:
        this machine's security software intermittently interferes with
        `adb push` / `app_process` spawns, and a retry after cleanup almost
        always succeeds.
        """
        with self._lifecycle_lock:
            if self.running:
                self.stop()
            self._user_stopped = False
            self.recovering = False
            self._last_max_size = max_size
            self._last_bitrate = bitrate
            try:
                return self._start_impl(serial, max_size, bitrate)
            except AppError as e:
                self._kill_child()
                self._cleanup()
                self._stop.set()
                time.sleep(1.0)
                self._stop.clear()
                try:
                    return self._start_impl(serial, max_size, bitrate)
                except AppError as e2:
                    self._kill_child()
                    raise AppError(f"启动 scrcpy 失败（已重试）: {e2}")

    def _start_impl(self, serial: str = "", max_size: int = 0, bitrate: int = 8_000_000) -> dict:
        if self.running:
            raise AppError("已有 scrcpy 会话在运行，请先停止。")
        self._stop.clear()
        self.error = None
        self._tunnel = None
        # Drop stale items left by the previous session — most importantly a
        # leftover ("closed", ...) marker, which would otherwise be consumed
        # by a new WS sender and cause an immediate spurious session stop.
        while True:
            try:
                self.video_queue.get_nowait()
            except queue.Empty:
                break

        server = _find_scrcpy_server()
        serial = _resolve_serial(serial)

        # Clear any stale scrcpy server left over from a crashed session; a
        # leftover app_process would otherwise hold the abstract socket and
        # block this start. Best-effort, ignore failures. The `[c]` trick
        # stops pkill from matching its own (adb shell) command line.
        _adb_ok(serial, ["shell", "pkill", "-f", "[c]om.genymobile.scrcpy"])
        time.sleep(0.3)
        _cleanup_stale_tunnels(serial)

        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(2)
        local_port = listener.getsockname()[1]

        scid = (int(time.time_ns()) & 0xFFFFFFFF) ^ 0x55AA55AA
        sock_name = socket_name(scid)

        # Tunnel: adb reverse preferred, adb forward as fallback.
        tunnel_forward = False
        tunnel_name = f"localabstract:{sock_name}"
        if not _adb_ok(serial, ["reverse", tunnel_name, f"tcp:{local_port}"]):
            if not _adb_ok(
                serial, ["forward", f"tcp:{local_port}", tunnel_name]
            ):
                listener.close()
                raise AppError("无法建立 adb reverse/forward 隧道（设备是否已授权 adb？）")
            tunnel_forward = True
        self._tunnel = ("forward" if tunnel_forward else "reverse", local_port, tunnel_name)

        if not _adb_ok(serial, ["push", str(server), "/data/local/tmp/scrcpy-server.jar"]):
            self._cleanup()
            raise AppError("推送 scrcpy-server 失败")

        args = [
            "shell",
            "CLASSPATH=/data/local/tmp/scrcpy-server.jar",
            "app_process",
            "/",
            "com.genymobile.scrcpy.Server",
            "4.0",
            f"scid={scid:08x}",
            "log_level=info",
            "video=true",
            "audio=false",
            "video_codec=h264",
            "control=true",
            "cleanup=true",
        ]
        if tunnel_forward:
            args.append("tunnel_forward=true")
        if max_size > 0:
            args.append(f"max_size={max_size}")
        if bitrate > 0:
            args.append(f"video_bitrate={bitrate}")

        self._child = _popen(serial, args)

        # The server opens TWO connections: video stream, then control channel.
        try:
            if tunnel_forward:
                video = _connect_retry(local_port, self._stop)
                control = _connect_retry(local_port, self._stop)
                _read_exact(video, 1, self._stop)  # dummy byte (forward only)
            else:
                video = _accept(listener, self._stop)
                control = _accept(listener, self._stop)
        except OSError as e:
            self._kill_child()
            self._cleanup()
            raise AppError(f"建立 scrcpy 连接失败: {e}")
        finally:
            listener.close()

        try:
            devname = _read_exact(video, DEVICE_NAME_LEN, self._stop)
            codec_id = int.from_bytes(_read_exact(video, 4, self._stop), "big")
            if codec_id == 0:
                raise AppError("设备端无法启动视频流（可能被其他 scrcpy 会话占用）")
            if codec_id != CODEC_ID_H264:
                raise AppError(f"设备端视频编码不是 h264 (codec=0x{codec_id:08x})")
            hdr = _read_exact(video, HEADER_LEN, self._stop)
            if not hdr[0] & 0x80:
                raise AppError("协议错误：未收到会话头")
            self.width = int.from_bytes(hdr[4:8], "big")
            self.height = int.from_bytes(hdr[8:12], "big")
        except AppError:
            self._kill_child()
            self._cleanup()
            raise
        except OSError as e:
            self._kill_child()
            self._cleanup()
            raise AppError(f"视频流握手失败: {e}")

        self.device_name = (
            devname.rstrip(b"\0").decode("utf-8", "replace").strip()
        )
        self.serial = serial

        control.settimeout(0.25)
        control.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self._control = control
        threading.Thread(target=_drain, args=(control, self._stop), daemon=True).start()

        self._thread = threading.Thread(target=self._video_loop, args=(video,), daemon=True)
        self._thread.start()

        return {
            "width": self.width,
            "height": self.height,
            "deviceName": self.device_name,
            "serial": serial,
        }

    def stop(self) -> None:
        with self._lifecycle_lock:
            self._user_stopped = True
            self._stop.set()
            self._kill_child()
            # Device-side fallback: make sure the server process is really gone
            # (killing the adb client doesn't always terminate the shell).
            _adb_ok(self.serial, ["shell", "pkill", "-f", "[c]om.genymobile.scrcpy"])
            self._cleanup()
            if self._thread is not None and self._thread.is_alive():
                self._thread.join(timeout=3.0)

    # -- control injection (fire-and-forget) ------------------------------ #
    def _send(self, buf: bytes) -> None:
        with self._lock:
            s = self._control
            if s is None:
                return
            try:
                s.sendall(buf)
            except OSError:
                pass

    def touch(self, action: int, x: int, y: int, pressure: int = PRESSURE_MAX,
              pointer_id: int = 0) -> None:
        """Touch in video-frame pixel coordinates; the server scales it."""
        if self.width <= 0 or self.height <= 0:
            return
        buf = struct.pack(
            ">BBQiiHHHII",
            MSG_INJECT_TOUCH_EVENT,
            action,
            pointer_id,
            x,
            y,
            self.width,
            self.height,
            pressure,
            0,  # actionButton
            0,  # buttons
        )
        self._send(buf)

    def key(self, action: int, keycode: int, meta: int = 0) -> None:
        buf = struct.pack(
            ">BBIII",
            MSG_INJECT_KEYCODE,
            action,
            keycode,
            0,  # repeat
            meta,
        )
        self._send(buf)

    def text(self, s: str) -> None:
        b = s.encode("utf-8")[:300]  # scrcpy 300-byte limit
        self._send(bytes([MSG_INJECT_TEXT]) + struct.pack(">I", len(b)) + b)

    def scroll(self, x: int, y: int, h_units: float = 0.0, v_units: float = 0.0) -> None:
        if self.width <= 0 or self.height <= 0:
            return

        def to_i16(v: float) -> int:
            e = round(v * 2048.0)
            return max(-32768, min(32767, e))

        buf = struct.pack(
            ">BiiHHhhI",
            MSG_INJECT_SCROLL_EVENT,
            x,
            y,
            self.width,
            self.height,
            to_i16(h_units),
            to_i16(v_units),
            0,  # buttons
        )
        self._send(buf)

    # -- internals -------------------------------------------------------- #
    def _video_loop(self, sock: socket.socket) -> None:
        sock.settimeout(0.25)
        pending_config: Optional[bytes] = None
        try:
            while not self._stop.is_set():
                try:
                    hdr = _read_exact(sock, HEADER_LEN, self._stop)
                except socket.timeout:
                    continue
                except OSError as e:
                    print(f"[scrcpy] 视频循环退出: {e}", flush=True)
                    self.error = str(e)
                    break

                if hdr[0] & 0x80:
                    # Size / orientation change.
                    w = int.from_bytes(hdr[4:8], "big")
                    h = int.from_bytes(hdr[8:12], "big")
                    if (w, h) != (self.width, self.height):
                        self.width, self.height = w, h
                        self._put(("size", w, h))
                    continue

                pts_flags = int.from_bytes(hdr[0:8], "big")
                length = int.from_bytes(hdr[8:12], "big")
                if length == 0 or length > MAX_PACKET:
                    continue
                data = _read_exact(sock, length, self._stop)

                # Config packets (SPS/PPS) must be merged into the next packet.
                if pts_flags & FLAG_CONFIG:
                    pending_config = data
                    if self.codec_str is None:
                        self.codec_str = h264_codec_string(data)
                    continue
                payload = (pending_config or b"") + data
                pending_config = None
                self._put(("media", payload))
        except Exception as e:
            print(f"[scrcpy] 视频循环异常: {e!r}", flush=True)
            self.error = str(e)
        finally:
            self._stop.set()
            self._cleanup()
            # Transparent auto-recovery (once): abnormal drops (AV killing the
            # device server, flaky tunnel) restart the session so the browser
            # keeps streaming with no user action. Only when a browser client
            # is attached — without one, a background retry just thrashes the
            # device/adb state. A user-initiated stop or a previous failed
            # recovery does not retry again.
            # NOTE: recovering must be set BEFORE emitting "closed" so the
            # WS sender sees it and keeps the socket open.
            if (
                self.error
                and self._ws_connected
                and not self._user_stopped
                and not self.recovering
            ):
                self.recovering = True
                threading.Timer(1.5, self._auto_recover).start()
            self._put(("closed", self.error))

    def _auto_recover(self) -> None:
        """Restart the session after an abnormal drop; keep the queue flowing."""
        if self._user_stopped:
            self.recovering = False
            return
        with self._lifecycle_lock:
            if self._user_stopped or self.running:
                self.recovering = False
                return
            try:
                self._stop.clear()
                info = self._start_impl(self.serial, self._last_max_size, self._last_bitrate)
                self.recovering = False
                self.error = None
                print(
                    f"[scrcpy] 自动重连成功: {info['width']}x{info['height']}",
                    flush=True,
                )
            except Exception as e:
                self.recovering = False
                print(f"[scrcpy] 自动重连失败: {e}", flush=True)

    def _put(self, item: tuple) -> None:
        """Push a video item, dropping the oldest frame when full."""
        try:
            self.video_queue.put_nowait(item)
        except queue.Full:
            try:
                self.video_queue.get_nowait()
            except queue.Empty:
                pass
            try:
                self.video_queue.put_nowait(item)
            except queue.Full:
                pass

    def _cleanup(self) -> None:
        if self._tunnel is not None:
            kind, port, name = self._tunnel
            if kind == "forward":
                _adb_ok(self.serial, ["forward", "--remove", f"tcp:{port}"])
            else:
                _adb_ok(self.serial, ["reverse", "--remove", name])
            self._tunnel = None
        with self._lock:
            if self._control is not None:
                try:
                    self._control.close()
                except OSError:
                    pass
                self._control = None
