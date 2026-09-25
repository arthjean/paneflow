const CLAUDECODE_ENV: &str = "CLAUDECODE";

pub(crate) unsafe fn scrub_claudecode_env_before_threads() {
    unsafe {
        std::env::remove_var(CLAUDECODE_ENV);
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use win32job::{ExtendedLimitInfo, Job};

    pub(super) fn install() -> Result<(), Box<dyn std::error::Error>> {
        let mut info = ExtendedLimitInfo::default();
        info.limit_kill_on_job_close().limit_breakaway_ok();
        let job = Job::create_with_limit_info(&info)?;
        job.assign_current_process()?;
        std::mem::forget(job);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ParentGuardStatus {
    Installed,
    Unsupported,
}

pub fn install_process_job() -> Result<ParentGuardStatus, Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::install()?;
        Ok(ParentGuardStatus::Installed)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(ParentGuardStatus::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_process_job_does_not_panic() {
        let _ = install_process_job();
        let _ = install_process_job();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_install_is_documented_unsupported() {
        assert_eq!(
            install_process_job().unwrap(),
            ParentGuardStatus::Unsupported
        );
    }
}
