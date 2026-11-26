use std::time::Duration;

pub const MAX_URL_CHARS: usize = 2 * 1024 * 1024;

pub const MAX_URL_DISPLAY_CHARS: usize = 32 * 1024;

pub const SNAPSHOT_TEXT_BYTES: usize = 32 * 1024;

pub const HOST_IDLE_EXIT: Duration = Duration::from_secs(300);

pub const DEVTOOLS_METHOD_TIMEOUT: Duration = Duration::from_secs(30);

pub const EVALUATE_TIMEOUT: Duration = Duration::from_secs(30);

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

pub const MAX_LOG_ENTRIES_PER_TAB: usize = 1000;

pub const NETWORK_IDLE_QUIET: Duration = Duration::from_millis(500);

pub const MAX_PROFILE_NAME_CHARS: usize = 64;

pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
