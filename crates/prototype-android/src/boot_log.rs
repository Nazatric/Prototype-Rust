//! Boot stage journal + panic crash-reporter (block 103).
//!
//! The user's phone has no adb and no logcat: when the first on-device
//! boot died (block-102 APK), there was NO way to learn why.  The ONE
//! artifact a non-technical user can reach is a file next to their game
//! files — so the boot now writes `boot-log.txt` into the asset folder:
//! every stage with a timestamp, every boot metric line, and (through
//! the panic hook) every panic with its location and the stage that was
//! running.  "Send me boot-log.txt" replaces "please install adb".
//!
//! Everything here is std-only, never panics itself (a failing log must
//! not take the app down), and works on every host — the desktop demo
//! simply never installs the file sink, so `bootlog` degrades to a
//! no-op there and the tests install it against a temp file.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where boot-log lines land (None = file logging off; logcat/stderr
/// still get every line through `log`).
static FILE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The boot stage currently running — the panic report includes it,
/// because "which stage died" is the single most diagnostic line.
static CURRENT_STAGE: Mutex<Option<String>> = Mutex::new(None);

/// Block 107: LIVE boot progress for the menu's loading screen — the
/// worker updates these, the UI thread polls them (lock-free atomics;
/// the loading bar shows real movement instead of a frozen tint).
static PROGRESS: Progress = Progress {
    archives_total: std::sync::atomic::AtomicUsize::new(0),
    archives_done: std::sync::atomic::AtomicUsize::new(0),
    members: std::sync::atomic::AtomicUsize::new(0),
    done: std::sync::atomic::AtomicBool::new(false),
};

struct Progress {
    archives_total: std::sync::atomic::AtomicUsize,
    archives_done: std::sync::atomic::AtomicUsize,
    members: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicBool,
}

/// Worker side: set the archive plan (total count) before indexing.
pub fn progress_plan(archives_total: usize) {
    PROGRESS.archives_total.store(archives_total, std::sync::atomic::Ordering::Relaxed);
}

/// Worker side: one more archive finished indexing.
pub fn progress_archive_done() {
    PROGRESS.archives_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Worker side: N more members indexed.
pub fn progress_members(n: usize) {
    PROGRESS.members.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// Worker side: the boot is DONE (the loading screen can hand over).
pub fn progress_done() {
    PROGRESS.done.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// UI side: the loading fraction 0.0..=1.0 (archive-granular, honest —
/// members count as a secondary signal the drawer can show).
pub fn progress_fraction() -> Option<f32> {
    if PROGRESS.done.load(std::sync::atomic::Ordering::Relaxed) {
        return Some(1.0);
    }
    let total = PROGRESS.archives_total.load(std::sync::atomic::Ordering::Relaxed);
    if total == 0 {
        return None;
    }
    let done = PROGRESS.archives_done.load(std::sync::atomic::Ordering::Relaxed);
    Some(done as f32 / total as f32)
}

/// UI side: the raw progress snapshot for the loading text.
pub fn progress_line() -> String {
    let total = PROGRESS.archives_total.load(std::sync::atomic::Ordering::Relaxed);
    let done = PROGRESS.archives_done.load(std::sync::atomic::Ordering::Relaxed);
    let members = PROGRESS.members.load(std::sync::atomic::Ordering::Relaxed);
    format!("{done}/{total} archives — {members} member(s) indexed")
}

/// UI side: reset between boot attempts (retry path).
pub fn progress_reset() {
    PROGRESS.archives_total.store(0, std::sync::atomic::Ordering::Relaxed);
    PROGRESS.archives_done.store(0, std::sync::atomic::Ordering::Relaxed);
    PROGRESS.members.store(0, std::sync::atomic::Ordering::Relaxed);
    PROGRESS.done.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Millis since the epoch (the boot journal's crude timestamp).
fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Route all later [`bootlog`] lines into `path` (Android: the asset
/// folder's `boot-log.txt`; the folder must exist — `ensure_folder`
/// runs before this in `android_main`).
pub fn install_file_log(path: PathBuf) {
    if let Ok(mut p) = FILE_PATH.lock() {
        *p = Some(path);
    }
    bootlog("boot log installed");
}

/// Record a stage transition (logcat + the journal).  Stages are the
/// unit of blame: the panic hook names the one that was running.
pub fn stage(name: &str) {
    if let Ok(mut s) = CURRENT_STAGE.lock() {
        *s = Some(name.to_string());
    }
    log::info!("boot stage: {name}");
    bootlog(&format!("STAGE {name}"));
}

/// The stage that is running right now (None before the first stage).
pub fn current_stage() -> Option<String> {
    CURRENT_STAGE.lock().ok().and_then(|s| s.clone())
}

/// Append one line to the boot journal (if installed).  Open-append-
/// close per line: a hard crash mid-boot still leaves every earlier
/// line on disk.  Write failures are swallowed BY CONTRACT.
pub fn bootlog(line: &str) {
    let path = match FILE_PATH.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    let Some(path) = path else { return };
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write as _;
    let _ = writeln!(f, "[{}] {line}", timestamp());
}

/// A panic payload → readable text (downcast both common shapes).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Install the global panic hook: every panic lands in the journal
/// (message + location + running stage) BEFORE the default hook runs —
/// the default still prints to stderr/logcat so host behaviour is
/// unchanged.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let stage = current_stage().unwrap_or_else(|| "(none)".into());
        let msg = panic_message(info.payload());
        let loc = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "(unknown location)".into());
        bootlog(&format!("PANIC: {msg} @ {loc} (stage: {stage})"));
        log::error!("PANIC: {msg} @ {loc} (stage: {stage})");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single guard around the GLOBALS these tests mutate: set the file
    /// sink, run the body, then restore None so parallel tests never
    /// see another test's temp file.
    fn with_file_sink(body: impl FnOnce(&PathBuf)) {
        let dir = std::env::temp_dir().join(format!(
            "prototype-boot-log-test-{}-{}",
            std::process::id(),
            timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boot-log.txt");
        install_file_log(path.clone());
        body(&path);
        if let Ok(mut p) = FILE_PATH.lock() {
            *p = None;
        }
        if let Ok(mut s) = CURRENT_STAGE.lock() {
            *s = None;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootlog_appends_timestamped_lines_to_the_file() {
        with_file_sink(|path| {
            bootlog("first line");
            bootlog("second line");
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.contains("first line"));
            assert!(text.contains("second line"));
            assert_eq!(text.lines().count(), 3, "install line + 2 entries");
        });
    }

    #[test]
    fn stage_tracks_the_running_stage_and_journals_it() {
        with_file_sink(|path| {
            stage("cpu: test stage");
            assert_eq!(current_stage().as_deref(), Some("cpu: test stage"));
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.contains("STAGE cpu: test stage"));
        });
    }

    #[test]
    fn panic_hook_writes_message_location_and_stage() {
        with_file_sink(|path| {
            install_panic_hook();
            stage("cpu: panic probe");
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                panic!("probe crash");
            }));
            assert!(outcome.is_err(), "the panic still unwinds");
            let text = std::fs::read_to_string(path).unwrap();
            assert!(
                text.contains("PANIC: probe crash"),
                "the message must land: {text}"
            );
            assert!(text.contains("(stage: cpu: panic probe)"));
        });
    }
}
