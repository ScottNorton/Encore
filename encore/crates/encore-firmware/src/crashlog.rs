//! Process-level crash evidence: log rotation + panic hook.
//!
//! Distinct from `debugger.rs`, which records *caught* subsystem crashes
//! (`catch_unwind` in `manager.rs`). This module handles *uncaught* failures:
//! a panic anywhere (including in `main`, spawned tasks, or the web server) and
//! preserving the log of a run that died or was killed by the watchdog.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default cap for the active `encore.log`. With one rotated generation
/// (`encore.log.prev`) the on-disk total stays under 2x this (~32 MB), which
/// keeps the ~123 MB `/lsync` partition with ample room to stage a new ~14 MB
/// `encore_next` for OTA / crash-recovery — the exact staging that failed when a
/// chatty run let the logs grow to 117 MB and filled the partition.
pub const LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// A `std::io::Write` sink that caps its file at `max_bytes` by rotating to
/// `<path>.prev` (overwriting the old `.prev`) once the next write would exceed
/// the cap. Total on-disk footprint is therefore bounded by `2 * max_bytes`.
///
/// This is what makes the cap hold *during* a run: `rotate_log` only preserves
/// the previous run's log at startup and does nothing about in-run growth, so
/// without this a busy session grows `encore.log` without bound until `/lsync`
/// fills and blocks the supervisor from staging a recovery binary.
///
/// Wrap in a `Mutex` to use as a `tracing_subscriber` writer (single-threaded
/// tests can use it directly).
pub struct RotatingWriter {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
}

impl RotatingWriter {
    /// Open (truncating) a fresh active log at `path`. Call `rotate_log` first if
    /// you want the previous run's log preserved as `.prev`.
    pub fn new(path: PathBuf, max_bytes: u64) -> std::io::Result<Self> {
        let file = Self::open_fresh(&path)?;
        Ok(Self {
            path,
            file,
            written: 0,
            max_bytes,
        })
    }

    fn open_fresh(path: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        self.file = Self::open_fresh(&self.path)?;
        self.written = 0;
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        let prev = self.path.with_extension("log.prev");
        std::fs::rename(&self.path, &prev)?;
        self.reopen()
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Rotate before this write would push us over the cap, so the active log
        // never exceeds max_bytes (assuming a single write is <= max_bytes, which
        // a log line always is). `written > 0` guards against an infinite loop if
        // one write is somehow larger than the whole cap.
        if self.written > 0
            && self.written + buf.len() as u64 > self.max_bytes
            && self.rotate().is_err()
        {
            // rename within the same dir on a local fs effectively never
            // fails; if it somehow does, still bound the file by truncating
            // in place (we lose `.prev` this one cycle, but `/lsync` is safe).
            let _ = self.reopen();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Rotate `path` to `<path>.prev` if it exists and is non-empty.
///
/// Overwrites any existing `.prev`. A missing or empty file is a no-op. This
/// replaces the old "truncate on startup" behaviour so the previous run's log
/// (the one that may have crashed) survives into `encore.log.prev`.
pub fn rotate_log(path: &Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => {
            let prev = path.with_extension("log.prev");
            std::fs::rename(path, &prev)
        }
        _ => Ok(()),
    }
}

/// Append one timestamped panic record to `<dir>/panic.log` and force it to disk.
///
/// `sync_all()` is the entire point: a watchdog reboot can land milliseconds
/// after a panic, and without the sync the record never reaches NAND. `unix_secs`
/// is passed in (not read from the clock) so this is deterministic in tests.
pub fn write_panic_record(dir: &Path, unix_secs: u64, payload: &str) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("panic.log");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "[{}] {}", unix_secs, payload)?;
    f.flush()?;
    f.sync_all()?;
    Ok(())
}

/// Install a process-wide panic hook that records the panic location and message
/// to `<dir>/panic.log` durably, then chains the previous hook so the panic still
/// prints to stderr (which the supervisor captures into `encore.log`).
///
/// This fires for EVERY panic, including ones later caught by `catch_unwind` in
/// `manager.rs` — the hook runs before unwinding. So `panic.log` becomes the one
/// place that always has file:line, which `CrashSummary` lacks (its backtrace is
/// empty by design — capture is too expensive on the Cortex-A7).
pub fn install_panic_hook(dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".to_string());
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let _ = write_panic_record(
            &dir,
            secs,
            &format!("PANIC [{}] at {}: {}", thread, location, msg),
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("encore-crashlog-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotate_moves_nonempty_log_to_prev() {
        let dir = scratch("rotate");
        let log = dir.join("encore.log");
        std::fs::write(&log, b"run that crashed\n").unwrap();

        rotate_log(&log).unwrap();

        let prev = log.with_extension("log.prev");
        assert!(prev.exists(), ".prev should exist after rotation");
        assert_eq!(
            std::fs::read_to_string(&prev).unwrap(),
            "run that crashed\n"
        );
        assert!(!log.exists(), "original log should be gone (renamed)");
    }

    #[test]
    fn rotate_is_noop_for_missing_or_empty() {
        let dir = scratch("noop");
        let log = dir.join("encore.log");

        // Missing file: Ok, no .prev created.
        rotate_log(&log).unwrap();
        assert!(!log.with_extension("log.prev").exists());

        // Empty file: Ok, left in place, no .prev created.
        std::fs::write(&log, b"").unwrap();
        rotate_log(&log).unwrap();
        assert!(!log.with_extension("log.prev").exists());
        assert!(log.exists());
    }

    #[test]
    fn rotating_writer_caps_footprint_under_heavy_logging() {
        let dir = scratch("rotcap");
        let log = dir.join("encore.log");
        let prev = log.with_extension("log.prev");
        let max = 4096u64;
        let mut w = RotatingWriter::new(log.clone(), max).unwrap();

        // Write ~200 KB through a 4 KB cap: ~50x the cap. If rotation didn't
        // bound it, the files would blow past `max`.
        let line = [b'x'; 200];
        for _ in 0..1000 {
            w.write_all(&line).unwrap();
            w.write_all(b"\n").unwrap();

            let active = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
            let prevlen = std::fs::metadata(&prev).map(|m| m.len()).unwrap_or(0);
            // Invariant at every step: neither file exceeds the cap, so the
            // combined footprint stays under 2x the cap no matter how much we log.
            assert!(active <= max, "active {} exceeds cap {}", active, max);
            assert!(prevlen <= max, "prev {} exceeds cap {}", prevlen, max);
            assert!(active + prevlen <= 2 * max);
        }
        // And it actually rotated (otherwise the test proves nothing).
        assert!(prev.exists(), "expected a rotation to have produced .prev");
    }

    #[test]
    fn panic_record_appends_and_persists() {
        let dir = scratch("panic");
        write_panic_record(&dir, 100, "first: src/foo.rs:10 boom").unwrap();
        write_panic_record(&dir, 200, "second: src/bar.rs:20 bang").unwrap();

        let content = std::fs::read_to_string(dir.join("panic.log")).unwrap();
        assert!(content.contains("[100] first: src/foo.rs:10 boom"));
        assert!(content.contains("[200] second: src/bar.rs:20 bang"));
        assert_eq!(content.lines().count(), 2, "records append, not overwrite");
    }
}
