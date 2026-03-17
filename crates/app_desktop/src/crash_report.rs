use directories::ProjectDirs;
use serde_json::json;
use std::backtrace::Backtrace;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct CrashConfig {
    crash_dir: PathBuf,
    log_path: Option<PathBuf>,
}

static CRASH_CONFIG: OnceLock<CrashConfig> = OnceLock::new();

pub fn install(log_path: Option<PathBuf>) -> Option<PathBuf> {
    let crash_dir = crash_dir()?;
    if fs::create_dir_all(&crash_dir).is_err() {
        return None;
    }

    let _ = CRASH_CONFIG.set(CrashConfig {
        crash_dir: crash_dir.clone(),
        log_path,
    });

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(report_path) = write_crash_bundle(info) {
            eprintln!("Voxlink wrote a crash report to {}", report_path.display());
        }
        previous_hook(info);
    }));

    Some(crash_dir)
}

fn crash_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "voxlink", "Voxlink").map(|dirs| dirs.data_dir().join("crashes"))
}

fn write_crash_bundle(info: &std::panic::PanicHookInfo<'_>) -> Option<PathBuf> {
    let config = CRASH_CONFIG.get()?;
    let timestamp = unix_timestamp_secs();
    let stem = format!("voxlink-crash-{timestamp}-{}", std::process::id());
    let report_path = config.crash_dir.join(format!("{stem}.json"));
    let log_snapshot_path = config.crash_dir.join(format!("{stem}.log"));
    let latest_path = config.crash_dir.join("latest-crash.txt");

    let panic_message = panic_message(info);
    let location = info
        .location()
        .map(|location| {
            json!({
                "file": location.file(),
                "line": location.line(),
                "column": location.column(),
            })
        })
        .unwrap_or_else(|| json!(null));

    let log_snapshot = config
        .log_path
        .as_ref()
        .filter(|path| path.exists())
        .map(|path| {
            let _ = fs::copy(path, &log_snapshot_path);
            log_snapshot_path.clone()
        });

    let payload = json!({
        "app": "Voxlink",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "thread": std::thread::current().name().unwrap_or("unnamed"),
        "timestamp_unix": timestamp,
        "panic_message": panic_message,
        "location": location,
        "backtrace": Backtrace::force_capture().to_string(),
        "exe": std::env::current_exe().ok().map(|path| path.display().to_string()),
        "automation": {
            "scenario": std::env::var("VOXLINK_AUTOMATION_SCENARIO").ok(),
            "role": std::env::var("VOXLINK_AUTOMATION_ROLE").ok(),
        },
        "log_path": config.log_path.as_ref().map(|path| display_path(path)),
        "log_snapshot_path": log_snapshot.as_ref().map(|path| display_path(path)),
    });

    let encoded = serde_json::to_vec_pretty(&payload).ok()?;
    fs::write(&report_path, encoded).ok()?;
    let _ = fs::write(&latest_path, report_path.display().to_string());
    Some(report_path)
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
