#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_git::feed::{FeedOptions, Feeds, Loss, IDLE_EVICTION};
use support::committed_repo;

const RELEASE_DEADLINE: Duration = Duration::from_secs(30);

const BASELINE_SETTLE: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
struct Resources {
    handles: usize,
    threads: usize,
    watches: usize,
}

#[cfg(target_os = "linux")]
fn resources() -> Resources {
    let count = |dir: &str| {
        std::fs::read_dir(dir)
            .map(|entries| entries.count())
            .unwrap_or(0)
    };
    Resources {
        handles: count("/proc/self/fd"),
        threads: count("/proc/self/task"),
        watches: paneflow_git::feed::inotify_watches_in_use(),
    }
}

#[cfg(target_os = "macos")]
fn resources() -> Resources {
    Resources {
        handles: std::fs::read_dir("/dev/fd")
            .map(|entries| entries.count())
            .unwrap_or(0),
        threads: 0,
        watches: 0,
    }
}

#[cfg(windows)]
fn resources() -> Resources {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut handles = 0u32;
    let counted = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) };
    assert_ne!(counted, 0, "GetProcessHandleCount");
    Resources {
        handles: handles as usize,
        threads: 0,
        watches: 0,
    }
}

impl Resources {
    fn within(&self, limit: &Resources) -> bool {
        self.handles <= limit.handles
            && self.threads <= limit.threads
            && self.watches <= limit.watches
    }
}

fn released_down_to(limit: &Resources, patience: Duration) -> Resources {
    let deadline = Instant::now() + patience;
    loop {
        let now = resources();
        if now.within(limit) || Instant::now() >= deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn repository(parent: &Path) -> PathBuf {
    let root = parent.join("repo");
    committed_repo(&root, &[("a/f.txt", "f\n"), ("b/g.txt", "g\n")]);
    std::fs::canonicalize(root).unwrap()
}

#[test]
fn an_idle_feed_is_evicted_and_releases_its_watches_and_handles() {
    support::isolate_process();
    let tmp = tempfile::tempdir().unwrap();
    let root = repository(tmp.path());
    let options = FeedOptions {
        cache_dir: Some(tmp.path().join("cache")),
        ..FeedOptions::default()
    };
    let mut feeds = Feeds::new(options.clone());
    let before_any_feed = resources();
    drop(
        Feeds::new(options)
            .acquire(&root, Instant::now())
            .map(|_| ()),
    );
    let baseline = released_down_to(&before_any_feed, BASELINE_SETTLE);
    let t0 = Instant::now();
    let feed = feeds.acquire(&root, t0).expect("feed");
    assert_eq!(feed.drain().loss, Some(Loss::WatchSetChanged));
    assert_ne!(resources(), baseline, "a live feed holds native resources");
    let probe = t0 + IDLE_EVICTION - Duration::from_secs(1);
    assert_eq!(
        feeds.evict_idle(probe),
        0,
        "a feed younger than the idle window stays"
    );
    feeds.acquire(&root, probe).expect("feed");
    assert_eq!(
        feeds.evict_idle(t0 + IDLE_EVICTION),
        0,
        "a probe restarts the idle window"
    );
    assert_eq!(feeds.evict_idle(probe + IDLE_EVICTION), 1);
    assert!(feeds.is_empty());
    let released = released_down_to(&baseline, RELEASE_DEADLINE);
    assert!(
        released.within(&baseline),
        "eviction released every watch, handle and thread: {released:?} against {baseline:?}"
    );
    let restarted = feeds
        .acquire(&root, probe + IDLE_EVICTION)
        .expect("a new feed");
    assert_eq!(
        restarted.drain().loss,
        Some(Loss::WatchSetChanged),
        "the next probe after an eviction starts a new baseline"
    );
    assert_eq!(feeds.starts(), 2);
}
