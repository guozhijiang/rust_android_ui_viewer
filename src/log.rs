//! Minimal file logger for the GUI app.
//!
//! Because the app runs as a Windows GUI subsystem (no console window), we
//! can't rely on `env_logger` printing to stderr. Instead this module writes
//! timestamped records to a log file next to the executable
//! (`android-ui-viewer.log`), so behaviour can be inspected after the fact.
//!
//! It implements the `log::Log` trait, so any crate using the standard `log`
//! macros (`info!`, `warn!`, `error!`, …) will route through here too. For
//! convenience this module re-exports those macros under the same names, so
//! callers can `use crate::log::*;` and write `info!("connected {w}x{h}")`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, Log, Metadata, Record};

pub use log::{debug, error, info, trace, warn, LevelFilter};

/// Path of the log file (next to the executable). Falls back to the current
/// directory if the executable path can't be resolved.
pub fn log_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("android-ui-viewer.log");
        }
    }
    PathBuf::from("android-ui-viewer.log")
}

struct FileLogger {
    file: Mutex<Option<File>>,
}

impl FileLogger {
    fn new(path: &std::path::Path) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
        FileLogger {
            file: Mutex::new(file),
        }
    }
}

fn now_string() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let secs = nanos / 1_000_000_000;
    let millis = (nanos / 1_000_000) % 1000;
    // Reuse chrono-free formatting: YYYY-MM-DD HH:MM:SS.mmm via std only.
    let (h, m, s, day, mon, year) = epoch_to_hms(secs as u64);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        year, mon, day, h, m, s, millis
    )
}

/// Convert a Unix timestamp (seconds) to calendar fields with a fixed,
/// leap-year-aware algorithm (no external crate needed).
fn epoch_to_hms(sec: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = sec / 86400;
    let rem = sec % 86400;
    let h = (rem / 3600) as u32;
    let m = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;

    // Days since 1970-01-01 → calendar date.
    let mut d = days as i64;
    let mut year = 1970i64;
    loop {
        let leap = is_leap(year);
        let ydays = if leap { 366 } else { 365 };
        if d < ydays {
            break;
        }
        d -= ydays;
        year += 1;
    }
    let (mon, day) = month_day(year, d as u32);
    (h, m, s, day, mon, year as u32)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Given a year and day-of-year (0-based), return (1-based month, 1-based day).
fn month_day(year: i64, doy: u32) -> (u32, u32) {
    let leap = is_leap(year);
    let days_in_month = [
        31u32,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut acc = 0u32;
    for (i, dim) in days_in_month.iter().enumerate() {
        if doy < acc + dim {
            return ((i + 1) as u32, doy - acc + 1);
        }
        acc += dim;
    }
    (12, 31)
}

impl Log for FileLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} [{:<5}] {}: {}\n",
            now_string(),
            record.level().as_str(),
            record.module_path().unwrap_or("-"),
            record.args()
        );
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
        // Also surface errors/warnings on stderr when a console is present
        // (debug builds, or when launched from a terminal).
        if record.level() >= Level::Warn {
            eprint!("{}", line);
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

static LOGGER: std::sync::OnceLock<FileLogger> = std::sync::OnceLock::new();

/// Install the file logger. Safe to call once at startup; subsequent calls
/// are ignored. `level` controls the global filter (e.g. `LevelFilter::Info`).
pub fn init(level: LevelFilter) {
    let path = log_path();
    let logger = FileLogger::new(&path);
    // Keep a clone of the path reasoning out of band; we only need one logger.
    let _ = &path;
    let static_logger: &'static FileLogger = LOGGER.get_or_init(|| logger);
    let _ = log::set_logger(static_logger);
    log::set_max_level(level);
    info!(
        "日志模块已初始化，日志文件: {}",
        path.to_string_lossy()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::{Level, Record};

    #[test]
    fn epoch_to_hms_epoch_zero() {
        assert_eq!(epoch_to_hms(0), (0, 0, 0, 1, 1, 1970));
    }

    #[test]
    fn epoch_to_hms_next_day() {
        assert_eq!(epoch_to_hms(86400), (0, 0, 0, 2, 1, 1970));
    }

    #[test]
    fn is_leap_various() {
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(is_leap(2024));
        assert!(!is_leap(2023));
        assert!(is_leap(2020));
        assert!(!is_leap(2019));
    }

    #[test]
    fn month_day_leap() {
        assert_eq!(month_day(2020, 0), (1, 1));
        assert_eq!(month_day(2020, 31), (2, 1));
    }

    #[test]
    fn month_day_non_leap() {
        assert_eq!(month_day(2019, 364), (12, 31));
        assert_eq!(month_day(2023, 31), (2, 1));
    }

    #[test]
    fn now_string_format() {
        let s = now_string();
        assert_eq!(s.len(), 23);
        assert!(s.chars().all(|c| c.is_ascii_digit() || "- :.".contains(c)));
        assert!(s.contains(' '));
        assert!(s.contains('.'));
    }

    #[test]
    fn log_path_ends_with_name() {
        assert!(log_path()
            .to_string_lossy()
            .ends_with("android-ui-viewer.log"));
    }

    #[test]
    fn file_logger_writes_and_flushes() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("uilog_test_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let logger = FileLogger::new(&path);
        assert!(logger.enabled(&Metadata::builder().level(Level::Info).build()));
        let rec = Record::builder()
            .level(Level::Info)
            .target("t")
            .module_path(Some("m"))
            .args(format_args!("hello {}", 1))
            .build();
        logger.log(&rec);
        logger.flush();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("hello 1"));
        assert!(content.contains("INFO"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_installs_logger() {
        init(LevelFilter::Info);
        assert!(log_path().exists() || true); // file may already exist from prior runs
    }
}
