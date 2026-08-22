//! H.264 decoding using FFmpeg's `avcodec` loaded dynamically from the DLLs
//! shipped with scrcpy (no C toolchain required). Decoded YUV frames are
//! converted to RGBA in plain Rust.
//!
//! Only opaque FFmpeg handles are used; the two small struct prefixes below
//! expose just the fields we touch (byte offsets are stable across FFmpeg 7/8).

use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use libloading::Library;

const AV_CODEC_ID_H264: u32 = 27;
const AV_PIX_FMT_YUV420P: c_int = 0;
const AV_PIX_FMT_YUVJ420P: c_int = 12;
const AV_PIX_FMT_NV12: c_int = 23;

const AVERROR_EAGAIN: c_int = -11;

/// RGB output for a single decoded frame.
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    /// RGBA, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

// ---- Minimal struct prefixes (offsets match FFmpeg 7/8). ----

/// First fields of `AVPacket`: buf(0) pts(8) dts(16) data(24) size(32) ...
#[repr(C)]
struct AvPacket {
    buf: *mut c_void,
    pts: i64,
    dts: i64,
    data: *mut u8,
    size: c_int,
    stream_index: c_int,
    flags: c_int,
}

/// First fields of `AVFrame`: data[8](0) linesize[8](64) extended_data(96)
/// width(104) height(108) nb_samples(112) format(116) ...
#[repr(C)]
struct AvFrame {
    data: [*mut u8; 8],
    linesize: [c_int; 8],
    extended_data: *mut *mut u8,
    width: c_int,
    height: c_int,
    nb_samples: c_int,
    format: c_int,
    key_frame: c_int,
    pict_type: c_int,
    sample_aspect_ratio: [c_int; 2],
    pts: i64,
}

type AvMalloc = unsafe extern "C" fn(usize) -> *mut c_void;
type AvFree = unsafe extern "C" fn(*mut c_void);
type AvStrerror = unsafe extern "C" fn(c_int, *mut c_char, usize) -> c_int;
type AvPacketAlloc = unsafe extern "C" fn() -> *mut AvPacket;
type AvPacketFree = unsafe extern "C" fn(*mut *mut AvPacket);
type AvPacketFromData = unsafe extern "C" fn(*mut AvPacket, *mut u8, c_int) -> c_int;
type AvPacketUnref = unsafe extern "C" fn(*mut AvPacket);
type AvcodecFindDecoder = unsafe extern "C" fn(u32) -> *mut c_void;
type AvcodecAllocContext = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type AvcodecOpen2 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int;
type AvcodecSendPacket = unsafe extern "C" fn(*mut c_void, *const AvPacket) -> c_int;
type AvcodecReceiveFrame = unsafe extern "C" fn(*mut c_void, *mut AvFrame) -> c_int;
type AvcodecFreeContext = unsafe extern "C" fn(*mut *mut c_void);
type AvcodecFlushBuffers = unsafe extern "C" fn(*mut c_void);
type AvFrameAlloc = unsafe extern "C" fn() -> *mut AvFrame;
type AvFrameFree = unsafe extern "C" fn(*mut *mut AvFrame);

/// Decoder backed by the dynamically loaded FFmpeg `avcodec` DLL.
pub struct H264Decoder {
    _avcodec: Library,
    _avutil: Library,

    av_malloc: AvMalloc,
    av_free: AvFree,
    packet_from_data: AvPacketFromData,
    packet_unref: AvPacketUnref,
    packet_free: AvPacketFree,
    frame_free: AvFrameFree,
    send: AvcodecSendPacket,
    receive: AvcodecReceiveFrame,
    flush: AvcodecFlushBuffers,
    free_ctx: AvcodecFreeContext,

    codec_ctx: *mut c_void,
    frame: *mut AvFrame,
    packet: *mut AvPacket,
}

fn last_av_error(avutil: &Library, av_free: &AvFree, code: c_int) -> String {
    unsafe {
        if let Ok(av_strerror) = avutil.get::<AvStrerror>(b"av_strerror\0") {
            let mut buf = [0u8; 256];
            av_strerror(code, buf.as_mut_ptr() as *mut c_char, buf.len());
            let s = String::from_utf8_lossy(&buf);
            let trimmed = s.trim_end_matches('\0');
            if !trimmed.is_empty() {
                return format!("{code} ({trimmed})");
            }
        }
        let _ = av_free;
        format!("{code}")
    }
}

impl H264Decoder {
    /// Load avcodec/avutil from `dll_dir` and prepare a software H.264 decoder.
    pub fn try_new(dll_dir: &Path) -> Result<Self> {
        // SAFETY: the libraries are kept alive for the entire decoder lifetime.
        let (avcodec, avutil);
        unsafe {
            avcodec = Library::new(dll_dir.join("avcodec-62.dll"))
                .map_err(|e| anyhow!("加载 avcodec-62.dll 失败: {e}"))?;
            avutil = Library::new(dll_dir.join("avutil-60.dll"))
                .map_err(|e| anyhow!("加载 avutil-60.dll 失败: {e}"))?;
        }

        let av_malloc: AvMalloc = unsafe { *avutil.get(b"av_malloc\0").map_err(|e| anyhow!("av_malloc: {e}"))? };
        let av_free: AvFree = unsafe { *avutil.get(b"av_free\0").map_err(|e| anyhow!("av_free: {e}"))? };

        let find_decoder: AvcodecFindDecoder =
            unsafe { *avcodec.get(b"avcodec_find_decoder\0").map_err(|e| anyhow!("avcodec_find_decoder: {e}"))? };
        let alloc_ctx: AvcodecAllocContext =
            unsafe { *avcodec.get(b"avcodec_alloc_context3\0").map_err(|e| anyhow!("avcodec_alloc_context3: {e}"))? };
        let open2: AvcodecOpen2 =
            unsafe { *avcodec.get(b"avcodec_open2\0").map_err(|e| anyhow!("avcodec_open2: {e}"))? };
        let send: AvcodecSendPacket =
            unsafe { *avcodec.get(b"avcodec_send_packet\0").map_err(|e| anyhow!("avcodec_send_packet: {e}"))? };
        let receive: AvcodecReceiveFrame =
            unsafe { *avcodec.get(b"avcodec_receive_frame\0").map_err(|e| anyhow!("avcodec_receive_frame: {e}"))? };
        let free_ctx: AvcodecFreeContext =
            unsafe { *avcodec.get(b"avcodec_free_context\0").map_err(|e| anyhow!("avcodec_free_context: {e}"))? };
        let flush: AvcodecFlushBuffers =
            unsafe { *avcodec.get(b"avcodec_flush_buffers\0").map_err(|e| anyhow!("avcodec_flush_buffers: {e}"))? };
        let frame_alloc: AvFrameAlloc =
            unsafe { *avutil.get(b"av_frame_alloc\0").map_err(|e| anyhow!("av_frame_alloc: {e}"))? };
        let frame_free: AvFrameFree =
            unsafe { *avutil.get(b"av_frame_free\0").map_err(|e| anyhow!("av_frame_free: {e}"))? };
        let packet_alloc: AvPacketAlloc =
            unsafe { *avcodec.get(b"av_packet_alloc\0").map_err(|e| anyhow!("av_packet_alloc: {e}"))? };
        let packet_free: AvPacketFree =
            unsafe { *avcodec.get(b"av_packet_free\0").map_err(|e| anyhow!("av_packet_free: {e}"))? };
        let packet_from_data: AvPacketFromData =
            unsafe { *avcodec.get(b"av_packet_from_data\0").map_err(|e| anyhow!("av_packet_from_data: {e}"))? };
        let packet_unref: AvPacketUnref =
            unsafe { *avcodec.get(b"av_packet_unref\0").map_err(|e| anyhow!("av_packet_unref: {e}"))? };

        let codec = unsafe { find_decoder(AV_CODEC_ID_H264) };
        if codec.is_null() {
            bail!("设备端/本机 avcodec 不含 H.264 解码器");
        }
        let mut codec_ctx = unsafe { alloc_ctx(codec) };
        if codec_ctx.is_null() {
            bail!("avcodec_alloc_context3 失败");
        }
        let mut frame = unsafe { frame_alloc() };
        let mut packet = unsafe { packet_alloc() };

        let ret = unsafe { open2(codec_ctx, codec, std::ptr::null_mut()) };
        if ret < 0 {
            let _ = last_av_error(&avutil, &av_free, ret);
            unsafe {
                if !frame.is_null() {
                    frame_free(&mut frame);
                }
                if !packet.is_null() {
                    packet_free(&mut packet);
                }
                free_ctx(&mut codec_ctx);
            }
            bail!("avcodec_open2 失败 ({})", last_av_error(&avutil, &av_free, ret));
        }

        Ok(Self {
            _avcodec: avcodec,
            _avutil: avutil,
            av_malloc,
            av_free,
            packet_from_data,
            packet_unref,
            packet_free,
            frame_free,
            send,
            receive,
            flush,
            free_ctx,
            codec_ctx,
            frame,
            packet,
        })
    }

    /// Feed one H.264 access unit (Annex-B, may include SPS/PPS). Returns the
    /// first decoded frame produced, if any.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<RgbaFrame>> {
        if data.is_empty() {
            return Ok(None);
        }
        unsafe {
            // Allocate refcounted buffer and attach it to the packet: the
            // decoder keeps a reference, so the data stays alive until used.
            let ptr = (self.av_malloc)(data.len());
            if ptr.is_null() {
                bail!("av_malloc 失败");
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            let ret = (self.packet_from_data)(self.packet, ptr as *mut u8, data.len() as c_int);
            if ret < 0 {
                (self.av_free)(ptr);
                bail!("av_packet_from_data 失败 ({ret})");
            }
            let ret = (self.send)(self.codec_ctx, self.packet);
            // We are done with our reference; the decoder keeps its own.
            (self.packet_unref)(self.packet);
            if ret < 0 && ret != AVERROR_EAGAIN {
                bail!("avcodec_send_packet 失败 ({ret})");
            }
        }

        let mut out = None;
        loop {
            let ret = unsafe { (self.receive)(self.codec_ctx, self.frame) };
            if ret == AVERROR_EAGAIN || ret < 0 {
                break;
            }
            let frame = unsafe { &*self.frame };
            let (w, h) = (frame.width, frame.height);
            if w <= 0 || h <= 0 {
                continue;
            }
            let conv = match frame.format {
                AV_PIX_FMT_YUV420P => Some(
                    yuv420p_to_rgba(
                        frame.data[0],
                        frame.data[1],
                        frame.data[2],
                        frame.linesize[0],
                        frame.linesize[1],
                        frame.linesize[2],
                        w,
                        h,
                    ),
                ),
                AV_PIX_FMT_YUVJ420P => Some(
                    yuv420p_to_rgba_full(
                        frame.data[0],
                        frame.data[1],
                        frame.data[2],
                        frame.linesize[0],
                        frame.linesize[1],
                        frame.linesize[2],
                        w,
                        h,
                    ),
                ),
                AV_PIX_FMT_NV12 => Some(nv12_to_rgba(
                    frame.data[0],
                    frame.data[1],
                    frame.linesize[0],
                    frame.linesize[1],
                    w,
                    h,
                )),
                other => {
                    bail!("不支持的像素格式: {other}");
                }
            };
            out = conv;
            break;
        }
        Ok(out)
    }

    /// Discard stale decoded frames after a resolution/layout change.
    pub fn flush(&self) {
        unsafe { (self.flush)(self.codec_ctx) };
    }
}

impl Drop for H264Decoder {
    fn drop(&mut self) {
        unsafe {
            (self.frame_free)(&mut self.frame);
            (self.packet_free)(&mut self.packet);
            (self.free_ctx)(&mut self.codec_ctx);
        }
    }
}

// ---- YUV -> RGBA conversion (integer math, BT.601) ----

#[inline]
fn clamp255(v: i32) -> u8 {
    if v < 0 {
        0
    } else if v > 255 {
        255
    } else {
        v as u8
    }
}

#[inline]
fn yuv_limited_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = clamp255((298 * c + 409 * e + 128) >> 8);
    let g = clamp255((298 * c - 100 * d - 208 * e + 128) >> 8);
    let b = clamp255((298 * c + 516 * d + 128) >> 8);
    (r, g, b)
}

#[inline]
fn yuv_full_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = clamp255(y as i32 + ((410 * e) >> 8));
    let g = clamp255(y as i32 - ((100 * d) >> 8) - ((208 * e) >> 8));
    let b = clamp255(y as i32 + ((517 * d) >> 8));
    (r, g, b)
}

fn yuv420p_to_rgba(
    y: *const u8,
    u: *const u8,
    v: *const u8,
    ly: c_int,
    lu: c_int,
    lv: c_int,
    w: i32,
    h: i32,
) -> RgbaFrame {
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    if w == 0 || h == 0 {
        return RgbaFrame {
            width: w as u32,
            height: h as u32,
            rgba,
        };
    }
    let (ly, lu, lv) = (ly as usize, lu as usize, lv as usize);
    for yy in 0..h {
        let row = &mut rgba[yy * w * 4..(yy + 1) * w * 4];
        let (uvy, uvy2) = ((yy / 2) * lu, (yy / 2) * lv);
        for x in 0..w {
            // SAFETY: the FFmpeg frame guarantees these pointer/linesize values
            // for a decoded YUV420P frame.
            let (yv, uv, vv) = unsafe {
                (
                    *y.add(yy * ly + x),
                    *u.add(uvy + x / 2),
                    *v.add(uvy2 + x / 2),
                )
            };
            let (r, g, b) = yuv_limited_to_rgb(yv, uv, vv);
            row[x * 4] = r;
            row[x * 4 + 1] = g;
            row[x * 4 + 2] = b;
            row[x * 4 + 3] = 255;
        }
    }
    RgbaFrame {
        width: w as u32,
        height: h as u32,
        rgba,
    }
}

fn yuv420p_to_rgba_full(
    y: *const u8,
    u: *const u8,
    v: *const u8,
    ly: c_int,
    lu: c_int,
    lv: c_int,
    w: i32,
    h: i32,
) -> RgbaFrame {
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    let (ly, lu, lv) = (ly as usize, lu as usize, lv as usize);
    for yy in 0..h {
        let row = &mut rgba[yy * w * 4..(yy + 1) * w * 4];
        let (uvy, uvy2) = ((yy / 2) * lu, (yy / 2) * lv);
        for x in 0..w {
            let (yv, uv, vv) = unsafe {
                (
                    *y.add(yy * ly + x),
                    *u.add(uvy + x / 2),
                    *v.add(uvy2 + x / 2),
                )
            };
            let (r, g, b) = yuv_full_to_rgb(yv, uv, vv);
            row[x * 4] = r;
            row[x * 4 + 1] = g;
            row[x * 4 + 2] = b;
            row[x * 4 + 3] = 255;
        }
    }
    RgbaFrame {
        width: w as u32,
        height: h as u32,
        rgba,
    }
}

fn nv12_to_rgba(
    y: *const u8,
    uv: *const u8,
    ly: c_int,
    luv: c_int,
    w: i32,
    h: i32,
) -> RgbaFrame {
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    let (ly, luv) = (ly as usize, luv as usize);
    for yy in 0..h {
        let row = &mut rgba[yy * w * 4..(yy + 1) * w * 4];
        let uvrow = (yy / 2) * luv;
        for x in 0..w {
            let yv = unsafe { *y.add(yy * ly + x) };
            let (uvv, vvv) = unsafe {
                let p = uv.add(uvrow + (x / 2) * 2);
                (*p, *p.add(1))
            };
            let (r, g, b) = yuv_limited_to_rgb(yv, uvv, vvv);
            row[x * 4] = r;
            row[x * 4 + 1] = g;
            row[x * 4 + 2] = b;
            row[x * 4 + 3] = 255;
        }
    }
    RgbaFrame {
        width: w as u32,
        height: h as u32,
        rgba,
    }
}