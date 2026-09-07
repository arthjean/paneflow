use std::io::Write;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};

static WRITER: OnceLock<Option<SyncSender<serde_json::Value>>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(super) fn enabled() -> bool {
    WRITER
        .get_or_init(|| {
            let path = std::env::var_os("PANEFLOW_BROWSER_BENCH")?;
            let (sender, receiver) = sync_channel::<serde_json::Value>(4096);
            std::thread::Builder::new()
                .name("browser-benchmark".into())
                .spawn(move || {
                    let Ok(mut file) = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(path)
                    else {
                        eprintln!("browser benchmark: cannot create recording file");
                        return;
                    };
                    for value in receiver {
                        if serde_json::to_writer(&mut file, &value).is_err()
                            || file.write_all(b"\n").is_err()
                        {
                            eprintln!("browser benchmark: recording failed");
                            break;
                        }
                    }
                })
                .ok()?;
            Some(sender)
        })
        .is_some()
}

pub(super) fn now_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } == 0 {
            return (time.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(time.tv_nsec as u64);
        }
    }
    0
}

pub(super) fn record(page: &str, event: &str, fields: serde_json::Value) {
    if !enabled() {
        return;
    }
    if let Some(Some(sender)) = WRITER.get() {
        let value = serde_json::json!({"schema": 1, "at_ns": now_ns(), "page": page, "event": event, "fields": fields, "dropped": DROPPED.load(Ordering::Relaxed)});
        if sender.try_send(value).is_err() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}
