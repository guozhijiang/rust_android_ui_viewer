//! Headless smoke test for the scrcpy live + control channel on a real device.
//!
//! Drives the real `live::start` session (no GUI) and asserts that:
//!   1. the session connects and the control channel is established
//!      (`LiveEvent::Connected { control: Some(..) }`);
//!   2. touch / key / text can be injected over the control channel without
//!      tearing down the video stream (frames keep arriving afterwards).
//!
//! Usage: `cargo run --example smoke`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use android_ui_viewer::live::{self, LiveEvent};

fn main() {
    let adb = "adb".to_string();
    let serial_hint = String::new(); // auto-detect the single connected device
    let scrcpy_dir = "D:\\scrcpy-win64-v4.0".to_string();
    let max_video_size: u32 = 1024;

    let (tx, rx) = mpsc::channel::<LiveEvent>();
    let stop = Arc::new(AtomicBool::new(false));

    live::start(
        adb,
        serial_hint,
        scrcpy_dir,
        max_video_size,
        stop.clone(),
        tx,
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut connected = false;
    let mut control_ok = false;
    let mut frames = 0u32;
    let mut injected = false;
    let mut error: Option<String> = None;

    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(LiveEvent::Connected {
                width,
                height,
                device_name,
                control,
                ..
            }) => {
                connected = true;
                println!("[ok] Connected: {device_name} {width}x{height}");
                if let Some(c) = control {
                    control_ok = true;
                    println!("[ok] Control channel established (scrcpy 实时操作可用)");
                    // Inject a benign sequence over the control channel.
                    // Touch coords are integer pixels in the video frame space.
                    let cx = (width / 2) as i32;
                    let cy = (height / 2) as i32;
                    c.touch_down(cx, cy);
                    c.touch_up(cx, cy);
                    c.press_key(4); // KEYCODE_BACK
                    c.text("hello");
                    // Scroll at the center: 2 units down (no horizontal).
                    c.scroll(cx, cy, 0.0, -2.0);
                    println!("[ok] Injected tap + BACK + text \"hello\" + scroll");
                    injected = true;
                } else {
                    println!("[warn] Connected but NO control channel (adb 回退)");
                }
            }
            Ok(LiveEvent::Frame(_)) => {
                frames += 1;
            }
            Ok(LiveEvent::Status(s)) => println!("[status] {s}"),
            Ok(LiveEvent::Error(e)) => {
                eprintln!("[error] {e}");
                error = Some(e);
                break;
            }
            Ok(LiveEvent::Stopped) => {
                println!("[info] session stopped");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

    stop.store(true, Ordering::Relaxed);

    println!("---- summary ----");
    println!("connected      : {connected}");
    println!("control channel: {control_ok}");
    println!("frames received: {frames}");
    println!("injected       : {injected}");
    if let Some(e) = &error {
        println!("error          : {e}");
    }

    if connected && control_ok && injected && frames > 0 {
        println!("RESULT: PASS — 控制通道在真机上建立且未影响视频流");
        std::process::exit(0);
    } else {
        println!("RESULT: FAIL");
        std::process::exit(1);
    }
}
