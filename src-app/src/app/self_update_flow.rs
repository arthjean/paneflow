use gpui::{ClipboardItem, Context, Window};

use crate::{
    DismissUpdate, PaneFlowApp, StartSelfUpdate, TOAST_HOLD_MS, ToastAction,
    system_package_update_command, update,
};

const DOWNLOAD_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub(crate) fn install_method_label(method: &update::install_method::InstallMethod) -> &'static str {
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

fn unknown_install_uses_targz() -> bool {
    cfg!(target_os = "linux")
}

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
                            update::install_method::PackageManager::Dnf
                            | update::install_method::PackageManager::Apt
                            | update::install_method::PackageManager::Zypper => {
                                title_bar::UpdatePillKind::InApp(in_app_state)
                            }
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

    pub(crate) fn handle_start_self_update(
        &mut self,
        _: &StartSelfUpdate,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kickoff_self_update_install(cx);
    }

    pub(crate) fn handle_dismiss_update(
        &mut self,
        _: &DismissUpdate,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.emit_update_dismissed();
        self.self_update.update_status = None;
        cx.notify();
    }

    fn on_preinstall_success(&mut self, cx: &mut Context<Self>) {
        self.self_update.self_update_status = update::SelfUpdateStatus::ReadyToRestart;
        self.save_session_blocking(cx);
        self.emit_update_success();
        cx.notify();
    }

    fn enter_downloading(&mut self, label: &'static str, cx: &mut Context<Self>) {
        let generation = self.self_update.download_generation.wrapping_add(1);
        self.self_update.download_generation = generation;
        self.self_update.self_update_status = update::SelfUpdateStatus::Downloading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            smol::Timer::after(DOWNLOAD_WATCHDOG).await;
            let _ = this.update(cx, |app, cx| {
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

    pub(crate) fn kickoff_self_update_install(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.self_update.self_update_status,
            update::SelfUpdateStatus::ReadyToRestart
        ) {
            log::info!("self-update: ReadyToRestart click - invoking cx.restart()");
            cx.restart();
            return;
        }

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

        if let update::install_method::InstallMethod::SystemPackage { manager } =
            &self.self_update.install_method
        {
            let version = match &self.self_update.update_status {
                Some(update::checker::UpdateStatus::Available { version, .. }) => version.clone(),
                _ => return,
            };

            if !is_strict_semver(&version) {
                log::warn!(
                    "self-update/system-package: refusing malformed version string: {version:?}"
                );
                self.show_toast("Update unavailable - invalid release tag".to_string(), cx);
                return;
            }

            if matches!(manager, update::install_method::PackageManager::RpmOstree) {
                cx.write_to_clipboard(ClipboardItem::new_string("rpm-ostree upgrade".to_string()));
                self.push_toast(
                    "PaneFlow detects an immutable distribution. Update must be run via `rpm-ostree upgrade` at the system level - the update has been copied to your clipboard.".to_string(),
                    Vec::new(),
                    TOAST_HOLD_MS * 4,
                    cx,
                );
                return;
            }

            #[cfg(not(target_os = "linux"))]
            let run_pkexec = false;
            #[cfg(target_os = "linux")]
            let run_pkexec = matches!(
                manager,
                update::install_method::PackageManager::Dnf
                    | update::install_method::PackageManager::Apt
                    | update::install_method::PackageManager::Zypper
            );

            if !run_pkexec {
                let command = system_package_update_command(Some(manager), &version);
                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                self.show_toast(format!("Copied: {command}"), cx);
                return;
            }

            #[cfg(target_os = "linux")]
            {
                let manager_owned = manager.clone();
                let manager_label: &'static str = match manager_owned {
                    update::install_method::PackageManager::Dnf => "dnf",
                    update::install_method::PackageManager::Apt => "apt",
                    update::install_method::PackageManager::Zypper => "zypper",
                    update::install_method::PackageManager::Other => "system-package",
                    update::install_method::PackageManager::RpmOstree => "rpm-ostree",
                };
                self.enter_downloading(manager_label, cx);

                cx.spawn(async move |this, cx| {
                    let result = smol::unblock({
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
                            let classified = update::UpdateError::classify(&err);
                            let _ = this.update(cx, |app, cx| match classified {
                                update::UpdateError::InstallDeclined { .. } => {
                                    app.self_update.self_update_status = update::SelfUpdateStatus::Idle;
                                    app.show_toast("Update cancelled".to_string(), cx);
                                    cx.notify();
                                }
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

        if self.self_update.update_attempt_count >= 3 {
            let releases_url = match &self.self_update.update_status {
                Some(update::checker::UpdateStatus::Available { url, .. }) => url.clone(),
                _ => "https://github.com/arthjean/paneflow/releases".to_string(),
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
                if let Err(err) = crate::external_open::open_url(url) {
                    log::warn!("self-update: open release page failed: {err}");
                }
                return;
            }
            _ => return,
        };

        if !update::signature::has_embedded_key() {
            self.push_toast(
                "This build can't self-update (unsigned). Download the latest version from the releases page.".to_string(),
                vec![ToastAction::OpenReleasesPage(
                    "https://github.com/arthjean/paneflow/releases".to_string(),
                )],
                TOAST_HOLD_MS * 4,
                cx,
            );
            return;
        }

        let method = self.self_update.install_method.clone();
        if let update::install_method::InstallMethod::AppImage { source_path, .. } = &method {
            let source_path = source_path.clone();
            self.enter_downloading("appimage", cx);

            let asset_url_for_verify = asset_url.clone();
            cx.spawn(async move |this, cx| {
                let result = smol::unblock({
                    let source_path = source_path.clone();
                    let asset_url = asset_url_for_verify.clone();
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

        let route_to_targz = matches!(&method, update::install_method::InstallMethod::TarGz { .. })
            || (unknown_install_uses_targz()
                && matches!(&method, update::install_method::InstallMethod::Unknown));
        if route_to_targz {
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

        if let update::install_method::InstallMethod::AppBundle { bundle_path } = &method {
            let url = asset_url.clone();
            let bundle = bundle_path.clone();
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

        let msg = anyhow::anyhow!(
            "Self-update dispatch did not handle install method {:?}. Download the new release manually from {asset_url}",
            method
        );
        self.record_update_failure("unsupported-dispatch", &msg, cx);
    }

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
