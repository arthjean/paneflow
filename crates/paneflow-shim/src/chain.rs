use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CHAIN_ENV: &str = "PANEFLOW_SHIM_CHAIN";
pub(crate) const CHAIN_TARGETS_ENV: &str = "PANEFLOW_SHIM_CHAIN_TARGETS";
pub(crate) const MAX_DEPTH: u32 = 8;
pub(crate) const RELAUNCH_WINDOW_MS: u64 = 10_000;

const FIELD_COUNT: usize = 6;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chain {
    pub(crate) tool: String,
    pub(crate) session: String,
    pub(crate) depth: u32,
    pub(crate) launches: u32,
    pub(crate) launched_ms: u64,
    pub(crate) arguments: u64,
    pub(crate) targets: Vec<PathBuf>,
}

pub(crate) struct Launch<'a> {
    pub(crate) tool: &'a str,
    pub(crate) session: &'a str,
    pub(crate) target: &'a Path,
    pub(crate) arguments: &'a [OsString],
    pub(crate) now_ms: u64,
    pub(crate) sent_back: bool,
}

impl Chain {
    pub(crate) fn from_env(
        chain: Option<&OsStr>,
        targets: Option<&OsStr>,
        last_target: Option<&OsStr>,
        session: &str,
    ) -> Option<Self> {
        let mut fields = chain?.to_str()?.splitn(FIELD_COUNT, ':');
        let depth = fields.next()?.parse().ok()?;
        let launches = fields.next()?.parse().ok()?;
        let launched_ms = fields.next()?.parse().ok()?;
        let arguments = u64::from_str_radix(fields.next()?, 16).ok()?;
        let chain_session = fields.next()?;
        let tool = fields.next()?.to_owned();
        if chain_session != session {
            return None;
        }
        let mut targets: Vec<PathBuf> = targets
            .map(|list| env::split_paths(list).collect())
            .unwrap_or_default();
        if let Some(last_target) = last_target.map(PathBuf::from) {
            if !targets.contains(&last_target) {
                targets.push(last_target);
            }
        }
        Some(Self {
            tool,
            session: chain_session.to_owned(),
            depth,
            launches,
            launched_ms,
            arguments,
            targets,
        })
    }

    pub(crate) fn extended(inherited: Option<&Self>, launch: &Launch<'_>) -> Self {
        let mut targets = inherited
            .filter(|chain| chain.tool == launch.tool)
            .map(|chain| chain.targets.clone())
            .unwrap_or_default();
        if !targets.iter().any(|known| known == launch.target) {
            targets.push(launch.target.to_path_buf());
        }
        Self {
            tool: launch.tool.to_owned(),
            session: launch.session.to_owned(),
            depth: inherited.map_or(0, |chain| chain.depth).saturating_add(1),
            launches: inherited
                .map_or(0, |chain| chain.launches)
                .saturating_add(u32::from(!launch.sent_back)),
            launched_ms: launch.now_ms,
            arguments: fingerprint(launch.arguments),
            targets,
        }
    }

    pub(crate) fn is_too_deep(&self) -> bool {
        self.depth >= MAX_DEPTH
    }

    pub(crate) fn was_sent_back(&self, tool: &str, arguments: &[OsString], now_ms: u64) -> bool {
        self.tool == tool
            && self.arguments == fingerprint(arguments)
            && now_ms.saturating_sub(self.launched_ms) <= RELAUNCH_WINDOW_MS
    }

    pub(crate) fn launcher(&self) -> Option<&Path> {
        self.targets.last().map(PathBuf::as_path)
    }

    pub(crate) fn env_value(&self) -> String {
        format!(
            "{}:{}:{}:{:016x}:{}:{}",
            self.depth, self.launches, self.launched_ms, self.arguments, self.session, self.tool
        )
    }

    pub(crate) fn targets_env_value(&self) -> Option<OsString> {
        env::join_paths(&self.targets).ok()
    }
}

pub(crate) fn agents_above(inherited: Option<&Chain>, reentered: bool) -> u32 {
    inherited
        .map_or(0, |chain| chain.launches)
        .saturating_sub(u32::from(reentered))
}

pub(crate) fn fingerprint(arguments: &[OsString]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for argument in arguments {
        let bytes = argument.as_encoded_bytes();
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        for byte in length.to_le_bytes().iter().chain(bytes) {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
