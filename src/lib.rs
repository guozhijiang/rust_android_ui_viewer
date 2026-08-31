//! Library crate for `android-ui-viewer`. Splitting the modules into a lib
//! (alongside `main.rs`) lets integration examples / tests drive the real
//! session logic (e.g. `live::start`) without a GUI.

pub mod adb;
pub mod app;
pub mod live;
pub mod log;
pub mod record;
pub mod scrcpy;
pub mod theme;
pub mod u2;
pub mod ui_tree;
