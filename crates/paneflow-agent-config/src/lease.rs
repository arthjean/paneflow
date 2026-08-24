use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Crash-safe lifetime lease for an agent configuration resource.
///
/// Each live session holds a shared OS lock. Cleanup upgrades to an exclusive
/// lock only after the final shared holder exits. The kernel releases locks on
/// process termination, so a killed shim cannot strand a stale lease marker.
pub struct ConfigLease {
    file: Option<File>,
}

pub struct LastConfigLease {
    file: File,
}

impl ConfigLease {
    pub fn acquire(resource: &Path) -> Result<Self> {
        let path = lease_path(resource)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.lock_shared()?;
        Ok(Self { file: Some(file) })
    }

    /// Release this session's shared lock and become the exclusive last owner.
    /// `None` means another live session still owns the resource.
    pub fn try_take_last(&mut self) -> Result<Option<LastConfigLease>> {
        let Some(file) = self.file.take() else {
            return Ok(None);
        };
        file.unlock()?;
        match file.try_lock() {
            Ok(()) => Ok(Some(LastConfigLease { file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Persist that the leased resource was created by PaneFlow.
    ///
    /// Callers serialize this update with their configuration lock. The bit
    /// survives process crashes and is consumed by the eventual last owner.
    pub fn mark_created(&mut self) -> Result<()> {
        let file = self.file.as_mut().ok_or_else(|| {
            Error::new(
                ErrorKind::BrokenPipe,
                "configuration lease was already released",
            )
        })?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&[1])?;
        file.set_len(1)?;
        file.sync_data()
    }
}

impl LastConfigLease {
    /// Consume and clear the durable resource-ownership bit.
    ///
    /// Clearing before cleanup makes a crash conservative: it may leave a
    /// managed file behind, but it cannot later delete a user-created file.
    pub fn take_created(&mut self) -> Result<bool> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut marker = [0];
        let created = self.file.read(&mut marker)? == 1 && marker[0] == 1;
        self.file.set_len(0)?;
        self.file.sync_data()?;
        Ok(created)
    }
}

fn lease_path(resource: &Path) -> Result<PathBuf> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "could not resolve the user configuration directory",
        )
    })?;
    let directory = config_dir.join("paneflow").join("agent-config-leases");
    std::fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("{:016x}.lock", resource_hash(resource))))
}

#[cfg(unix)]
fn resource_hash(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    fnv1a(path.as_os_str().as_bytes().iter().copied())
}

#[cfg(windows)]
fn resource_hash(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    fnv1a(path.as_os_str().encode_wide().flat_map(u16::to_le_bytes))
}

#[cfg(not(any(unix, windows)))]
fn resource_hash(path: &Path) -> u64 {
    fnv1a(path.to_string_lossy().bytes())
}

fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_final_live_lease_can_clean_up() {
        let resource = unique_resource("last");
        let mut first = ConfigLease::acquire(&resource).unwrap();
        let mut second = ConfigLease::acquire(&resource).unwrap();
        first.mark_created().unwrap();

        assert!(first.try_take_last().unwrap().is_none());
        let mut last = second.try_take_last().unwrap().unwrap();
        assert!(last.take_created().unwrap());
        drop(last);

        let mut later = ConfigLease::acquire(&resource).unwrap();
        let mut last = later.try_take_last().unwrap().unwrap();
        assert!(!last.take_created().unwrap());
    }

    #[test]
    fn dropped_lease_does_not_strand_the_resource() {
        let resource = unique_resource("crash");
        let mut abandoned = ConfigLease::acquire(&resource).unwrap();
        abandoned.mark_created().unwrap();
        drop(abandoned);

        let mut survivor = ConfigLease::acquire(&resource).unwrap();
        let mut last = survivor.try_take_last().unwrap().unwrap();
        assert!(last.take_created().unwrap());
    }

    fn unique_resource(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "paneflow-agent-config-lease-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }
}
