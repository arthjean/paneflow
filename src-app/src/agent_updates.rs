use std::path::{Path, PathBuf};
use std::time::Duration;

use paneflow_agent_config::RuntimeUpdate;

use crate::agent_launcher::TerminalAgent;

const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(6);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const UPDATE_STDOUT_CAP: u64 = 64 * 1024;
const PROGRESS_MAX_CHARS: usize = 120;
const SELF_UPDATER_VIA: &str = "its own updater";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdatePlan {
    pub(crate) via: &'static str,
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

impl UpdatePlan {
    pub(crate) fn display_command(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(quote_for_display)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UpdateMethod {
    Auto,
    SelfUpdate,
    Npm,
    Bun,
    Pnpm,
    Yarn,
    Script,
}

impl UpdateMethod {
    pub(crate) const ALL: [Self; 7] = [
        Self::Auto,
        Self::SelfUpdate,
        Self::Npm,
        Self::Bun,
        Self::Pnpm,
        Self::Yarn,
        Self::Script,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::SelfUpdate => "self",
            Self::Npm => "npm",
            Self::Bun => "bun",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Script => "script",
        }
    }

    pub(crate) fn from_id(id: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|method| Some(method.id()) == id)
            .unwrap_or(Self::Auto)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::SelfUpdate => "Built-in updater",
            Self::Npm => "npm",
            Self::Bun => "bun",
            Self::Pnpm => "pnpm",
            Self::Yarn => "Yarn",
            Self::Script if cfg!(windows) => "PowerShell installer",
            Self::Script => "curl installer",
        }
    }

    fn declared(self, update: RuntimeUpdate) -> bool {
        match self {
            Self::Auto => true,
            Self::SelfUpdate => update.self_update.is_some(),
            Self::Npm | Self::Bun | Self::Pnpm | Self::Yarn => update.npm_package.is_some(),
            Self::Script => script_url(update).is_some(),
        }
    }

    fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Auto | Self::SelfUpdate => &[],
            Self::Npm => &["npm"],
            Self::Bun => &["bun"],
            Self::Pnpm => &["pnpm"],
            Self::Yarn => &["yarn"],
            Self::Script if cfg!(windows) => &["powershell"],
            Self::Script => &["curl", "bash"],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AgentInspection {
    pub(crate) executable: Option<PathBuf>,
    pub(crate) latest: Option<String>,
    pub(crate) plans: Vec<(UpdateMethod, Result<UpdatePlan, String>)>,
}

impl AgentInspection {
    pub(crate) fn plan(&self, method: UpdateMethod) -> Option<&Result<UpdatePlan, String>> {
        self.plans
            .iter()
            .find(|(candidate, _)| *candidate == method)
            .map(|(_, plan)| plan)
    }

    pub(crate) fn offered_methods(&self, selected: UpdateMethod) -> Vec<UpdateMethod> {
        self.plans
            .iter()
            .filter(|(method, plan)| {
                *method == UpdateMethod::Auto || *method == selected || plan.is_ok()
            })
            .map(|(method, _)| *method)
            .collect()
    }

    pub(crate) fn installs_second_copy(&self, selected: UpdateMethod) -> bool {
        let (Some(Ok(auto)), Some(Ok(chosen))) =
            (self.plan(UpdateMethod::Auto), self.plan(selected))
        else {
            return false;
        };
        let reinstalls_native = selected == UpdateMethod::Script && auto.via == SELF_UPDATER_VIA;
        selected != UpdateMethod::Auto && auto.via != chosen.via && !reinstalls_native
    }
}

#[derive(Clone, Debug)]
pub(crate) enum UpdatePhase {
    Running { progress: Option<String> },
    Updated { version: Option<String> },
    Unchanged,
    Failed { message: String },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AgentUpdateState {
    pub(crate) inspection: Option<AgentInspection>,
    pub(crate) phase: Option<UpdatePhase>,
}

impl AgentUpdateState {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.phase, Some(UpdatePhase::Running { .. }))
    }

    pub(crate) fn newer_version(&self, installed: Option<&str>) -> Option<&str> {
        if matches!(self.phase, Some(UpdatePhase::Unchanged)) {
            return None;
        }
        let latest = self.inspection.as_ref()?.latest.as_deref()?;
        installed
            .is_some_and(|installed| version_is_newer(latest, installed))
            .then_some(latest)
    }

    pub(crate) fn is_up_to_date(&self, installed: Option<&str>) -> bool {
        match &self.phase {
            Some(UpdatePhase::Unchanged) => true,
            Some(UpdatePhase::Running { .. }) => false,
            _ => {
                self.inspection
                    .as_ref()
                    .is_some_and(|inspection| inspection.latest.is_some())
                    && installed.is_some()
                    && self.newer_version(installed).is_none()
            }
        }
    }

    pub(crate) fn runnable_plan(&self, method: UpdateMethod) -> Option<&UpdatePlan> {
        self.inspection.as_ref()?.plan(method)?.as_ref().ok()
    }
}

pub(crate) fn inspect(agent: TerminalAgent) -> AgentInspection {
    let update = agent.runtime().update;
    let Some(executable) = agent.real_executable() else {
        return AgentInspection::default();
    };
    let latest = update.npm_package.and_then(npm_latest_version);
    let real = std::fs::canonicalize(&executable).unwrap_or_else(|_| executable.clone());
    let plans = UpdateMethod::ALL
        .into_iter()
        .filter(|method| method.declared(update))
        .map(|method| {
            let missing = method
                .tools()
                .iter()
                .find(|tool| which::which(tool).is_err());
            let plan = match missing {
                Some(tool) => Err(format!("{tool} is not on PATH.")),
                None => method_plan(
                    method,
                    &executable,
                    &real,
                    update,
                    latest.as_deref(),
                    agent.display_name(),
                    &|path| path.is_dir(),
                ),
            };
            (method, plan)
        })
        .collect();
    AgentInspection {
        executable: Some(executable),
        latest,
        plans,
    }
}

pub(crate) fn method_plan(
    method: UpdateMethod,
    executable: &Path,
    real: &Path,
    update: RuntimeUpdate,
    latest: Option<&str>,
    label: &str,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Result<UpdatePlan, String> {
    let global = |via: &'static str, program: &str, args: &[&str]| {
        let package = update
            .npm_package
            .ok_or_else(|| format!("{label} is not published on npm."))?;
        Ok(UpdatePlan {
            via,
            program: program.into(),
            args: args
                .iter()
                .map(|arg| (*arg).to_string())
                .chain(std::iter::once(format!(
                    "{package}@{}",
                    latest.unwrap_or("latest")
                )))
                .collect(),
        })
    };
    match method {
        UpdateMethod::Auto => plan_update(executable, real, update, latest, label, is_dir),
        UpdateMethod::SelfUpdate => update
            .self_update
            .map(|argv| self_update_plan(executable, argv))
            .ok_or_else(|| format!("{label} has no built-in updater.")),
        UpdateMethod::Npm => global("npm", "npm", &["install", "-g"]),
        UpdateMethod::Bun => global("bun", "bun", &["add", "-g"]),
        UpdateMethod::Pnpm => global("pnpm", "pnpm", &["add", "-g"]),
        UpdateMethod::Yarn => global("Yarn", "yarn", &["global", "add"]),
        UpdateMethod::Script => {
            let url = script_url(update)
                .ok_or_else(|| format!("{label} has no official installer script."))?;
            Ok(script_plan(url, cfg!(windows)))
        }
    }
}

fn script_url(update: RuntimeUpdate) -> Option<&'static str> {
    if cfg!(windows) {
        update.powershell_script
    } else {
        update.curl_script
    }
}

fn script_plan(url: &str, windows: bool) -> UpdatePlan {
    if windows {
        UpdatePlan {
            via: "its PowerShell installer",
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                format!("irm {url} | iex"),
            ],
        }
    } else {
        UpdatePlan {
            via: "its curl installer",
            program: "bash".into(),
            args: vec!["-c".into(), format!("curl -fsSL {url} | bash")],
        }
    }
}

fn self_update_plan(executable: &Path, argv: &[&str]) -> UpdatePlan {
    UpdatePlan {
        via: SELF_UPDATER_VIA,
        program: executable.to_string_lossy().into_owned(),
        args: argv.iter().skip(1).map(|arg| (*arg).to_string()).collect(),
    }
}

pub(crate) fn run_update(
    plan: &UpdatePlan,
    mut on_progress: impl FnMut(String) + Send + 'static,
) -> Result<(), String> {
    let program =
        which::which(&plan.program).map_err(|_| format!("{} is not on PATH", plan.program))?;
    let mut command = std::process::Command::new(program);
    command.args(&plan.args);
    let mut pending = Vec::new();
    let output = paneflow_process::run_with_timeout_tapping_stderr(
        command,
        UPDATE_TIMEOUT,
        UPDATE_STDOUT_CAP,
        move |chunk| {
            pending.extend_from_slice(chunk);
            if let Some(line) = last_complete_line(&mut pending) {
                on_progress(line);
            }
        },
    )
    .map_err(|error| match error {
        paneflow_process::ProcError::Timeout => "the update timed out after 30 minutes".to_string(),
        error => error.to_string(),
    })?;
    if output.status.success() {
        return Ok(());
    }
    let reason = last_meaningful_line(&output.stderr)
        .or_else(|| last_meaningful_line(&output.stdout))
        .unwrap_or_else(|| format!("exited with {}", output.status));
    Err(reason)
}

pub(crate) fn plan_update(
    executable: &Path,
    real: &Path,
    update: RuntimeUpdate,
    latest: Option<&str>,
    label: &str,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Result<UpdatePlan, String> {
    let launcher = normalized(executable);
    let target = normalized(real);
    let anywhere = |needle: &str| launcher.contains(needle) || target.contains(needle);
    if anywhere("/mise/installs/") || anywhere("/mise/shims/") {
        return Err("Managed by mise: change the version mise pins.".into());
    }
    if anywhere("/microsoft/winget/") {
        return Err("Installed with WinGet: run winget upgrade.".into());
    }
    if anywhere("/chocolatey/") {
        return Err("Installed with Chocolatey: run choco upgrade as administrator.".into());
    }
    if let Some(package) = update.npm_package
        && let Some(plan) = package_manager_plan(executable, real, package, latest, is_dir)?
    {
        return Ok(plan);
    }
    if let Some((name, cask)) = homebrew_package(&target) {
        let mut args = vec!["upgrade".to_string()];
        if cask {
            args.push("--cask".into());
        }
        args.push(name);
        return Ok(UpdatePlan {
            via: "Homebrew",
            program: "brew".into(),
            args,
        });
    }
    if let Some(argv) = update.self_update {
        return Ok(self_update_plan(executable, argv));
    }
    Err(format!("Update {label} with the installer you used."))
}

fn package_manager_plan(
    executable: &Path,
    real: &Path,
    package: &str,
    latest: Option<&str>,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Result<Option<UpdatePlan>, String> {
    let launcher = normalized(executable);
    let target = normalized(real);
    let spec = format!("{package}@{}", latest.unwrap_or("latest"));
    let installed_segment = format!("/node_modules/{}/", package.to_ascii_lowercase());
    let plan = |via: &'static str, program: &str, args: &[&str]| {
        Ok(Some(UpdatePlan {
            via,
            program: program.into(),
            args: args
                .iter()
                .map(|arg| (*arg).to_string())
                .chain(std::iter::once(spec.clone()))
                .collect(),
        }))
    };

    if let Some(root) = launcher
        .find("/.bun/bin/")
        .map(|at| executable_text(executable)[..at].to_string())
    {
        let global = Path::new(&root)
            .join(".bun")
            .join("install")
            .join("global")
            .join("node_modules")
            .join(package);
        if is_dir(&global) || target.contains(&installed_segment) {
            return plan("bun", "bun", &["add", "-g"]);
        }
    }
    if launcher.contains("/.volta/bin/") || launcher.contains("/volta/bin/") {
        return plan("Volta", "volta", &["install"]);
    }
    let windows_prefix = executable
        .parent()
        .filter(|dir| is_dir(&dir.join("node_modules").join(package)));
    let under_node_modules = target.contains(&installed_segment) || windows_prefix.is_some();
    if !under_node_modules {
        return Ok(None);
    }
    if has_segment(&launcher, "pnpm") || has_segment(&target, "pnpm") || target.contains("/.pnpm/")
    {
        return plan("pnpm", "pnpm", &["add", "-g"]);
    }
    if target.contains("/yarn/global/node_modules/") || launcher.contains("/yarn/bin/") {
        return plan("Yarn", "yarn", &["global", "add"]);
    }
    if let Some(prefix) = windows_prefix {
        let prefix = prefix.to_string_lossy().into_owned();
        return plan("npm", "npm", &["install", "-g", "--prefix", &prefix]);
    }
    let lib_segment = format!("/lib{installed_segment}");
    if let Some(at) = target.find(&lib_segment) {
        let prefix = executable_text(real)[..at].to_string();
        let prefix = if prefix.is_empty() {
            "/".to_string()
        } else {
            prefix
        };
        return plan("npm", "npm", &["install", "-g", "--prefix", &prefix]);
    }
    Err("Installed inside a project's node_modules: update it from that project.".into())
}

fn homebrew_package(target: &str) -> Option<(String, bool)> {
    for (marker, cask) in [("/cellar/", false), ("/caskroom/", true)] {
        if let Some(at) = target.find(marker) {
            let name = target[at + marker.len()..].split('/').next()?;
            if !name.is_empty() {
                return Some((name.to_string(), cask));
            }
        }
    }
    None
}

fn executable_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalized(path: &Path) -> String {
    executable_text(path).to_ascii_lowercase()
}

fn has_segment(path: &str, segment: &str) -> bool {
    path.split('/').any(|part| part == segment)
}

fn quote_for_display(token: &str) -> String {
    if !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:@=\\".contains(&b))
    {
        token.to_string()
    } else {
        format!("\"{token}\"")
    }
}

fn npm_latest_version(package: &str) -> Option<String> {
    let url = format!("{NPM_REGISTRY}/{}/latest", package.replace('/', "%2F"));
    let mut response = ureq::get(&url)
        .config()
        .timeout_global(Some(REGISTRY_TIMEOUT))
        .build()
        .header("Accept", "application/json")
        .call()
        .map_err(|error| tracing::debug!(target: "paneflow_app::agent_updates", %package, "npm registry lookup failed: {error}"))
        .ok()?;
    let body: serde_json::Value = response.body_mut().read_json().ok()?;
    body.get("version")?.as_str().map(str::to_string)
}

pub(crate) fn version_is_newer(candidate: &str, installed: &str) -> bool {
    match (version_numbers(candidate), version_numbers(installed)) {
        (Some(candidate), Some(installed)) => candidate > installed,
        _ => false,
    }
}

fn version_numbers(version: &str) -> Option<Vec<u64>> {
    let core = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+', ' '])
        .next()?;
    let numbers = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (numbers.len() >= 2).then_some(numbers)
}

fn last_complete_line(pending: &mut Vec<u8>) -> Option<String> {
    let end = pending
        .iter()
        .rposition(|byte| matches!(byte, b'\n' | b'\r'))?;
    let complete: Vec<u8> = pending.drain(..=end).collect();
    last_meaningful_line(&complete)
}

fn last_meaningful_line(bytes: &[u8]) -> Option<String> {
    let text = strip_ansi(&String::from_utf8_lossy(bytes));
    text.split(['\n', '\r'])
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(PROGRESS_MAX_CHARS).collect())
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() || next == '~' {
                    break;
                }
            }
        } else if chars.next_if_eq(&']').is_some() {
            while let Some(next) = chars.next() {
                if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: RuntimeUpdate = RuntimeUpdate {
        npm_package: Some("@anthropic-ai/claude-code"),
        self_update: Some(&["claude", "update"]),
        curl_script: Some("https://claude.ai/install.sh"),
        powershell_script: Some("https://claude.ai/install.ps1"),
    };
    const CODEX: RuntimeUpdate = RuntimeUpdate {
        npm_package: Some("@openai/codex"),
        self_update: None,
        curl_script: None,
        powershell_script: None,
    };

    fn explicit(method: UpdateMethod, update: RuntimeUpdate) -> Result<UpdatePlan, String> {
        let exe = Path::new("/home/u/.local/bin/agent");
        method_plan(method, exe, exe, update, Some("9.9.9"), "Agent", &|_| false)
    }

    #[test]
    fn an_explicit_package_manager_installs_the_package_globally_with_that_tool() {
        let cases = [
            (UpdateMethod::Npm, "npm install -g @openai/codex@9.9.9"),
            (UpdateMethod::Bun, "bun add -g @openai/codex@9.9.9"),
            (UpdateMethod::Pnpm, "pnpm add -g @openai/codex@9.9.9"),
            (UpdateMethod::Yarn, "yarn global add @openai/codex@9.9.9"),
        ];
        for (method, command) in cases {
            assert_eq!(explicit(method, CODEX).unwrap().display_command(), command);
        }
        assert!(explicit(UpdateMethod::SelfUpdate, CODEX).is_err());
        assert!(explicit(UpdateMethod::Script, CODEX).is_err());
    }

    #[test]
    fn the_installer_script_runs_through_powershell_on_windows_and_curl_elsewhere() {
        let windows = script_plan("https://claude.ai/install.ps1", true);
        assert_eq!(windows.program, "powershell");
        assert_eq!(
            windows.args.last().unwrap(),
            "irm https://claude.ai/install.ps1 | iex"
        );
        let unix = script_plan("https://claude.ai/install.sh", false);
        assert_eq!(unix.program, "bash");
        assert_eq!(
            unix.args,
            ["-c", "curl -fsSL https://claude.ai/install.sh | bash"]
        );
        assert!(explicit(UpdateMethod::Script, CLAUDE).is_ok());
    }

    #[test]
    fn method_ids_round_trip_and_unknown_ones_fall_back_to_automatic() {
        for method in UpdateMethod::ALL {
            assert_eq!(UpdateMethod::from_id(Some(method.id())), method);
        }
        assert_eq!(UpdateMethod::from_id(Some("brew")), UpdateMethod::Auto);
        assert_eq!(UpdateMethod::from_id(None), UpdateMethod::Auto);
    }

    #[test]
    fn a_second_copy_is_flagged_only_when_the_tool_differs_from_the_owner() {
        let plan_via = |via: &'static str| {
            Ok(UpdatePlan {
                via,
                program: "x".into(),
                args: Vec::new(),
            })
        };
        let inspection = AgentInspection {
            executable: None,
            latest: None,
            plans: vec![
                (UpdateMethod::Auto, plan_via(SELF_UPDATER_VIA)),
                (UpdateMethod::SelfUpdate, plan_via(SELF_UPDATER_VIA)),
                (UpdateMethod::Bun, plan_via("bun")),
                (UpdateMethod::Script, plan_via("its curl installer")),
            ],
        };
        assert!(inspection.installs_second_copy(UpdateMethod::Bun));
        assert!(!inspection.installs_second_copy(UpdateMethod::SelfUpdate));
        assert!(!inspection.installs_second_copy(UpdateMethod::Script));
        assert!(!inspection.installs_second_copy(UpdateMethod::Auto));
    }

    #[test]
    fn an_agent_is_up_to_date_only_once_the_latest_release_is_known() {
        let mut state = AgentUpdateState {
            inspection: Some(AgentInspection {
                executable: None,
                latest: Some("2.1.316".into()),
                plans: Vec::new(),
            }),
            phase: None,
        };
        assert!(state.is_up_to_date(Some("2.1.316")));
        assert!(!state.is_up_to_date(Some("2.1.300")));
        assert!(!state.is_up_to_date(None));
        state.inspection.as_mut().unwrap().latest = None;
        assert!(!state.is_up_to_date(Some("2.1.316")));
        state.phase = Some(UpdatePhase::Unchanged);
        assert!(state.is_up_to_date(Some("2.1.316")));
    }

    fn plan(
        executable: &str,
        real: &str,
        update: RuntimeUpdate,
        dirs: &[&str],
    ) -> Result<UpdatePlan, String> {
        let dirs: Vec<String> = dirs.iter().map(|dir| dir.replace('\\', "/")).collect();
        plan_update(
            Path::new(executable),
            Path::new(real),
            update,
            Some("9.9.9"),
            "Agent",
            &|path| dirs.contains(&path.to_string_lossy().replace('\\', "/")),
        )
    }

    #[test]
    fn a_native_install_uses_its_own_updater_with_the_resolved_launcher() {
        let exe = "C:/Users/u/.local/bin/claude.exe";
        let plan = plan(exe, exe, CLAUDE, &[]).unwrap();
        assert_eq!(plan.via, "its own updater");
        assert_eq!(plan.program, exe);
        assert_eq!(plan.args, ["update"]);
    }

    #[test]
    fn a_bun_global_is_updated_by_bun_pinned_to_the_latest_release() {
        let exe = "C:/Users/u/.bun/bin/codex.exe";
        let plan = plan(
            exe,
            exe,
            CODEX,
            &["C:/Users/u/.bun/install/global/node_modules/@openai/codex"],
        )
        .unwrap();
        assert_eq!(plan.display_command(), "bun add -g @openai/codex@9.9.9");
    }

    #[test]
    fn a_windows_npm_shim_is_updated_into_its_own_prefix() {
        let plan = plan(
            "C:/Users/u/AppData/Roaming/npm/codex.cmd",
            "C:/Users/u/AppData/Roaming/npm/codex.cmd",
            CODEX,
            &["C:/Users/u/AppData/Roaming/npm/node_modules/@openai/codex"],
        )
        .unwrap();
        assert_eq!(plan.via, "npm");
        assert_eq!(
            plan.args,
            [
                "install",
                "-g",
                "--prefix",
                "C:/Users/u/AppData/Roaming/npm",
                "@openai/codex@9.9.9"
            ]
        );
    }

    #[test]
    fn a_unix_npm_global_resolves_its_prefix_even_under_homebrew_node() {
        let plan = plan(
            "/opt/homebrew/bin/claude",
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            CLAUDE,
            &[],
        )
        .unwrap();
        assert_eq!(
            plan.display_command(),
            "npm install -g --prefix /opt/homebrew @anthropic-ai/claude-code@9.9.9"
        );
    }

    #[test]
    fn pnpm_yarn_and_volta_globals_go_through_their_owner() {
        let pnpm = plan(
            "/home/u/.local/share/pnpm/codex",
            "/home/u/.local/share/pnpm/global/5/node_modules/@openai/codex/bin/codex.js",
            CODEX,
            &[],
        )
        .unwrap();
        assert_eq!(pnpm.display_command(), "pnpm add -g @openai/codex@9.9.9");
        let yarn = plan(
            "/home/u/.yarn/bin/codex",
            "/home/u/.config/yarn/global/node_modules/@openai/codex/bin/codex.js",
            CODEX,
            &[],
        )
        .unwrap();
        assert_eq!(
            yarn.display_command(),
            "yarn global add @openai/codex@9.9.9"
        );
        let volta = plan(
            "/home/u/.volta/bin/codex",
            "/home/u/.volta/bin/volta-shim",
            CODEX,
            &[],
        )
        .unwrap();
        assert_eq!(volta.display_command(), "volta install @openai/codex@9.9.9");
    }

    #[test]
    fn homebrew_kegs_and_casks_are_upgraded_by_name() {
        let cask = plan(
            "/opt/homebrew/bin/codex",
            "/opt/homebrew/Caskroom/codex/0.166.0/codex-aarch64-apple-darwin",
            CODEX,
            &[],
        )
        .unwrap();
        assert_eq!(cask.display_command(), "brew upgrade --cask codex");
        let keg = plan(
            "/usr/local/bin/claude",
            "/usr/local/Cellar/claude-code/2.1.0/bin/claude",
            CLAUDE,
            &[],
        )
        .unwrap();
        assert_eq!(keg.display_command(), "brew upgrade claude-code");
    }

    #[test]
    fn installs_paneflow_cannot_own_are_left_to_the_user_with_the_reason() {
        let mise = plan(
            "/home/u/.local/share/mise/installs/node/22/bin/codex",
            "/home/u/.local/share/mise/installs/node/22/lib/node_modules/@openai/codex/bin/codex.js",
            CODEX,
            &[],
        );
        assert!(mise.unwrap_err().contains("mise"));
        let winget = plan(
            "C:/Users/u/AppData/Local/Microsoft/WinGet/Links/claude.exe",
            "C:/Users/u/AppData/Local/Microsoft/WinGet/Packages/Anthropic.ClaudeCode/claude.exe",
            CLAUDE,
            &[],
        );
        assert!(winget.unwrap_err().contains("WinGet"));
        let project = plan(
            "/work/app/node_modules/.bin/codex",
            "/work/app/node_modules/@openai/codex/bin/codex.js",
            CODEX,
            &[],
        );
        assert!(project.unwrap_err().contains("project"));
        let unknown = plan("/usr/bin/codex", "/usr/bin/codex", CODEX, &[]);
        assert_eq!(
            unknown.unwrap_err(),
            "Update Agent with the installer you used."
        );
    }

    #[test]
    fn versions_compare_numerically_and_ignore_unparsable_ones() {
        assert!(version_is_newer("2.1.320", "2.1.316"));
        assert!(version_is_newer("0.167.0", "0.166.0"));
        assert!(version_is_newer("1.10.0", "1.9.9"));
        assert!(!version_is_newer("2.1.316", "2.1.316"));
        assert!(!version_is_newer("1.4.0", "v1.4.0"));
        assert!(!version_is_newer("nightly", "1.0.0"));
    }

    #[test]
    fn progress_is_the_last_complete_line_without_escape_sequences() {
        let mut pending = b"\x1b[32mresolving\x1b[0m\r\nadded 3 packages\npartial".to_vec();
        assert_eq!(
            last_complete_line(&mut pending).as_deref(),
            Some("added 3 packages")
        );
        assert_eq!(pending, b"partial");
        assert_eq!(last_complete_line(&mut pending), None);
        assert_eq!(
            last_meaningful_line(b"\x1b]0;title\x07npm ERR! code EBUSY\n\n").as_deref(),
            Some("npm ERR! code EBUSY")
        );
    }

    #[test]
    fn display_commands_quote_paths_with_spaces() {
        let plan = UpdatePlan {
            via: "npm",
            program: "npm".into(),
            args: vec!["--prefix".into(), "C:/Program Files/nodejs".into()],
        };
        assert_eq!(
            plan.display_command(),
            "npm --prefix \"C:/Program Files/nodejs\""
        );
    }
}
