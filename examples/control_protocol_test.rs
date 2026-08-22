//! Offline verification of the scrcpy control-channel binary protocol.
//!
//! Spins up a mock scrcpy control server (a plain TCP listener扮演设备端),
//! drives the REAL `LiveControl` over that socket, and asserts the exact
//! bytes it emits match the scrcpy v4.0 wire format. No Android device or
//! adb required — this proves the serialization (touch 32B / keycode 14B /
//! text len+utf8 / scroll 21B) is byte-correct against the official protocol.
//!
//! Usage: `cargo run --example control_protocol_test`

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use android_ui_viewer::live::LiveControl;

// Android KeyEvent meta flags (subset we use).
const META_CTRL_LEFT_ON: u32 = 0x2000;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_be_bytes([b[o], b[o + 1]])
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();

    // Mock "device" side: accept one control connection, read everything.
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match sock.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        buf
    });

    // Client side: the real control channel implementation.
    let client = TcpStream::connect(addr).expect("connect");
    let ctrl = LiveControl::from_stream(client, 0);
    ctrl.set_size(1080, 2400);

    // A realistic gesture + key + text sequence. Coordinates are integer
    // pixels in the video frame space (scrcpy v4.0 uses int, not float).
    ctrl.touch_down(540, 1200);
    ctrl.touch_move(600, 1300);
    ctrl.touch_up(540, 1200);
    ctrl.press_key(4); // KEYCODE_BACK
    ctrl.key_down_meta(67, META_CTRL_LEFT_ON); // Ctrl+C
    ctrl.key_up_meta(67, META_CTRL_LEFT_ON);
    ctrl.text("hi");
    // Scroll at (540,1200): 2 units down (v negative = up in scrcpy, so -2.0
    // means scroll down on the device), no horizontal component.
    ctrl.scroll(540, 1200, 0.0, -2.0);

    // Give the OS socket buffer time to flush before the client drops.
    std::thread::sleep(Duration::from_millis(300));

    let b = server.join().expect("server thread");

    let mut failures = Vec::new();
    let mut i = 0usize;

    macro_rules! check {
        ($cond:expr, $msg:expr) => {
            if !($cond) {
                failures.push(format!("offset {}: {}", i, $msg));
            }
        };
    }

    // --- touch down (32B) ---
    check!(b[i] == 2, "touch msg type == 2 (INJECT_TOUCH_EVENT)");
    check!(b[i + 1] == 0, "touch down action == 0");
    check!(u32_at(&b, i + 2) as u64 == 0, "pointerId == 0");
    check!(i32_at(&b, i + 10) == 540, "x == 540 (int pixels)");
    check!(i32_at(&b, i + 14) == 1200, "y == 1200 (int pixels)");
    check!(u16_at(&b, i + 18) == 1080, "screenW == 1080");
    check!(u16_at(&b, i + 20) == 2400, "screenH == 2400");
    check!(u16_at(&b, i + 22) == 0xFFFF, "pressure == 0xFFFF");
    check!(u32_at(&b, i + 24) == 0, "actionButton == 0");
    check!(u32_at(&b, i + 28) == 0, "buttons == 0");
    i += 32;

    // --- touch move (32B) ---
    check!(b[i] == 2, "touch msg type == 2");
    check!(b[i + 1] == 2, "touch move action == 2");
    check!(i32_at(&b, i + 10) == 600, "x == 600 (int pixels)");
    check!(i32_at(&b, i + 14) == 1300, "y == 1300 (int pixels)");
    i += 32;

    // --- touch up (32B, pressure 0) ---
    check!(b[i] == 2, "touch msg type == 2");
    check!(b[i + 1] == 1, "touch up action == 1");
    check!(u16_at(&b, i + 22) == 0, "up pressure == 0");
    i += 32;

    // --- key down BACK (14B) ---
    check!(b[i] == 0, "keycode msg type == 0");
    check!(b[i + 1] == 0, "key down action == 0");
    check!(u32_at(&b, i + 2) == 4, "keycode == 4 (BACK)");
    check!(u32_at(&b, i + 10) == 0, "meta == 0");
    i += 14;
    // --- key up BACK (14B) ---
    check!(b[i] == 0 && b[i + 1] == 1, "key up action == 1");
    check!(u32_at(&b, i + 2) == 4, "keycode == 4 (BACK)");
    i += 14;

    // --- key down Ctrl+C (14B, meta set) ---
    check!(b[i] == 0 && b[i + 1] == 0, "key down action == 0");
    check!(u32_at(&b, i + 2) == 67, "keycode == 67 (C)");
    check!(u32_at(&b, i + 10) == META_CTRL_LEFT_ON, "meta == CTRL_LEFT");
    i += 14;
    // --- key up Ctrl+C (14B) ---
    check!(b[i] == 0 && b[i + 1] == 1, "key up action == 1");
    check!(u32_at(&b, i + 2) == 67, "keycode == 67 (C)");
    check!(u32_at(&b, i + 10) == META_CTRL_LEFT_ON, "meta == CTRL_LEFT");
    i += 14;

    // --- text "hi" (1 + 4 + 2 = 7B) ---
    check!(b[i] == 1, "text msg type == 1 (INJECT_TEXT)");
    check!(u32_at(&b, i + 1) == 2, "text len == 2");
    check!(&b[i + 5..i + 7] == b"hi", "text payload == \"hi\"");
    i += 7;

    // --- scroll (21B) ---
    check!(b[i] == 3, "scroll msg type == 3 (INJECT_SCROLL_EVENT)");
    check!(i32_at(&b, i + 1) == 540, "scroll x == 540");
    check!(i32_at(&b, i + 5) == 1200, "scroll y == 1200");
    check!(u16_at(&b, i + 9) == 1080, "scroll screenW == 1080");
    check!(u16_at(&b, i + 11) == 2400, "scroll screenH == 2400");
    check!(i16_at(&b, i + 13) == 0, "hScroll == 0");
    // v_units -2.0 encoded as round(-2.0 * 2048) = -4096.
    check!(i16_at(&b, i + 15) == -4096, "vScroll == -4096 (-2.0 units)");
    check!(u32_at(&b, i + 17) == 0, "scroll buttons == 0");
    i += 21;

    check!(i == b.len(), format!("total length {} == consumed {}", b.len(), i));

    println!("captured {} bytes", b.len());
    if failures.is_empty() {
        println!("RESULT: PASS — 控制通道协议字节与 scrcpy v4.0 完全一致");
        std::process::exit(0);
    } else {
        for f in &failures {
            eprintln!("[fail] {f}");
        }
        println!("RESULT: FAIL ({} assertion(s))", failures.len());
        std::process::exit(1);
    }
}
