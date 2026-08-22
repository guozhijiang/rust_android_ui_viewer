//! Probe which FFmpeg symbols fail to resolve via libloading, to diagnose the
//! "av_frame_alloc: GetProcAddress failed" error from the real decoder.

use std::path::Path;

fn main() {
    let dir = Path::new(r"D:\scrcpy-win64-v4.0");
    let avcodec = unsafe { libloading::Library::new(dir.join("avcodec-62.dll")) };
    let avutil = unsafe { libloading::Library::new(dir.join("avutil-60.dll")) };
    match &avcodec {
        Ok(l) => println!("avcodec-62 loaded OK"),
        Err(e) => println!("avcodec-62 load ERR: {e}"),
    }
    match &avutil {
        Ok(l) => println!("avutil-60 loaded OK"),
        Err(e) => println!("avutil-60 load ERR: {e}"),
    }
    if let Ok(avutil) = &avutil {
        for s in [
            "av_malloc",
            "av_free",
            "av_frame_alloc",
            "av_frame_free",
            "av_packet_alloc",
            "av_packet_free",
        ] {
            let r = unsafe { avutil.get::<unsafe extern "C" fn()>(format!("{s}\0").as_bytes()) };
            println!("avutil {s}: {}", if r.is_ok() { "OK" } else { "FAIL" });
        }
    }
    if let Ok(avcodec) = &avcodec {
        for s in [
            "avcodec_find_decoder",
            "avcodec_alloc_context3",
            "avcodec_open2",
            "avcodec_send_packet",
            "avcodec_receive_frame",
            "avcodec_free_context",
            "avcodec_flush_buffers",
        ] {
            let r = unsafe { avcodec.get::<unsafe extern "C" fn()>(format!("{s}\0").as_bytes()) };
            println!("avcodec {s}: {}", if r.is_ok() { "OK" } else { "FAIL" });
        }
    }
}
