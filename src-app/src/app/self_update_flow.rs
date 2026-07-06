//! In-app self-update dispatcher - routes clicks on the update pill to the
//! right installer branch (SystemPackage / AppImage / TarGz+Linux Unknown /
//! legacy `.run`) based on the detected
//! [`crate::update::install_method::InstallMethod`].
//!
//! Extracted from `main.rs` per US-028 of the src-app refactor PRD.

use gpui::{ClipboardItem, Context, Window};

use crate::app::telemetry_events::UpdateDismissReason;
use crate::{
    DismissUpdate, PaneFlowApp, StartSelfUpdate, TOAST_HOLD_MS, ToastAction,
    system_package_update_command, update,
};

/// App-level backstop for a wedged `Downloading`/`Installing` state (EP-002,
/// U-002/U-015). Every installer worker is spawned + detached and the only
/// transitions out of `Downloading` live inside those workers' match arms, so
/// a worker whose future never resolves would pin the pill busy forever. The
/// per-attempt watchdog routes through `record_update_failure` after this
/// deadline. Sized longer than any per-subprocess deadline (the AppImage tool's
/// is 10 min) and generous enough to outlast a legitimate pkexec polkit prompt
/// the user is still answering.
const DOWNLOAD_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// One-line summary of the install method for log messages - used by the
/// auto-kickoff gate to keep diagnostic noise low when the running binary
/// is not auto-updatable.
fn install_method_label(method: &update::install_method::InstallMethod) -> &'static str {
    match method {
        update::install_method::InstallMethod::AppImage { .. } => "appimage",
        update::install_method::InstallMethod::TarGz { .. } => "targz",
        update::install_method::InstallMethod::AppBundle { .. } => "app-bundle",
        update::install_method::InstallMethod::WindowsMsi { .. } => "windows-msi",
        update::install_method::InstallMethod::SystemPackage { .. } => "system-package",
        update::install_method::InstallMethod::ExternallyManaged { .. } => "externally-managed",
        update::install_method::InstallMethod::Unknown => "unknown",
    }
}

/// `Unknown` is only a self-updatable tar.gz migration path on Linux. macOS
/// `Unknown` means the binary is outside a `.app` bundle, while Windows uses
/// MSI; both must avoid writing a Linux layout under `$HOME/.local`.
fn unknown_install_uses_targz() -> bool {
    cfg!(target_os = "linux")
}

/// Strict-semver guard for the release tag before it reaches any
/// user-facing surface (clipboard, toast, argv). Matches the regex
/// `^v?\d+\.\d+\.\d+$` - identical to the validator inside
/// `update::linux::system_package::validate_version`, inlined here so
/// the check runs even on code paths that bypass `run_update`
/// (`PackageManager::Other` clipboard fallback, non-Linux targets,
/// and the `EnvironmentBroken` clipboard fallback). Keeping the rule
/// in two places is a deliberate trade for keeping `validate_version`
/// private to its Linux-only module; US-054 adds
/// `system_package::tests::version_validators_agree` to guard the two
/// implementations against drifting apart.
pub(crate) fn is_strict_semver(raw: &str) -> bool {
    let rest = raw.strip_prefix('v').unwrap_or(raw);
    let mut completed_parts: usize = 0;
    let mut segment_len: usize = 0;
    for ch in rest.chars() {
        match ch {
            '0'..='9' => segment_len = segment_len.saturating_add(1),
            '.' => {
                if segment_len == 0 {
                    return false;
                }
                completed_parts = completed_parts.saturating_add(1);
                segment_len = 0;
            }
            _ => return false,
        }
    }
    if segment_len == 0 {
        return false;
    }
    completed_parts.saturating_add(1) == 3
}

impl PaneFlowApp {
    /// Current update CTA state, shared by every surface that renders it
    /// (Diff title-bar pill, Cli/Agents sidebar banner). `None` when no
    /// update is available (or the pill was dismissed for this launch).
    ///
    /// Pill state for in-app installer flows (AppImage, TarGz, AppBundle,
    /// MSI, pkexec dnf|apt) is shared between the SystemPackage branch and
    /// the catch-all so both reflect the live install state machine; if
    /// the SystemPackage branch ignored it, the pkexec dnf/apt path would
    /// render "Update via dnf" frozen for the entire install while
    /// is_busy() silently dropped clicks.
    pub(crate) fn update_pill_info(&self) -> Option<crate::window_chrome::title_bar::UpdateInfo> {
        use crate::window_chrome::title_bar;
        let in_app_state = match &self.self_update.self_update_status {
            update::SelfUpdateStatus::Idle => title_bar::SelfUpdatePillState::Idle,
            update::SelfUpdateStatus::Downloading => title_bar::SelfUpdatePillState::Downloading,
            update::SelfUpdateStatus::Installing => title_bar::SelfUpdatePillState::Installing,
            update::SelfUpdateStatus::ReadyToRestart => {
                title_bar::SelfUpdatePillState::ReadyToRestart
            }
            update::SelfUpdateStatus::Errored(_) => title_bar::SelfUpdatePillState::Errored,
        };
        match &self.self_update.update_status {
            Some(update::checker::UpdateStatus::Available { version, .. }) => {
                let kind = match &self.self_update.install_method {
                    update::install_method::InstallMethod::SystemPackage { manager } => {
                        match manager {
                            // Dnf / Apt: in-app pkexec install. Pill follows
                            // the install state machine like every other
                            // in-app installer.
                            update::install_method::PackageManager::Dnf
                            | update::install_method::PackageManager::Apt => {
                                title_bar::UpdatePillKind::InApp(in_app_state)
                            }
                            // Clipboard-only paths: kickoff_self_update_install
                            // returns early after copying the upgrade command,
                            // self_update_status never leaves Idle.
                            update::install_method::PackageManager::RpmOstree => {
                                title_bar::UpdatePillKind::SystemManaged(
                                    title_bar::SystemPackageKind::RpmOstree,
                                )
                            }
                            update::install_method::PackageManager::Other => {
                                title_bar::UpdatePillKind::SystemManaged(
                                    title_bar::SystemPackageKind::Other,
                                )
                            }
                        }
                    }
                    // Flatpak / Snap / `PANEFLOW_UPDATE_EXPLANATION` -
                    // packager owns updates, render the same generic
                    // SystemHint pill. The explanation copy is surfaced
                    // by the click handler below.
                    update::install_method::InstallMethod::ExternallyManaged { .. } => {
                        title_bar::UpdatePillKind::SystemManaged(
                            title_bar::SystemPackageKind::Other,
                        )
                    }
                    _ => title_bar::UpdatePillKind::InApp(in_app_state),
                };
                Some(title_bar::UpdateInfo {
                    version: version.clone(),
                    kind,
                })
            }
            _ => None,
        }
    }

    /// Action entry point. Stays a thin wrapper around
    /// [`PaneFlowApp::kickoff_self_update_install`] so that auto-kickoff
    /// from the polling loop can share the exact same logic without
    /// having to forge a `Window`.
    pub(crate) fn handle_start_self_update(
        &mut self,
        _: &StartSelfUpdate,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kickoff_self_update_install(cx);
    }

    /// US-007 AC3: dismiss the update pill for the current launch.
    /// Clears `update_status` so the title-bar pill disappears, fires
    /// a `update_dismissed` PostHog event, and forces a re-render.
    /// Intentionally NOT persisted - the next paneflow launch will
    /// re-detect the update and re-show the pill (we don't want a
    /// user accidentally sticking on an old version because the
    /// preference outlived their interest).
    pub(crate) fn handle_dismiss_update(
        &mut self,
        _: &DismissUpdate,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Capture the to_version BEFORE we drop update_status so the
        // emit helper can reference it.
        self.emit_update_dismissed(UpdateDismissReason::UserDismissed);
        self.self_update.update_status = None;
        cx.notify();
    }

    /// US-017: shared completion for every pre-installed update path. Flips to
    /// `ReadyToRestart`, persists the session (blocking - the next event is a
    /// process-replacing restart), and queues the `update_installed` analytics
    /// event WITHOUT a blocking flush (the background `poll_flush` loop drains
    /// it; the restart click stays zero-I/O). Dedups the six identical blocks
    /// that previously inlined this - and that previously called
    /// `flush_blocking` on the render thread, the `[HIGH]` finding.
    fn on_preinstall_success(&mut self, cx: &mut Context<Self>) {
        self.self_update.self_update_status = update::SelfUpdateStatus::ReadyToRestart;
        self.save_session_blocking(cx);
        self.emit_update_success();
        cx.notify();
    }

    /// Enter the `Downloading` state and arm a one-shot watchdog (EP-002,
    /// U-002/U-015). Replaces the bare `self_update_status = Downloading;
    /// cx.notify();` at every installer dispatch site so no path can sit in
    /// `Downloading` forever: if we are still in THIS attempt and still busy
    /// after [`DOWNLOAD_WATCHDOG`], the failure is routed through
    /// [`PaneFlowApp::record_update_failure`] (which leaves the busy state,
    /// bumps the 3-strikes counter, and surfaces the timeout toast), making the
    /// retry / circuit-breaker / `EnvironmentBroken` paths reachable again.
    fn enter_downloading(&mut self, label: &'static str, cx: &mut Context<Self>) {
        let generation = self.self_update.download_generation.wrapping_add(1);
        self.self_update.download_generation = generation;
        self.self_update.self_update_status = update::SelfUpdateStatus::Downloading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            smol::Timer::after(DOWNLOAD_WATCHDOG).await;
            let _ = this.update(cx, |app, cx| {
                // Fire only if THIS download is still the live one and still
                // busy; a completed / failed / superseded worker already moved
                // the state on, and the generation guard stops a stale watchdog
                // from clobbering a newer attempt.
                if app.self_update.download_generation == generation
                    && app.self_update.self_update_status.is_busy()
                {
                    log::warn!(
                        "self-update/{label}: watchdog fired after {DOWNLOAD_WATCHDOG:?} - \
                         worker wedged in {:?}; resetting via record_update_failure",
                        app.self_update.self_update_status,
                    );
                    app.record_update_failure(
                        label,
                        &anyhow::Error::new(update::UpdateError::Timeout),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Kick off the in-app self-update flow. See the module-level doc for the
    /// branch matrix; on any failure a toast surfaces and the update pill
    /// returns to "Update failed".
    pub(crate) fn kickoff_self_update_install(&mut self, cx: &mut Context<Self>) {
        // Fast path: the new binary is already on disk and
        // `set_restart_path` has been wired ahead of time by the
        // background pre-installer (see `try_auto_kickoff_install`
        // below). The click handler does ZERO I/O - just hand control
        // to GPUI's relauncher script. This is what makes the
        // user-perceived restart latency drop from "vachement long"
        // (download + install + analytics flush) to GPUI's
        // ~100 ms `kill -0` polling interval.
        if matches!(
            self.self_update.self_update_status,
            update::SelfUpdateStatus::ReadyToRestart
        ) {
            log::info!("self-update: ReadyToRestart click - invoking cx.restart()");
            cx.restart();
            return;
        }

        // Externally managed runtime (Flatpak / Snap / packager-baked
        // `PANEFLOW_UPDATE_EXPLANATION`). The in-app updater is disabled
        // by design - surface the packager's explanation copy and copy
        // the upgrade command to the clipboard so the user has a one-click
        // path forward. Mirrors how Zed handles `ZED_UPDATE_EXPLANATION`.
        if let update::install_method::InstallMethod::ExternallyManaged { explanation } =
            &self.self_update.install_method
        {
            cx.write_to_clipboard(ClipboardItem::new_string(explanation.clone()));
            self.push_toast(explanation.clone(), Vec::new(), TOAST_HOLD_MS * 4, cx);
            return;
        }

        if self.self_update.self_update_status.is_busy() {
            return;
        }

        // System-package installs (.deb/.rpm). Fedora / RHEL / Rocky and
        // Ubuntu / Debian users on the signed pkg.paneflow.dev repo get an
        // in-app pkexec-elevated `dnf|apt-get install` (US-002). openSUSE,
        // Solus, Void, NixOS et al. fall back to the clipboard-copy flow so
        // they at least see a package-manager hint. `return`s
        // BEFORE reading `asset_url` below - the pkexec flow pulls its
        // payload from the system repo; no direct GitHub download.
        //
        // Note: `InstallMethod::SystemPackage` is declared unconditionally
        // (not `#[cfg]`-gated) so this `if let` must compile on every
        // target. The pkexec call below is Linux-only; non-Linux targets
        // route through the clipboard-copy fallback. In practice
        // `install_method::detect()` only produces `SystemPackage` on
        // Linux, so the non-Linux path is compile-only ballast.
        if let update::install_method::InstallMethod::SystemPackage { manager } =
            &self.self_update.install_method
        {
            let version = match &self.self_update.update_status {
                Some(update::checker::UpdateStatus::Available { version, .. }) => version.clone(),
                _ => return,
            };

            // Defence in depth: reject any version that is not strict
            // semver BEFORE formatting it into a clipboard string, a
            // toast, or argv. US-001 already regex-validates inside
            // `run_update`, but the `PackageManager::Other` branch and
            // the `EnvironmentBroken` fallback both construct a
            // user-visible "Copied: sudo apt install paneflow=<ver>"
            // toast WITHOUT going through `run_update` first. A
            // compromised GitHub tag (e.g. `0.2.3; rm -rf $HOME`)
            // would otherwise end up in the user's clipboard verbatim.
            // The three-dot-decimal grammar matches
            // `system_package::validate_version`.
            if !is_strict_semver(&version) {
                log::warn!(
                    "self-update/system-package: refusing malformed version string: {version:?}"
                );
                self.show_toast("Update unavailable - invalid release tag".to_string(), cx);
                return;
            }

            // US-004: rpm-ostree (Silverblue / Kinoite / Bazzite).
            // Immutable distros stage updates offline for the next
            // reboot - pkexec+dnf would fail against the read-only
            // `/usr`. Surface a dedicated informational toast and
            // copy `rpm-ostree upgrade` to the clipboard. No
            // subprocess spawn, no `cx.restart()` - the update does
            // not take effect until the user reboots.
            if matches!(manager, update::install_method::PackageManager::RpmOstree) {
                cx.write_to_clipboard(ClipboardItem::new_string("rpm-ostree upgrade".to_string()));
                // Long-form informational copy; use `push_toast`
                // with 4× hold so the user has time to read it
                // (default TOAST_HOLD_MS is tuned for short
                // "Copied: …" confirmations).
                self.push_toast(
                    "PaneFlow detects an immutable distribution. Update must be run via `rpm-ostree upgrade` at the system level - the update has been copied to your clipboard.".to_string(),
                    Vec::new(),
                    TOAST_HOLD_MS * 4,
                    cx,
                );
                return;
            }

            // PackageManager::Other (Solus, Void, NixOS, …): no reliable
            // repo from our side → keep the clipboard-copy behaviour.
            // Same code path as pre-US-002. Also used as the
            // compile-only fallback on macOS / Windows for the
            // (unreachable-at-runtime) Dnf / Apt variants.
            //
            // `RpmOstree` is intentionally absent from the whitelist
            // below - Silverblue / Kinoite users are already served
            // by the dedicated informational arm above, which always
            // `return`s. If a future refactor removes that early
            // return, `RpmOstree` would fall through to the generic
            // clipboard-copy path (safe but wrong copy - never to
            // pkexec, because the whitelist excludes it).
            #[cfg(not(target_os = "linux"))]
            let run_pkexec = false;
            #[cfg(target_os = "linux")]
            let run_pkexec = matches!(
                manager,
                update::install_method::PackageManager::Dnf
                    | update::install_method::PackageManager::Apt
            );

            if !run_pkexec {
                let command = system_package_update_command(Some(manager), &version);
                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                self.show_toast(format!("Copied: {command}"), cx);
                return;
            }

            // Dnf / Apt on Linux: full pkexec flow, matching the
            // AppImage / TarGz one-click UX. Status transitions:
            // Idle → Downloading → (on Ok) Installing → save_session →
            // set_restart_path → restart.
            #[cfg(target_os = "linux")]
            {
                let manager_owned = manager.clone();
                let manager_label: &'static str = match manager_owned {
                    update::install_method::PackageManager::Dnf => "dnf",
                    update::install_method::PackageManager::Apt => "apt",
                    // Other / RpmOstree are short-circuited above via
                    // the rpm-ostree informational arm and the
                    // `run_pkexec` gate; these arms exist purely for
                    // compile-time exhaustiveness.
                    update::install_method::PackageManager::Other => "system-package",
                    update::install_method::PackageManager::RpmOstree => "rpm-ostree",
                };
                self.enter_downloading(manager_label, cx);

                cx.spawn(async move |this, cx| {
                    let result = smol::unblock({
                        // Clone into the worker task; `manager_owned`
                        // (outer) stays in scope for the
                        // `EnvironmentBroken` clipboard fallback below.
                        let manager_for_worker = manager_owned.clone();
                        let version_for_worker = version.clone();
                        move || {
                            update::linux::system_package::run_update(
                                &manager_for_worker,
                                &version_for_worker,
                            )
                        }
                    })
                    .await;

                    match result {
                        Ok(()) => {
                            let restart_path = std::path::PathBuf::from("/usr/bin/paneflow");
                            let _ = this.update(cx, |app, cx| {
                                // Pre-installed: flip to ReadyToRestart so the
                                // pill becomes a one-call restart button. The
                                // analytics flush + session save still run
                                // here (now, while the user is busy), not at
                                // click time.
                                app.on_preinstall_success(cx);
                            });
                            cx.update(|cx| {
                                log::info!(
                                    "self-update/{manager_label}: pre-installed - restart pending at /usr/bin/paneflow"
                                );
                                cx.set_restart_path(restart_path);
                            });
                        }
                        Err(err) => {
                            // Classify once on the async side; the
                            // closure below only decides which state
                            // transition + toast copy to run on the
                            // main thread.
                            let classified = update::UpdateError::classify(&err);
                            let _ = this.update(cx, |app, cx| match classified {
                                // Polkit "Cancel" - benign. Revert to
                                // Idle, neutral toast, DO NOT bump the
                                // retry counter (user intent, not a
                                // failure).
                                update::UpdateError::InstallDeclined { .. } => {
                                    app.self_update.self_update_status = update::SelfUpdateStatus::Idle;
                                    app.show_toast("Update cancelled".to_string(), cx);
                                    cx.notify();
                                }
                                // pkexec missing / no polkit agent /
                                // exit 127 - fall back to the
                                // clipboard-copy behaviour so the user
                                // has a runnable command. No retry
                                // bump (transient env issue, not a
                                // package-mgr failure).
                                update::UpdateError::EnvironmentBroken { .. } => {
                                    let command = system_package_update_command(
                                        Some(&manager_owned),
                                        &version,
                                    );
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        command.clone(),
                                    ));
                                    app.self_update.self_update_status = update::SelfUpdateStatus::Idle;
                                    app.show_toast(format!("Copied: {command}"), cx);
                                    cx.notify();
                                }
                                // US-005: backpressure - `dnf-automatic`
                                // or an interactive `sudo apt install`
                                // held the package-manager lock at
                                // pre-flight time. Transient condition
                                // (user can retry in a moment), NOT a
                                // real failure, so skip the 3-strikes
                                // counter and show a neutral toast.
                                // Match on the exact sentinel emitted
                                // by `run_update` so brittle substring
                                // matching is avoided.
                                update::UpdateError::Other(ref msg)
                                    if msg == update::linux::system_package::BUSY_MESSAGE =>
                                {
                                    app.self_update.self_update_status = update::SelfUpdateStatus::Idle;
                                    app.push_toast(
                                        update::linux::system_package::BUSY_MESSAGE.to_string(),
                                        Vec::new(),
                                        TOAST_HOLD_MS * 2,
                                        cx,
                                    );
                                    cx.notify();
                                }
                                // Anything else (mirror 5xx, disk full,
                                // signal, transaction conflict) - real
                                // failure; feed the 3-strikes counter
                                // via record_update_failure.
                                _ => {
                                    app.record_update_failure(manager_label, &err, cx);
                                }
                            });
                        }
                    }
                })
                .detach();
                return;
            }
        }

        // After 3 consecutive failures, the 4th click stops re-trying and
        // points the user at the releases page (US-013). Skipping the
        // network here is important - repeated fast retries against a
        // flaky mirror are never the right answer.
        if self.self_update.update_attempt_count >= 3 {
            let releases_url = match &self.self_update.update_status {
                Some(update::checker::UpdateStatus::Available { url, .. }) => url.clone(),
                _ => "https://github.com/ArthurDEV44/paneflow/releases".to_string(),
            };
            self.push_toast(
                "Update keeps failing. Download manually from the releases page.".to_string(),
                vec![ToastAction::OpenReleasesPage(releases_url)],
                TOAST_HOLD_MS * 4,
                cx,
            );
            return;
        }

        let asset_url = match &self.self_update.update_status {
            Some(update::checker::UpdateStatus::Available {
                asset_url: Some(url),
                ..
            }) => url.clone(),
            Some(update::checker::UpdateStatus::Available { url, .. }) => {
                // No Linux asset on this release (edge case: draft, mis-tagged).
                // Fall back to opening the release page so the user can grab it.
                let _ = open::that(url);
                return;
            }
            _ => return,
        };

        // No trust anchor baked into this build (a dev build, or a release cut
        // before the US-002 signing keys were provisioned). Refuse to start ANY
        // installer before touching disk. `fetch_and_verify` already fails
        // closed on a keyless build (signature.rs), but for AppImage that
        // rejection only fires *after* `appimageupdatetool -O` has rewritten the
        // live binary in place - mutating a binary we can never verify. Bailing
        // here keeps every install path verify-before-side-effect and shows a
        // clear message instead of a silently corrupted AppImage.
        if !update::signature::has_embedded_key() {
            self.push_toast(
                "This build can't self-update (unsigned). Download the latest version from the releases page.".to_string(),
                vec![ToastAction::OpenReleasesPage(
                    "https://github.com/ArthurDEV44/paneflow/releases".to_string(),
                )],
                TOAST_HOLD_MS * 4,
                cx,
            );
            return;
        }

        // Use the cached install method. The install location never changes
        // at runtime, so one probe at startup is enough.
        let method = self.self_update.install_method.clone();
        if let update::install_method::InstallMethod::AppImage { source_path, .. } = &method {
            let source_path = source_path.clone();
            // `appimageupdatetool` does one opaque call that covers both the
            // zsync download and the in-place rewrite. Most of the
            // wall-clock time is spent fetching delta blocks, so `Downloading`
            // matches what the user actually sees on a slow link.
            self.enter_downloading("appimage", cx);

            let asset_url_for_verify = asset_url.clone();
            cx.spawn(async move |this, cx| {
                let result = smol::unblock({
                    let source_path = source_path.clone();
                    let asset_url = asset_url_for_verify.clone();
                    // US-006: pass the new asset URL so run_update can fetch
                    // its `.minisig` and re-verify the rewritten AppImage.
                    move || update::linux::appimage::run_update(&source_path, &asset_url)
                })
                .await;

                match result {
                    Ok(updated_path) => {
                        let _ = this.update(cx, |app, cx| {
                            app.on_preinstall_success(cx);
                        });
                        cx.update(|cx| {
                            log::info!(
                                "self-update/appimage: pre-installed - restart pending at {}",
                                updated_path.display()
                            );
                            cx.set_restart_path(updated_path);
                        });
                    }
                    Err(err) => {
                        let _ = this.update(cx, |app, cx| {
                            app.record_update_failure("appimage", &err, cx);
                        });
                    }
                }
            })
            .detach();
            return;
        }

        // `Unknown` (dev builds, legacy `.run` migrations) routes through the
        // tar.gz updater only on Linux, where `$HOME` exists and
        // `~/.local/paneflow.app/` is the portable install target. macOS
        // `Unknown` means the binary is outside a `.app` bundle, and Windows
        // uses MSI, so both fall through instead of silently writing a
        // Linux-style install elsewhere. On Windows, `targz::run_update` reads an unset
        // `$HOME` and fails with a cryptic "HOME environment variable is not
        // set", so `Unknown` falls through to the manual-download path too.
        // `TarGz` itself is only ever produced on Linux by `detect()`.
        let route_to_targz = matches!(&method, update::install_method::InstallMethod::TarGz { .. })
            || (unknown_install_uses_targz()
                && matches!(&method, update::install_method::InstallMethod::Unknown));
        if route_to_targz {
            // Atomic directory swap under `$HOME/.local/paneflow.app/`.
            // `run_update` derives the target paths from `$HOME` internally,
            // so we only need to hand it the release asset URL.
            //
            // `Unknown` (dev builds, legacy `.run` migrations) dispatches here
            // too: `pick_asset` already returns a `.tar.gz` URL for Unknown
            // (see `AssetFormat::from_install_method`), and since v0.2.0 no
            // longer emits `.run` assets, falling through to the legacy
            // installer below would try to `chmod +x` + execve a gzip file.
            //
            // Log the migration path so dev-build users (who hit the Unknown
            // branch after `cargo run`) see what's happening. The updater
            // still proceeds - the install lands at `$HOME/.local/paneflow.app/`
            // regardless of where `current_exe()` was - but the log makes the
            // directory change visible instead of silent.
            if matches!(&method, update::install_method::InstallMethod::Unknown) {
                log::warn!(
                    "self-update: install method Unknown - downloading tar.gz release \
                     into $HOME/.local/paneflow.app/; the updated binary will be at a \
                     different path than the currently-running one."
                );
            }
            let url = asset_url.clone();
            self.enter_downloading("targz", cx);

            cx.spawn(async move |this, cx| {
                let result = smol::unblock(move || update::linux::targz::run_update(&url)).await;

                match result {
                    Ok(restart_path) => {
                        let _ = this.update(cx, |app, cx| {
                            app.on_preinstall_success(cx);
                        });
                        cx.update(|cx| {
                            log::info!(
                                "self-update/targz: pre-installed - restart pending at {}",
                                restart_path.display()
                            );
                            cx.set_restart_path(restart_path);
                        });
                    }
                    Err(err) => {
                        let _ = this.update(cx, |app, cx| {
                            app.record_update_failure("targz", &err, cx);
                        });
                    }
                }
            })
            .detach();
            return;
        }

        // US-010: Windows MSI install. Unlike AppImage/TarGz/DMG, the MSI
        // cannot be fully pre-installed while Paneflow is still running:
        // Windows Installer would detect the live `paneflow.exe`, show its
        // native FilesInUse dialog, and may close the very process that owns
        // the restart state. The GUI therefore only downloads + verifies the
        // MSI, then spawns a breakaway relay, saves state, and exits. The relay
        // runs msiexec after this PID is gone and relaunches the detected
        // install path on success.
        if let update::install_method::InstallMethod::WindowsMsi { install_path } = &method {
            let url = asset_url.clone();
            let install_path = install_path.clone();
            self.enter_downloading("msi", cx);

            cx.spawn(async move |this, cx| {
                let result =
                    smol::unblock(move || update::windows::msi::stage(&url, &install_path)).await;
                match result {
                    Ok(staged) => {
                        let _ = this.update(cx, |app, cx| {
                            app.save_session_blocking(cx);
                            match update::windows::msi::spawn_relay(staged) {
                                Ok(()) => {
                                    log::info!(
                                        "self-update/msi: relay spawned - quitting so msiexec can replace paneflow.exe"
                                    );
                                    app.self_update.self_update_status =
                                        update::SelfUpdateStatus::Installing;
                                    cx.notify();
                                    cx.quit();
                                }
                                Err(err) => {
                                    app.record_update_failure("msi-relay", &err, cx);
                                }
                            }
                        });
                    }
                    Err(err) => {
                        let _ = this.update(cx, |app, cx| {
                            app.record_update_failure("msi", &err, cx);
                        });
                    }
                }
            })
            .detach();
            return;
        }

        // US-009: macOS `.app` bundle - mount the DMG, swap the bundle
        // atomically, then restart through the promoted `.app` path.
        // Dispatch is an `if let` (not a cfg guard) so the code remains
        // a single compile-closure across all targets; the
        // `InstallMethod::AppBundle` variant is only produced on macOS
        // by `install_method::detect()`, so on Linux / Windows this
        // branch is runtime-dead without needing a `#[cfg(target_os)]`.
        if let update::install_method::InstallMethod::AppBundle { bundle_path } = &method {
            let url = asset_url.clone();
            // US-004: replace the bundle at its detected location, not a
            // hardcoded /Applications path.
            let bundle = bundle_path.clone();
            // EP-002 AC2: `dmg::install` runs `hdiutil attach/detach` + `cp` on
            // the ALREADY-downloaded local `.dmg` (network fetch separately
            // bounded by `UPDATE_HTTP_TIMEOUT`). Killing a mounted-volume
            // operation mid-flight risks leaking a mount / corrupting the swap,
            // so these local tools are NOT wrapped in `run_with_timeout`; the
            // worker watchdog armed below bounds a wedged install.
            self.enter_downloading("dmg", cx);

            cx.spawn(async move |this, cx| {
                let result =
                    smol::unblock(move || update::macos::dmg::install(&url, &bundle)).await;
                match result {
                    Ok(restart_path) => {
                        let _ = this.update(cx, |app, cx| {
                            app.on_preinstall_success(cx);
                        });
                        cx.update(|cx| {
                            log::info!(
                                "self-update/dmg: pre-installed - restart pending at {}",
                                restart_path.display()
                            );
                            cx.set_restart_path(restart_path);
                        });
                    }
                    Err(err) => {
                        let _ = this.update(cx, |app, cx| {
                            app.record_update_failure("dmg", &err, cx);
                        });
                    }
                }
            })
            .detach();
            return;
        }

        // All supported install methods must have returned above. Do not fall
        // back to the retired `.run` executor: current releases do not ship it,
        // and keeping a downloader plus chmod plus exec path around would be an
        // avoidable update-chain risk.
        let msg = anyhow::anyhow!(
            "Self-update dispatch did not handle install method {:?}. Download the new release manually from {asset_url}",
            method
        );
        self.record_update_failure("unsupported-dispatch", &msg, cx);
    }

    /// Best-effort background pre-install. Called once per polling cycle
    /// after `update_status` transitions to `Available`. By the time
    /// the user actually clicks the pill, the new binary is already on
    /// disk and `set_restart_path` is wired - `cx.restart()` is the
    /// only thing left to do, dropping click→restart latency from
    /// download-time + 2 s analytics flush to GPUI's `kill -0` watcher
    /// interval (~100 ms). Mirrors Zed's silent auto-update worker
    /// (`crates/auto_update/src/auto_update.rs::poll`).
    ///
    /// Gating, in order:
    /// - `update_status` is `Available`.
    /// - `self_update_status` is `Idle` - never re-kick a flow that's
    ///   already downloading, installed, or errored.
    /// - `update_attempt_count < 3` - reuse the 3-strikes circuit
    ///   breaker so a flaky mirror doesn't burn user bandwidth every
    ///   poll cycle.
    /// - `install_method` is auto-installable without exiting the app
    ///   (AppImage / TarGz / AppBundle / Linux Unknown). WindowsMsi deliberately
    ///   waits for an explicit click because it must quit Paneflow before
    ///   invoking Windows Installer. macOS Unknown is a non-bundle launch and
    ///   is intentionally not auto-kicked. SystemPackage needs pkexec
    ///   (interactive auth - never auto), ExternallyManaged defers to the host
    ///   package manager.
    pub(crate) fn try_auto_kickoff_install(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.self_update.update_status,
            Some(update::checker::UpdateStatus::Available { .. })
        ) {
            return;
        }
        if !matches!(
            self.self_update.self_update_status,
            update::SelfUpdateStatus::Idle
        ) {
            return;
        }
        if self.self_update.update_attempt_count >= 3 {
            return;
        }
        let auto_eligible = matches!(
            self.self_update.install_method,
            update::install_method::InstallMethod::AppImage { .. }
                | update::install_method::InstallMethod::TarGz { .. }
                | update::install_method::InstallMethod::AppBundle { .. }
        ) || (matches!(
            self.self_update.install_method,
            update::install_method::InstallMethod::Unknown
        ) && unknown_install_uses_targz());
        if !auto_eligible {
            log::debug!(
                "self-update/auto-kickoff: skipped (install_method={})",
                install_method_label(&self.self_update.install_method)
            );
            return;
        }

        log::info!(
            "self-update/auto-kickoff: starting background pre-install (install_method={})",
            install_method_label(&self.self_update.install_method)
        );
        self.kickoff_self_update_install(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_targz_fallback_excludes_macos() {
        assert_eq!(unknown_install_uses_targz(), cfg!(target_os = "linux"));
    }
}
