#[derive(Debug, thiserror::Error)]
pub enum GhosttyError {
    #[error("libghostty is available only on Linux or Windows with the `native` feature")]
    UnsupportedPlatform,
    #[error("terminal dimensions must be within 1..={max}: got {cols}x{rows}")]
    InvalidDimensions { cols: usize, rows: usize, max: u16 },
    #[error("libghostty ABI mismatch: {0}")]
    AbiMismatch(String),
    #[error("libghostty `{operation}` failed with result {code}")]
    Ffi { operation: &'static str, code: i32 },
    #[error("{resource} exceeds the {limit}-unit safety cap")]
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
    #[error("libghostty returned invalid UTF-8 for {0}")]
    InvalidUtf8(&'static str),
    #[error("paste contains control sequences and requires explicit approval")]
    UnsafePaste,
}

pub type Result<T> = std::result::Result<T, GhosttyError>;
