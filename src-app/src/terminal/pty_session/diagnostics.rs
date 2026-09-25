use super::*;

#[derive(Clone, Copy, PartialEq)]
pub enum Osc52Mode {
    Disabled,
    CopyOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyBuildDiagnostics {
    pub version: &'static str,
    pub source_sha: &'static str,
    pub api_version: &'static str,
    pub zig_version: &'static str,
    pub optimization: &'static str,
    pub simd: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalBackendFailurePhase {
    Initialization,
}

impl TerminalBackendFailurePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initialization => "initialization",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalBackendFailureDiagnostics {
    pub phase: TerminalBackendFailurePhase,
    pub reason_code: &'static str,
    pub os_error: Option<i32>,
}

impl TerminalBackendFailureDiagnostics {
    pub(in crate::terminal) const GHOSTTY_INITIALIZATION_FAILED: &'static str =
        "ghostty_initialization_failed";

    pub(in crate::terminal) fn new(
        phase: TerminalBackendFailurePhase,
        reason_code: &'static str,
        os_error: Option<i32>,
    ) -> Self {
        Self {
            phase,
            reason_code,
            os_error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalBackendDiagnostics {
    pub failure: Option<TerminalBackendFailureDiagnostics>,
    pub target_triple: &'static str,
    pub ghostty: GhosttyBuildDiagnostics,
}

impl std::fmt::Display for TerminalBackendDiagnostics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (failure_phase, reason_code, os_error) =
            self.failure
                .as_ref()
                .map_or(("none", "none", None), |failure| {
                    (
                        failure.phase.as_str(),
                        failure.reason_code,
                        failure.os_error,
                    )
                });
        write!(
            formatter,
            "backend=ghostty failure_phase={failure_phase} reason_code={reason_code} target={} os_error=",
            self.target_triple
        )?;
        match os_error {
            Some(code) => write!(formatter, "{code}")?,
            None => formatter.write_str("none")?,
        }
        write!(
            formatter,
            " ghostty_version={} ghostty_source_sha={} ghostty_api_version={} zig_version={} optimization={} simd={}",
            self.ghostty.version,
            self.ghostty.source_sha,
            self.ghostty.api_version,
            self.ghostty.zig_version,
            self.ghostty.optimization,
            self.ghostty.simd,
        )
    }
}

pub(in crate::terminal) fn raw_os_error_from_anyhow(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|source| {
        source
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_failure_phases_and_reason_codes_are_stable() {
        assert_eq!(
            TerminalBackendFailurePhase::Initialization.as_str(),
            "initialization"
        );
        assert_eq!(
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            "ghostty_initialization_failed"
        );
    }

    #[test]
    fn backend_diagnostics_expose_target_triple() {
        let diagnostics = TerminalState::new_display_only(24, 80).backend_diagnostics();
        assert_eq!(diagnostics.target_triple, env!("PANEFLOW_TARGET_TRIPLE"));
        #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
        assert_eq!(diagnostics.target_triple, "x86_64-pc-windows-msvc");
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(diagnostics.target_triple, "aarch64-apple-darwin");
    }

    #[test]
    fn backend_diagnostics_expose_pinned_ghostty_build_identity() {
        let diagnostics = TerminalState::new_display_only(24, 80).backend_diagnostics();
        let ghostty = diagnostics.ghostty;
        let identity = paneflow_terminal_ghostty::build_identity();
        assert_eq!(
            ghostty.version,
            paneflow_terminal_ghostty::GHOSTTY_APP_VERSION
        );
        assert_eq!(ghostty.source_sha, identity.source_sha);
        assert_eq!(ghostty.api_version, identity.api_version);
        assert_eq!(ghostty.zig_version, identity.zig_version);
        assert_eq!(ghostty.optimization, identity.optimization);
        assert_eq!(ghostty.simd, identity.simd);
    }
}
