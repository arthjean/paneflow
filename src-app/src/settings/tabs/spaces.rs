use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px, svg,
};
use paneflow_config::schema::{
    DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceConfig, SpaceId, validate_space_name,
};
use paneflow_mcp_install::IntegrationStatus;

use std::collections::BTreeMap;

use crate::PaneFlowApp;
use crate::app::spaces::SPACE_LIMIT_REASON;
use crate::settings::components::{
    KeyboardStopExt, input_frame, select_menu_surface, settings_breadcrumb, with_alpha,
};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::Block;
use crate::settings::tabs::agents::{
    EnvRowInputs, collect_env, input_box, new_env_row, validate_env_values,
};
use crate::settings::tabs::space_accounts::{is_account_key, other_variable_count, same_key};
use crate::ui_primitives::{
    AccessibleControlExt, TITLE, TooltipDelayExt, lerp_color, text_tooltip,
};

const IDENTITY_PADDING: f32 = 16.;
const IDENTITY_GAP: f32 = 18.;
const IDENTITY_MARK: f32 = 56.;
const AVATAR_ICON_SIZE: f32 = 18.;
const AVATAR_SCRIM_ALPHA: f32 = 0.5;
const AVATAR_BADGE: f32 = 22.;
const AVATAR_BADGE_RING: f32 = 2.;
const AVATAR_BADGE_ICON: f32 = 11.;
const AVATAR_BADGE_OFFSET: f32 = 2.;
const NAME_FIELD_WIDTH: f32 = 320.;
const NAME_FIELD_HEIGHT: f32 = 32.;
const PICKER_TARGET: f32 = 24.;
const PICKER_SWATCH: f32 = 16.;
const ENV_NAME_WIDTH: f32 = 180.;
const WELCOME_MARK: f32 = 36.;
const WELCOME_MARK_GAP: f32 = 8.;
const PRESET_HEIGHT: f32 = 32.;
const PRESET_RADIUS: f32 = 8.;
const ENV_ROW_PADDING_Y: f32 = 10.;
const WELCOME_TEXT_WIDTH: f32 = 380.;
const PRESET_MARK: f32 = 14.;
const PRESET_PADDING_X: f32 = 12.;
const SECRET_MASK: &str = "••••••••";
const ADD_ICON: &str = "icons/settings/add-01.svg";
const EDIT_ICON: &str = "icons/settings/pencil-edit-02.svg";
const DELETE_ICON: &str = "icons/settings/delete-02.svg";
const ALERT_ICON: &str = "icons/settings/alert-02.svg";
const CANCEL_ICON: &str = "icons/settings/cancel-circle.svg";
const SPACES_INTRO: &str =
    "Group workspaces by context, each with its own agent accounts and terminal environment.";
const WELCOME_TITLE: &str = "Keep your contexts apart";
const WELCOME_BODY: &str = "A space groups workspaces, such as work or personal projects, and can \
     sign their agents into a separate account.";
const ENV_HINT: &str = "Variables every terminal in this space gets, for example \
     AWS_PROFILE for a separate cloud profile.";
const ENV_EDITOR_INTRO: &str = "Every terminal in this space gets these variables. ~ stands for \
     your home folder, and $NAME reuses a variable from Paneflow's environment.";
const ENV_NAME_EXAMPLE: &str = "AWS_PROFILE";
const ENV_VALUE_EXAMPLE: &str = "work";
const SPACE_PRESETS: [(&str, SpaceColor); 3] = [
    ("Work", SpaceColor::Blue),
    ("Personal", SpaceColor::Green),
    ("Client", SpaceColor::Magenta),
];

pub(crate) struct SpaceNameEditor {
    pub(crate) id: SpaceId,
    pub(crate) error: Option<&'static str>,
}

pub(crate) struct SpaceEnvEditor {
    pub(crate) id: SpaceId,
    pub(crate) rows: Vec<EnvRowInputs>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintTone {
    Error,
    Warning,
    Preview,
}

pub(crate) fn validated_space_env(
    rows: &[(String, String)],
    home: Option<&std::path::Path>,
) -> Result<BTreeMap<String, String>, String> {
    let env = collect_env(rows)?;
    if let Some((key, reason)) = env
        .keys()
        .find_map(|key| crate::space_env::env_key_error(key).map(|reason| (key, reason)))
    {
        return Err(format!("'{key}': {reason}"));
    }
    validate_env_values(&env, home)?;
    for (key, value) in &env {
        let expanded = crate::env_expand::expand_with_process_env(value, home)
            .unwrap_or_else(|_| value.clone());
        if let Some(reason) = crate::space_env::env_value_error(key, &expanded) {
            return Err(format!("'{key}': {reason}"));
        }
    }
    Ok(env)
}

pub(crate) fn env_row_hint(
    key: &str,
    value: &str,
    home: Option<&std::path::Path>,
) -> Option<(String, HintTone)> {
    let (key, value) = (key.trim(), value.trim());
    if !key.is_empty()
        && let Some(reason) = crate::space_env::env_key_error(key)
    {
        return Some((reason.to_string(), HintTone::Error));
    }
    if value.is_empty() {
        return None;
    }
    let expanded = match crate::env_expand::expand_with_process_env(value, home) {
        Ok(expanded) => expanded,
        Err(name) => {
            return Some((
                format!("{name} is not set in your environment"),
                HintTone::Error,
            ));
        }
    };
    if let Some(reason) = crate::space_env::env_value_error(key, &expanded) {
        return Some((reason.to_string(), HintTone::Error));
    }
    if crate::space_env::stores_secret(key) {
        return Some((
            crate::space_env::PLAIN_TEXT_WARNING.to_string(),
            HintTone::Warning,
        ));
    }
    (expanded != value).then_some((expanded, HintTone::Preview))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpaceWarning {
    pub(crate) text: String,
    pub(crate) turn_off: Option<&'static str>,
}

pub(crate) fn space_env_warnings(
    env: &BTreeMap<String, String>,
    inherited: impl Fn(&str) -> bool,
) -> Vec<SpaceWarning> {
    let mut warnings: Vec<SpaceWarning> = crate::space_env::shadowing_credentials(env, inherited)
        .into_iter()
        .map(|(key, tool)| SpaceWarning {
            text: format!(
                "{key} is set in Paneflow's environment, so {tool} uses it instead of this space's account."
            ),
            turn_off: Some(key),
        })
        .collect();
    warnings.extend(
        env.iter()
            .filter(|(key, value)| !value.trim().is_empty() && crate::space_env::stores_secret(key))
            .map(|(key, _)| SpaceWarning {
                text: format!("{key}: {}", crate::space_env::PLAIN_TEXT_WARNING),
                turn_off: None,
            }),
    );
    warnings
}

#[derive(Default)]
pub(crate) struct SpaceAvatarImport {
    pub(crate) busy: Option<SpaceId>,
    pub(crate) error: Option<(SpaceId, String)>,
}

pub(crate) type IntegrationResults = Vec<(String, Result<(), String>)>;

#[derive(Default)]
pub(crate) struct SpaceIntegrations {
    pub(crate) status: std::collections::HashMap<SpaceId, Vec<IntegrationStatus>>,
    pub(crate) errors: std::collections::HashMap<(SpaceId, String), String>,
    pub(crate) busy: Option<(SpaceId, String)>,
}

impl SpaceIntegrations {
    pub(crate) fn record(&mut self, space: &SpaceId, results: IntegrationResults) {
        for (slug, result) in results {
            let key = (space.clone(), slug);
            match result {
                Ok(()) => {
                    self.errors.remove(&key);
                }
                Err(error) => {
                    log::warn!(
                        "spaces: space {space}: {} integration failed: {error}",
                        key.1
                    );
                    self.errors.insert(key, error);
                }
            }
        }
    }
}

fn space_integration_statuses(
    config: &paneflow_config::schema::PaneFlowConfig,
) -> std::collections::HashMap<SpaceId, Vec<IntegrationStatus>> {
    config
        .spaces
        .iter()
        .filter_map(|space| {
            crate::space_env::integration_target(config, &space.id).map(|target| {
                (
                    space.id.clone(),
                    paneflow_mcp_install::list_target_integrations(&target),
                )
            })
        })
        .collect()
}

fn integration_binaries() -> Result<paneflow_mcp_install::IntegrationBinaries, String> {
    Ok(paneflow_mcp_install::IntegrationBinaries {
        hook_binary: crate::ai_hooks::extract::ensure_ai_hook_extracted()
            .map_err(|error| format!("hook extraction failed: {error:#}"))?,
        bridge_binary: crate::ai_hooks::extract::ensure_bridge_extracted()
            .map_err(|error| format!("bridge extraction failed: {error:#}"))?,
    })
}

fn failed_for_every_runtime(
    target: &paneflow_mcp_install::IntegrationTarget,
    error: &str,
) -> IntegrationResults {
    paneflow_mcp_install::list_target_integrations(target)
        .into_iter()
        .map(|status| (status.slug.to_string(), Err(error.to_string())))
        .collect()
}

pub(crate) fn mirror_into_space(
    config: &paneflow_config::schema::PaneFlowConfig,
    space: &SpaceId,
) -> IntegrationResults {
    let Some(target) = crate::space_env::integration_target(config, space) else {
        return Vec::new();
    };
    let installed = paneflow_mcp_install::list_integrations();
    match integration_binaries() {
        Ok(binaries) => {
            paneflow_mcp_install::mirror_installed_integrations(&installed, &target, &binaries)
        }
        Err(error) => failed_for_every_runtime(&target, &error),
    }
}

pub(crate) fn launch_refresh_groups(
    targets: Vec<(SpaceId, paneflow_mcp_install::IntegrationTarget)>,
) -> Vec<Vec<(SpaceId, paneflow_mcp_install::IntegrationTarget)>> {
    let mut groups: Vec<Vec<(SpaceId, paneflow_mcp_install::IntegrationTarget)>> = Vec::new();
    for (space, target) in targets {
        match groups
            .iter_mut()
            .find(|group| group[0].1.same_directories(&target))
        {
            Some(group) => group.push((space, target)),
            None => groups.push(vec![(space, target)]),
        }
    }
    groups
}

fn refresh_at_launch(
    config: &paneflow_config::schema::PaneFlowConfig,
) -> Vec<(SpaceId, IntegrationResults)> {
    let targets: Vec<_> = config
        .spaces
        .iter()
        .filter_map(|space| {
            crate::space_env::integration_target(config, &space.id)
                .map(|target| (space.id.clone(), target))
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let binaries = integration_binaries();
    launch_refresh_groups(targets)
        .into_iter()
        .flat_map(|group| {
            let results = match &binaries {
                Ok(binaries) => paneflow_mcp_install::refresh_target_integrations(
                    &group
                        .iter()
                        .map(|(_, target)| target.clone())
                        .collect::<Vec<_>>(),
                    binaries,
                ),
                Err(error) => failed_for_every_runtime(&group[0].1, error),
            };
            group
                .into_iter()
                .map(move |(space, _)| (space, results.clone()))
        })
        .collect()
}

pub(crate) const SPACE_COLORS: [SpaceColor; 6] = [
    SpaceColor::Red,
    SpaceColor::Green,
    SpaceColor::Yellow,
    SpaceColor::Blue,
    SpaceColor::Magenta,
    SpaceColor::Cyan,
];

pub(crate) fn space_color_name(color: SpaceColor) -> &'static str {
    match color {
        SpaceColor::Red => "Red",
        SpaceColor::Green => "Green",
        SpaceColor::Yellow => "Yellow",
        SpaceColor::Blue => "Blue",
        SpaceColor::Magenta => "Magenta",
        SpaceColor::Cyan => "Cyan",
    }
}

pub(crate) fn space_delete_message(workspaces: usize) -> String {
    match workspaces {
        0 => "It has no workspaces".to_string(),
        1 => "Its 1 workspace moves to Default".to_string(),
        n => format!("Its {n} workspaces move to Default"),
    }
}

pub(crate) fn renamed_spaces(
    spaces: &[SpaceConfig],
    id: &SpaceId,
    name: &str,
) -> Result<Option<Vec<SpaceConfig>>, &'static str> {
    let others = spaces
        .iter()
        .filter(|space| &space.id != id)
        .map(|space| space.name.as_str());
    let name = validate_space_name(name, others).map_err(|error| error.message())?;
    let mut next = spaces.to_vec();
    let Some(space) = next.iter_mut().find(|space| &space.id == id) else {
        return Ok(None);
    };
    if space.name == name {
        return Ok(None);
    }
    space.name = name;
    Ok(Some(next))
}

impl PaneFlowApp {
    pub(crate) fn edit_space_name(
        &mut self,
        id: SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .cached_config
            .space(&id)
            .map(|space| space.name.clone())
        else {
            return;
        };
        self.space_name_input.update(cx, |input, cx| {
            input.set_value(SharedString::from(name), cx)
        });
        self.space_detail = Some(id.clone());
        self.space_name_editor = Some(SpaceNameEditor { id, error: None });
        self.space_delete_armed = None;
        let focus = self.space_name_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn begin_space_name_edit(&mut self, id: SpaceId, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .space_name_editor
            .as_ref()
            .is_some_and(|editor| editor.id == id)
        {
            return;
        }
        self.space_name_editor = Some(SpaceNameEditor { id, error: None });
        self.space_delete_armed = None;
        let focus = self.space_name_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn cancel_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_name_editor = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn commit_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.space_name_editor.as_ref() else {
            return;
        };
        let id = editor.id.clone();
        let name = self.space_name_input.read(cx).value();
        match renamed_spaces(&self.cached_config.spaces, &id, &name) {
            Err(message) => {
                if let Some(editor) = self.space_name_editor.as_mut() {
                    editor.error = Some(message);
                }
                cx.notify();
            }
            Ok(next) => {
                self.space_name_editor = None;
                if let Some(spaces) = next {
                    self.persist_spaces(spaces, cx);
                }
                if self
                    .space_name_input
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
                {
                    self.settings_focus.focus(window, cx);
                }
                cx.notify();
            }
        }
    }

    fn set_space_color(&mut self, id: &SpaceId, color: Option<SpaceColor>, cx: &mut Context<Self>) {
        let mut spaces = self.cached_config.spaces.clone();
        let Some(space) = spaces.iter_mut().find(|space| &space.id == id) else {
            return;
        };
        if space.color == color {
            return;
        }
        space.color = color;
        self.persist_spaces(spaces, cx);
    }

    fn delete_space(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        self.space_delete_armed = None;
        if self.space_detail.as_ref() == Some(id) {
            self.space_detail = None;
        }
        if self
            .space_env_editor
            .as_ref()
            .is_some_and(|editor| &editor.id == id)
        {
            self.space_env_editor = None;
        }
        if self
            .space_name_editor
            .as_ref()
            .is_some_and(|editor| &editor.id == id)
        {
            self.space_name_editor = None;
        }
        let spaces: Vec<SpaceConfig> = self
            .cached_config
            .spaces
            .iter()
            .filter(|space| &space.id != id)
            .cloned()
            .collect();
        let avatar = self
            .cached_config
            .space(id)
            .and_then(|space| space.avatar.clone());
        self.persist_spaces(spaces, cx);
        if let Some(avatar) = avatar {
            discard_avatar_in_background(avatar, cx);
        }
    }

    fn choose_space_avatar(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        if self.space_avatar_import.busy.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose image".into()),
        });
        self.space_avatar_import = SpaceAvatarImport {
            busy: Some(id.clone()),
            error: None,
        };
        cx.notify();
        let space = id.clone();
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(mut paths))) => match paths.pop() {
                    Some(source) => {
                        let target = space.clone();
                        Some(
                            smol::unblock(move || {
                                crate::space_avatar::import_avatar(&source, &target)
                            })
                            .await,
                        )
                    }
                    None => None,
                },
                Ok(Err(error)) => Some(Err(format!("Unable to open the file picker: {error}"))),
                Ok(Ok(None)) | Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.space_avatar_import.busy = None;
                match result {
                    Some(Ok(file_name)) => this.set_space_avatar(&space, Some(file_name), cx),
                    Some(Err(error)) => this.space_avatar_import.error = Some((space, error)),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_space_avatar(&mut self, id: &SpaceId, avatar: Option<String>, cx: &mut Context<Self>) {
        let mut spaces = self.cached_config.spaces.clone();
        let Some(space) = spaces.iter_mut().find(|space| &space.id == id) else {
            if let Some(orphan) = avatar {
                discard_avatar_in_background(orphan, cx);
            }
            return;
        };
        if space.avatar == avatar {
            return;
        }
        let previous = std::mem::replace(&mut space.avatar, avatar);
        self.space_avatar_import.error = None;
        self.persist_spaces(spaces, cx);
        if let Some(previous) = previous {
            discard_avatar_in_background(previous, cx);
        }
    }

    pub(crate) fn refresh_space_integration_status(&self, cx: &mut Context<Self>) {
        let config = self.cached_config.clone();
        cx.spawn(async move |this, cx| {
            let status = smol::unblock(move || space_integration_statuses(&config)).await;
            let _ = this.update(cx, |this, cx| {
                this.space_integrations.status = status;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn refresh_space_integrations_at_launch(&self, cx: &mut Context<Self>) {
        if cfg!(debug_assertions) {
            log::info!(
                "spaces: a debug build leaves the space integrations untouched; Settings > Spaces > Retry installs them"
            );
            return;
        }
        let config = self.cached_config.clone();
        cx.spawn(async move |this, cx| {
            let results = smol::unblock(move || refresh_at_launch(&config)).await;
            let _ = this.update(cx, |this, cx| {
                for (space, results) in results {
                    this.space_integrations.record(&space, results);
                }
                this.refresh_space_integration_status(cx);
            });
        })
        .detach();
    }

    pub(crate) fn mirror_integrations_into_space(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        let config = self.cached_config.clone();
        let space = id.clone();
        cx.spawn(async move |this, cx| {
            let target = space.clone();
            let results = smol::unblock(move || mirror_into_space(&config, &target)).await;
            let _ = this.update(cx, |this, cx| {
                this.space_integrations.record(&space, results);
                this.refresh_space_integration_status(cx);
            });
        })
        .detach();
    }

    pub(crate) fn retry_space_integration(
        &mut self,
        id: &SpaceId,
        slug: &str,
        cx: &mut Context<Self>,
    ) {
        if self.space_integrations.busy.is_some() {
            return;
        }
        let Some(target) = crate::space_env::integration_target(&self.cached_config, id) else {
            return;
        };
        let (space, slug) = (id.clone(), slug.to_string());
        self.space_integrations.busy = Some((space.clone(), slug.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let install_slug = slug.clone();
            let result = smol::unblock(move || {
                let binaries = integration_binaries()?;
                paneflow_mcp_install::install_target_integration(&target, &install_slug, &binaries)
                    .map(|_| ())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.space_integrations.busy = None;
                this.space_integrations.record(&space, vec![(slug, result)]);
                this.refresh_space_integration_status(cx);
            });
        })
        .detach();
    }

    fn open_space_env_editor(&mut self, id: &SpaceId, window: &mut Window, cx: &mut Context<Self>) {
        let env = self
            .cached_config
            .space(id)
            .map(|space| space.env.clone())
            .unwrap_or_default();
        let mut rows: Vec<EnvRowInputs> = env
            .iter()
            .filter(|(key, _)| !is_account_key(key))
            .map(|(key, value)| space_env_row(key, value, cx))
            .collect();
        if rows.is_empty() {
            rows.push(space_env_row("", "", cx));
        }
        let first = rows[0].key.read(cx).focus_handle.clone();
        self.space_env_editor = Some(SpaceEnvEditor {
            id: id.clone(),
            rows,
            error: None,
        });
        self.space_delete_armed = None;
        self.space_account_menu = None;
        window.focus(&first, cx);
        cx.notify();
    }

    fn save_space_env(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.space_env_editor.as_ref() else {
            return;
        };
        let id = editor.id.clone();
        let rows: Vec<(String, String)> = editor
            .rows
            .iter()
            .map(|row| (row.key.read(cx).value(), row.value.read(cx).value()))
            .collect();
        let typed = match validated_space_env(&rows, dirs::home_dir().as_deref()) {
            Ok(env) => env,
            Err(message) => {
                if let Some(editor) = self.space_env_editor.as_mut() {
                    editor.error = Some(message);
                }
                cx.notify();
                return;
            }
        };
        self.space_env_editor = None;
        let mut spaces = self.cached_config.spaces.clone();
        let Some(space) = spaces.iter_mut().find(|space| space.id == id) else {
            cx.notify();
            return;
        };
        let env = with_account_keys(&space.env, typed);
        if space.env != env {
            space.env = env;
            self.persist_spaces(spaces, cx);
            self.mirror_integrations_into_space(&id, cx);
        }
        cx.notify();
    }

    fn space_workspace_count(&self, id: Option<&SpaceId>) -> usize {
        self.workspaces
            .iter()
            .filter(|workspace| crate::app::spaces::in_space(workspace, id))
            .count()
    }

    pub(crate) fn render_spaces_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        if let Some(space) = self
            .space_detail
            .as_ref()
            .and_then(|id| self.cached_config.space(id))
        {
            return self.render_space_detail(space, ui, ink, cx);
        }
        let spaces = &self.cached_config.spaces;
        if spaces.is_empty() {
            return Block::new("Spaces")
                .child(render_spaces_welcome(ink, ui, cx))
                .finish();
        }
        let mut list = kit::group(ink).child(self.render_default_space_row(ink, ui));
        for space in spaces {
            list = list
                .child(kit::rule(ink))
                .child(self.render_space_row(space, ink, ui, cx));
        }
        list = list.child(kit::rule(ink)).child(render_new_space_row(
            spaces.len() >= MAX_SPACES,
            ink,
            cx,
        ));
        Block::new("Spaces").child(list).finish()
    }

    pub(crate) fn render_spaces_heading(
        &self,
        title: gpui::Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        if let Some(space) = self
            .space_detail
            .as_ref()
            .and_then(|id| self.cached_config.space(id))
        {
            return render_space_breadcrumb(space.name.clone(), ui, cx);
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(Ink::of(ui), SPACES_INTRO))
            .into_any_element()
    }

    pub(crate) fn open_space_detail(
        &mut self,
        id: &SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.space_detail = Some(id.clone());
        self.space_delete_armed = None;
        self.space_env_editor = None;
        self.space_avatar_import.error = None;
        if self.space_name_editor.take().is_some() {
            self.settings_focus.focus(window, cx);
        }
        self.reset_settings_scroll();
        cx.notify();
    }

    pub(crate) fn close_space_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_detail = None;
        self.space_delete_armed = None;
        self.space_account_menu = None;
        self.space_env_editor = None;
        self.space_name_editor = None;
        self.space_avatar_import.error = None;
        self.settings_focus.focus(window, cx);
        self.reset_settings_scroll();
        cx.notify();
    }

    pub(crate) fn step_back_from_space_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.space_account_menu.take().is_some()
            || self.space_env_editor.take().is_some()
            || self.space_delete_armed.take().is_some()
        {
            cx.notify();
            return;
        }
        self.close_space_detail(window, cx);
    }

    fn create_preset_space(
        &mut self,
        name: &str,
        color: SpaceColor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.add_space_with(Some(name), Some(color), cx) {
            self.open_space_detail(&id, window, cx);
        }
    }

    fn render_default_space_row(&self, ink: Ink, ui: crate::theme::UiColors) -> AnyElement {
        let count = self.space_workspace_count(None);
        kit::row(
            ink,
            Some(crate::app::sidebar::default_space_mark(kit::LEAD, ui)),
            Text::new(DEFAULT_SPACE_NAME).line(kit::line(
                ink,
                workspace_count_label(count, "Workspaces without a space"),
            )),
            None,
        )
        .into_any_element()
    }

    fn render_space_row(
        &self,
        space: &SpaceConfig,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.space_workspace_count(Some(&space.id));
        let id = space.id.clone();
        let text = Text::new(space.name.clone())
            .line(kit::line(
                ink,
                space_meta_label(
                    count,
                    &self.space_account_labels(space),
                    other_variable_count(&space.env),
                ),
            ))
            .maybe_line(self.render_space_status(space, ink));
        kit::link_row(
            SharedString::from(format!("space-row-{}", space.id)),
            ink,
            Some(crate::app::sidebar::space_mark(
                space.color,
                space.avatar.as_deref(),
                kit::LEAD,
                ui,
            )),
            text,
            kit::Place::Middle,
        )
        .aria_label(format!("Open space {}", space.name))
        .on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_space_detail(&id, window, cx);
        }))
        .into_any_element()
    }

    fn render_space_status(&self, space: &SpaceConfig, ink: Ink) -> Option<AnyElement> {
        let failed = self
            .space_integrations
            .errors
            .keys()
            .any(|(id, _)| id == &space.id);
        if failed {
            return Some(status_line(
                CANCEL_ICON,
                ink.danger,
                "Hooks not added".to_string(),
                ink.danger,
            ));
        }
        let warnings =
            space_env_warnings(&space.env, crate::space_env::inherited_from_paneflow).len();
        (warnings > 0).then(|| {
            status_line(
                ALERT_ICON,
                ink.warning,
                warning_count_label(warnings),
                ink.tertiary,
            )
        })
    }

    fn render_space_detail(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.space_workspace_count(Some(&space.id));
        let mut block = Block::new("Spaces")
            .gap(kit::SECTION_GAP)
            .child(self.render_space_identity(space, ui, ink, cx));
        if let Some(accounts) = self.render_space_accounts(space, ui, cx) {
            block = block.child(accounts);
        }
        block
            .child(self.render_space_env_section(space, ui, ink, cx))
            .child(self.render_space_delete(space, count, ink, cx))
            .finish()
    }

    fn render_space_identity(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let error = self
            .space_avatar_import
            .error
            .as_ref()
            .filter(|(id, _)| id == &space.id)
            .map(|(_, error)| error.clone());
        let remove_image = space.avatar.is_some().then(|| {
            let id = space.id.clone();
            kit::icon_button(
                SharedString::from(format!("space-avatar-remove-{}", space.id)),
                "Remove image",
                DELETE_ICON,
                ink,
            )
            .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_space_avatar(&id, None, cx);
            }))
        });
        kit::group(ink)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(IDENTITY_GAP))
                    .p(px(IDENTITY_PADDING))
                    .child(self.render_space_avatar_button(space, ui, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(self.render_space_name_field(space, ui, cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.))
                                    .child(self.render_space_colors(space, ink, ui, cx))
                                    .children(remove_image),
                            ),
                    ),
            )
            .when_some(error, |card, error| {
                card.child(
                    div()
                        .px(px(IDENTITY_PADDING))
                        .pb(px(IDENTITY_PADDING))
                        .child(hint_line(ink, error, HintTone::Error)),
                )
            })
            .into_any_element()
    }

    fn render_space_avatar_button(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("space-avatar-{}", space.id));
        let busy = self.space_avatar_import.busy.as_ref() == Some(&space.id);
        let action = if space.avatar.is_some() {
            "Change image"
        } else {
            "Add image"
        };
        let id = space.id.clone();
        div()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .flex_none()
            .size(px(IDENTITY_MARK))
            .cursor(CursorStyle::PointingHand)
            .role(gpui::accesskit::Role::Button)
            .aria_label(format!("{action} for space {}", space.name))
            .opaque_keyboard_stop(px(IDENTITY_MARK / 2.))
            .delayed_tooltip(text_tooltip(action))
            .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.choose_space_avatar(&id, cx);
            }))
            .child(crate::app::sidebar::space_mark(
                space.color,
                space.avatar.as_deref(),
                IDENTITY_MARK,
                ui,
            ))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(with_alpha(gpui::black(), AVATAR_SCRIM_ALPHA))
                    .when(!busy, |overlay| {
                        overlay
                            .opacity(0.)
                            .group_hover(group, |style| style.opacity(1.))
                    })
                    .child(if busy {
                        crate::ui_primitives::comet_spinner(px(AVATAR_ICON_SIZE), gpui::white())
                    } else {
                        svg()
                            .size(px(AVATAR_ICON_SIZE))
                            .flex_none()
                            .path("icons/settings/pencil-edit-02.svg")
                            .text_color(gpui::white())
                            .into_any_element()
                    }),
            )
            .when(!busy, |avatar| {
                avatar.child(
                    div()
                        .absolute()
                        .right(px(-AVATAR_BADGE_OFFSET))
                        .bottom(px(-AVATAR_BADGE_OFFSET))
                        .size(px(AVATAR_BADGE))
                        .p(px(AVATAR_BADGE_RING))
                        .rounded_full()
                        .bg(select_menu_surface(ui))
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(lerp_color(select_menu_surface(ui), ui.text, 0.14))
                                .child(
                                    svg()
                                        .size(px(AVATAR_BADGE_ICON))
                                        .flex_none()
                                        .path(if space.avatar.is_some() {
                                            "icons/settings/pencil-edit-02.svg"
                                        } else {
                                            "icons/settings/add-01.svg"
                                        })
                                        .text_color(ui.text),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_space_name_field(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editing = self
            .space_name_editor
            .as_ref()
            .filter(|editor| editor.id == space.id);
        if editing.is_none() && self.space_name_input.read(cx).value() != space.name {
            let name = SharedString::from(space.name.clone());
            self.space_name_input
                .update(cx, |input, cx| input.set_value(name, cx));
        }
        let id = space.id.clone();
        let field = input_frame(
            div()
                .id(SharedString::from(format!("space-name-{}", space.id)))
                .w_full()
                .max_w(px(NAME_FIELD_WIDTH))
                .h(px(NAME_FIELD_HEIGHT))
                .flex()
                .items_center()
                .px(px(10.)),
            &self.space_name_input,
            editing.is_some_and(|editor| editor.error.is_some()),
            ui,
        )
        .text_size(TITLE)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(ui.text)
        .cursor(CursorStyle::IBeam)
        .aria_label("Space name")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &gpui::MouseDownEvent, window, cx| {
                this.begin_space_name_edit(id.clone(), window, cx);
            }),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if this.space_name_editor.is_none() {
                return;
            }
            match event.keystroke.key.as_str() {
                "enter" => {
                    this.commit_space_name(window, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    this.cancel_space_name(window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
            if this.space_name_editor.is_some() {
                this.commit_space_name(window, cx);
            }
        }))
        .child(self.space_name_input.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(field)
            .when_some(editing.and_then(|editor| editor.error), |column, error| {
                column.child(hint_line(Ink::of(ui), error, HintTone::Error))
            })
            .into_any_element()
    }

    fn render_space_colors(
        &self,
        space: &SpaceConfig,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = space.id.to_string();
        let mut swatches = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .ml(px(-(PICKER_TARGET - PICKER_SWATCH) / 2.));
        let choices = std::iter::once(None).chain(SPACE_COLORS.into_iter().map(Some));
        for choice in choices {
            let selected = space.color == choice;
            let id = space.id.clone();
            let label = choice.map_or("No color", space_color_name);
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!(
                        "space-color-{key}-{}",
                        choice.map_or("none", space_color_name)
                    )))
                    .size(px(PICKER_TARGET))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor(CursorStyle::PointingHand)
                    .when(selected, |target| {
                        target.border_1().border_color(ink.secondary)
                    })
                    .when(!selected, |target| {
                        target.hover(move |style| style.bg(ink.hover))
                    })
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(label)
                    .keyboard_stop(px(PICKER_TARGET / 2.))
                    .aria_toggled(if selected {
                        gpui::accesskit::Toggled::True
                    } else {
                        gpui::accesskit::Toggled::False
                    })
                    .delayed_tooltip(text_tooltip(label))
                    .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_space_color(&id, choice, cx);
                    }))
                    .child(crate::app::sidebar::space_mark(
                        choice,
                        None,
                        PICKER_SWATCH,
                        ui,
                    )),
            );
        }
        swatches.into_any_element()
    }

    fn render_space_env_section(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = self
            .space_env_editor
            .as_ref()
            .filter(|editor| editor.id == space.id);
        let variables: BTreeMap<String, String> = space
            .env
            .iter()
            .filter(|(key, _)| !is_account_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let body = match editor {
            Some(editor) => kit::group(ink).child(
                div()
                    .p(px(IDENTITY_PADDING))
                    .child(self.render_space_env_editor(editor, ui, ink, cx)),
            ),
            None => {
                let id = space.id.clone();
                let (label, icon) = if variables.is_empty() {
                    ("Add variable", ADD_ICON)
                } else {
                    ("Edit variables", EDIT_ICON)
                };
                let mut card = kit::group(ink);
                if !variables.is_empty() {
                    card = card
                        .child(render_env_list(&variables, ink))
                        .child(kit::rule(ink));
                }
                card.child(
                    kit::link_row(
                        SharedString::from(format!("space-env-edit-{}", space.id)),
                        ink,
                        Some(kit::disc(ink, icon)),
                        Text::new(label),
                        if variables.is_empty() {
                            kit::Place::Only
                        } else {
                            kit::Place::Last
                        },
                    )
                    .on_activate(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.open_space_env_editor(&id, window, cx);
                        },
                    )),
                )
            }
        };
        let warnings = if editor.is_none() {
            space_env_warnings(&space.env, crate::space_env::inherited_from_paneflow)
        } else {
            Vec::new()
        };
        kit::section(
            ink,
            "Environment",
            editor.is_none().then_some(ENV_HINT),
            body,
        )
        .when(!warnings.is_empty(), |section| {
            section.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(4.))
                    .px(px(kit::TEXT_INSET))
                    .children(
                        warnings
                            .into_iter()
                            .map(|warning| self.render_space_warning(&space.id, warning, ink, cx)),
                    ),
            )
        })
        .into_any_element()
    }

    fn render_space_env_editor(
        &self,
        editor: &SpaceEnvEditor,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = dirs::home_dir();
        let column_label = |label: &'static str| {
            div()
                .text_size(px(kit::SIZE_SM))
                .line_height(px(kit::LINE_SM))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.secondary)
                .child(label)
        };
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w_full()
            .child(kit::line(ink, ENV_EDITOR_INTRO))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .pt(px(6.))
                    .child(
                        div()
                            .w(px(ENV_NAME_WIDTH))
                            .flex_none()
                            .child(column_label("Name")),
                    )
                    .child(div().flex_1().min_w_0().child(column_label("Value"))),
            );
        let removable = editor.rows.len() > 1;
        for (row_idx, row) in editor.rows.iter().enumerate() {
            let hint = env_row_hint(
                &row.key.read(cx).value(),
                &row.value.read(cx).value(),
                home.as_deref(),
            );
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .w_full()
                            .child(div().w(px(ENV_NAME_WIDTH)).flex_none().child(input_box(
                                row.key.clone(),
                                true,
                                ui,
                            )))
                            .child(div().flex_1().min_w_0().child(input_box(
                                row.value.clone(),
                                true,
                                ui,
                            )))
                            .when(removable, |line| {
                                line.child(
                                    kit::icon_button(
                                        SharedString::from(format!("space-env-remove-{row_idx}")),
                                        "Remove variable",
                                        DELETE_ICON,
                                        ink,
                                    )
                                    .keyboard_stop(px(8.))
                                    .on_activate(cx.listener(
                                        move |this, _: &ClickEvent, _w, cx| {
                                            if let Some(editor) = this.space_env_editor.as_mut()
                                                && row_idx < editor.rows.len()
                                            {
                                                editor.rows.remove(row_idx);
                                                cx.notify();
                                            }
                                        },
                                    )),
                                )
                            }),
                    )
                    .when_some(hint, |line, (text, tone)| {
                        line.child(
                            div()
                                .pl(px(ENV_NAME_WIDTH + 6.))
                                .child(hint_line(ink, text, tone)),
                        )
                    }),
            );
        }
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .pt(px(6.))
            .child(
                kit::button(
                    "space-env-add",
                    "Add variable",
                    Tone::Ghost,
                    Size::Small,
                    ink,
                )
                .on_activate(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    let row = space_env_row("", "", cx);
                    if let Some(editor) = this.space_env_editor.as_mut() {
                        editor.rows.push(row);
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(
                        kit::button(
                            "space-env-cancel",
                            "Cancel",
                            Tone::Secondary,
                            Size::Small,
                            ink,
                        )
                        .on_activate(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.space_env_editor = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        kit::button("space-env-save", "Save", Tone::Primary, Size::Small, ink)
                            .on_activate(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.save_space_env(cx);
                            })),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(rows)
            .when_some(editor.error.clone(), |column, error| {
                column.child(hint_line(ink, error, HintTone::Error))
            })
            .child(footer)
            .into_any_element()
    }

    fn render_space_warning(
        &self,
        space: &SpaceId,
        warning: SpaceWarning,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hint = hint_line(ink, warning.text, HintTone::Warning);
        let Some(key) = warning.turn_off else {
            return hint;
        };
        let id = space.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(div().flex_1().min_w_0().child(hint))
            .child(
                kit::button(
                    SharedString::from(format!("space-turn-off-{space}-{key}")),
                    format!("Turn off {key} here"),
                    Tone::Secondary,
                    Size::Small,
                    ink,
                )
                .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.turn_off_inherited(&id, key, cx);
                })),
            )
            .into_any_element()
    }

    fn turn_off_inherited(&mut self, id: &SpaceId, key: &'static str, cx: &mut Context<Self>) {
        let mut spaces = self.cached_config.spaces.clone();
        let Some(space) = spaces.iter_mut().find(|space| &space.id == id) else {
            return;
        };
        space.env.insert(key.to_string(), String::new());
        let name = space.name.clone();
        self.persist_spaces(spaces, cx);
        self.show_toast(
            format!("New panes in {name} no longer see {key}. Open panes keep it."),
            cx,
        );
    }

    fn render_space_delete(
        &self,
        space: &SpaceConfig,
        count: usize,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let armed = self.space_delete_armed.as_ref() == Some(&space.id);
        let consequence = format!("{}.", space_delete_message(count));
        let id = space.id.clone();
        let (text, control) = if armed {
            (
                Text::new(format!("Delete {}?", space.name)).danger(ink),
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        kit::button(
                            "space-delete-cancel",
                            "Cancel",
                            Tone::Secondary,
                            Size::Small,
                            ink,
                        )
                        .on_activate(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.space_delete_armed = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        kit::button(
                            "space-delete-confirm",
                            "Delete space",
                            Tone::Danger,
                            Size::Small,
                            ink,
                        )
                        .on_activate(cx.listener(
                            move |this, _: &ClickEvent, _w, cx| {
                                this.delete_space(&id, cx);
                            },
                        )),
                    )
                    .into_any_element(),
            )
        } else {
            (
                Text::new("Delete space"),
                kit::button(
                    SharedString::from(format!("space-delete-{}", space.id)),
                    "Delete",
                    Tone::Danger,
                    Size::Small,
                    ink,
                )
                .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.space_delete_armed = Some(id.clone());
                    this.space_name_editor = None;
                    this.space_env_editor = None;
                    cx.notify();
                }))
                .into_any_element(),
            )
        };
        kit::group(ink)
            .child(kit::row(
                ink,
                None,
                text.line(kit::line(ink, consequence)),
                Some(control),
            ))
            .into_any_element()
    }
}

fn render_space_breadcrumb(
    name: String,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    div()
        .flex()
        .px(px(kit::TEXT_INSET))
        .pb(px(kit::SECTION_GAP))
        .child(settings_breadcrumb(
            "spaces-breadcrumb",
            "Spaces",
            "Back to all spaces",
            name,
            ui,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_space_detail(window, cx);
            }),
        ))
        .into_any_element()
}

fn render_new_space_row(at_limit: bool, ink: Ink, cx: &mut Context<PaneFlowApp>) -> AnyElement {
    if at_limit {
        return kit::row(
            ink,
            Some(kit::disc(ink, ADD_ICON)),
            Text::new("Space limit reached").line(kit::line(ink, SPACE_LIMIT_REASON)),
            None,
        )
        .into_any_element();
    }
    kit::link_row(
        "space-new",
        ink,
        Some(kit::disc(ink, ADD_ICON)),
        Text::new("New space"),
        kit::Place::Last,
    )
    .on_activate(cx.listener(|this, _: &ClickEvent, window, cx| {
        if let Some(id) = this.add_space(cx) {
            this.edit_space_name(id, window, cx);
        }
    }))
    .into_any_element()
}

fn render_spaces_welcome(
    ink: Ink,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let mut marks = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(WELCOME_MARK_GAP))
        .child(crate::app::sidebar::default_space_mark(WELCOME_MARK, ui));
    for (_, color) in SPACE_PRESETS {
        marks = marks.child(crate::app::sidebar::space_mark(
            Some(color),
            None,
            WELCOME_MARK,
            ui,
        ));
    }
    let mut presets = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_center()
        .gap(px(8.));
    for (name, color) in SPACE_PRESETS {
        presets = presets.child(
            preset_chip(
                SharedString::from(format!("space-preset-{}", name.to_lowercase())),
                name,
                crate::app::sidebar::space_mark(Some(color), None, PRESET_MARK, ui),
                ink,
            )
            .on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.create_preset_space(name, color, window, cx);
            })),
        );
    }
    presets = presets.child(
        preset_chip(
            "space-preset-custom",
            "Custom",
            svg()
                .size(px(PRESET_MARK))
                .flex_none()
                .path(ADD_ICON)
                .text_color(ink.primary)
                .into_any_element(),
            ink,
        )
        .on_activate(cx.listener(|this, _: &ClickEvent, window, cx| {
            if let Some(id) = this.add_space(cx) {
                this.edit_space_name(id, window, cx);
            }
        })),
    );
    kit::group(ink)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .px(px(32.))
                .pt(px(32.))
                .pb(px(28.))
                .child(marks)
                .child(kit::headline(ink, WELCOME_TITLE).pt(px(16.)))
                .child(
                    kit::page_description(ink, WELCOME_BODY)
                        .pt(px(6.))
                        .max_w(px(WELCOME_TEXT_WIDTH))
                        .text_center(),
                )
                .child(div().pt(px(20.)).child(presets)),
        )
        .into_any_element()
}

fn preset_chip(
    id: impl Into<SharedString>,
    label: &'static str,
    mark: AnyElement,
    ink: Ink,
) -> gpui::Stateful<gpui::Div> {
    let id: SharedString = id.into();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .h(px(PRESET_HEIGHT))
        .pl(px(PRESET_PADDING_X - 2.))
        .pr(px(PRESET_PADDING_X))
        .rounded(px(PRESET_RADIUS))
        .bg(ink.well)
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(ink.well_hover))
        .active(move |style| style.bg(ink.well_pressed))
        .accessible_control(
            gpui::accesskit::Role::Button,
            if label == "Custom" {
                "New space".to_string()
            } else {
                format!("Create a {label} space")
            },
        )
        .keyboard_stop(px(PRESET_RADIUS))
        .child(mark)
        .child(
            div()
                .text_size(px(kit::SIZE_MD))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.primary)
                .child(label),
        )
}

fn render_env_list(variables: &BTreeMap<String, String>, ink: Ink) -> AnyElement {
    let mono = kit::mono_family();
    let mut list = div().flex().flex_col();
    for (index, (key, value)) in variables.iter().enumerate() {
        if index > 0 {
            list = list.child(kit::rule(ink));
        }
        let shown = if value.is_empty() {
            empty_value_label(key).to_string()
        } else if crate::space_env::stores_secret(key) {
            SECRET_MASK.to_string()
        } else {
            value.clone()
        };
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(16.))
                .px(px(kit::ROW_INSET))
                .py(px(ENV_ROW_PADDING_Y))
                .font_family(mono.clone())
                .text_size(px(kit::SIZE_SM))
                .line_height(px(kit::LINE_SM))
                .child(
                    div()
                        .w(px(ENV_NAME_WIDTH))
                        .flex_none()
                        .truncate()
                        .text_color(ink.primary)
                        .child(key.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.tertiary)
                        .child(shown),
                ),
        );
    }
    list.into_any_element()
}

fn status_line(
    icon: &'static str,
    icon_color: gpui::Hsla,
    text: impl Into<SharedString>,
    text_color: gpui::Hsla,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.))
        .child(
            svg()
                .mt(px(2.))
                .size(px(12.))
                .flex_none()
                .path(icon)
                .text_color(icon_color),
        )
        .child(kit::line_in(text_color, text).flex_1())
        .into_any_element()
}

pub(crate) fn hint_line(ink: Ink, text: impl Into<SharedString>, tone: HintTone) -> AnyElement {
    match tone {
        HintTone::Error => kit::line_in(ink.danger, text).into_any_element(),
        HintTone::Warning => status_line(ALERT_ICON, ink.warning, text, ink.secondary),
        HintTone::Preview => kit::line(ink, text).into_any_element(),
    }
}

fn discard_avatar_in_background(file_name: String, cx: &mut Context<PaneFlowApp>) {
    cx.background_spawn(async move { crate::space_avatar::discard_avatar(&file_name) })
        .detach();
}

fn workspace_count_label(count: usize, empty: &'static str) -> String {
    match count {
        0 => empty.to_string(),
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    }
}

fn space_meta_label(workspaces: usize, accounts: &[String], variables: usize) -> String {
    let mut parts = vec![workspace_count_label(workspaces, "No workspaces")];
    parts.extend(accounts.iter().cloned());
    match variables {
        0 => {}
        1 => parts.push("1 variable".to_string()),
        n => parts.push(format!("{n} variables")),
    }
    parts.join(" · ")
}

fn empty_value_label(key: &str) -> &'static str {
    let platform = crate::space_env::platform_key(key);
    if crate::space_env::SHADOWING_CREDENTIALS
        .iter()
        .any(|(credential, _, _)| platform == *credential)
    {
        "Turned off"
    } else {
        "Empty"
    }
}

fn with_account_keys(
    current: &BTreeMap<String, String>,
    typed: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = current
        .iter()
        .filter(|(key, _)| {
            is_account_key(key) && !typed.keys().any(|typed_key| same_key(typed_key, key))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    env.extend(typed);
    env
}

fn space_env_row(key: &str, value: &str, cx: &mut Context<PaneFlowApp>) -> EnvRowInputs {
    let row = new_env_row(key, value, cx);
    let value_hint = if key.is_empty() {
        ENV_VALUE_EXAMPLE
    } else {
        empty_value_label(key)
    };
    row.key
        .update(cx, |input, cx| input.set_placeholder(ENV_NAME_EXAMPLE, cx));
    row.value
        .update(cx, |input, cx| input.set_placeholder(value_hint, cx));
    row
}

fn warning_count_label(count: usize) -> String {
    match count {
        1 => "1 warning".to_string(),
        n => format!("{n} warnings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(n: u8, name: &str) -> SpaceConfig {
        SpaceConfig {
            id: SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap(),
            name: name.to_string(),
            color: None,
            avatar: None,
            env: Default::default(),
        }
    }

    #[test]
    fn renaming_a_space_rejects_invalid_names_and_writes_nothing() {
        let spaces = vec![space(1, "Work"), space(2, "Perso")];
        let id = spaces[0].id.clone();
        assert_eq!(renamed_spaces(&spaces, &id, "  "), Err("Name required"));
        assert_eq!(
            renamed_spaces(&spaces, &id, "perso"),
            Err("Name already used")
        );
        assert_eq!(
            renamed_spaces(&spaces, &id, "default"),
            Err("Default is reserved")
        );
        assert_eq!(
            renamed_spaces(&spaces, &id, &"x".repeat(33)),
            Err("32 characters maximum")
        );
    }

    #[test]
    fn renaming_a_space_keeps_its_own_name_and_trims_the_new_one() {
        let spaces = vec![space(1, "Work"), space(2, "Perso")];
        let id = spaces[0].id.clone();
        assert_eq!(renamed_spaces(&spaces, &id, "Work"), Ok(None));
        let renamed = renamed_spaces(&spaces, &id, "  Client A ")
            .unwrap()
            .unwrap();
        assert_eq!(renamed[0].name, "Client A");
        assert_eq!(renamed[1].name, "Perso");
    }

    fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn saving_a_space_env_refuses_each_invalid_entry_with_its_reason() {
        let home = std::env::temp_dir().join("settings-home");
        assert_eq!(
            validated_space_env(&rows(&[("PANEFLOW_HOME", "/x")]), Some(&home)),
            Err("'PANEFLOW_HOME': PANEFLOW_ variables are reserved for Paneflow".to_string())
        );
        assert_eq!(
            validated_space_env(&rows(&[("TERM", "dumb")]), Some(&home)),
            Err("'TERM': Paneflow sets it for every terminal".to_string())
        );
        assert_eq!(
            validated_space_env(&rows(&[("LD_PRELOAD", "/x.so")]), Some(&home)),
            Err("'LD_PRELOAD': It could change how programs load or run agents".to_string())
        );
        assert_eq!(
            validated_space_env(&rows(&[("2FA", "x")]), Some(&home)),
            Err("'2FA': Not a valid variable name".to_string())
        );
        assert_eq!(
            validated_space_env(&rows(&[("CODEX_HOME", "codex-work")]), Some(&home)),
            Err("'CODEX_HOME': Must be an absolute path".to_string())
        );
        let saved = validated_space_env(
            &rows(&[
                ("CLAUDE_CONFIG_DIR", "~/.claude-work"),
                ("", ""),
                ("GIT_AUTHOR_EMAIL", " me@work.example "),
            ]),
            Some(&home),
        )
        .unwrap();
        assert_eq!(
            saved.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("~/.claude-work"),
            "the raw value is stored and expanded at launch"
        );
        assert_eq!(
            saved.get("GIT_AUTHOR_EMAIL").map(String::as_str),
            Some("me@work.example")
        );
    }

    #[test]
    fn each_env_row_explains_an_error_a_secret_or_its_expansion() {
        let home = std::env::temp_dir().join("settings-home");
        assert_eq!(
            env_row_hint("PANEFLOW_X", "1", Some(&home)),
            Some((
                "PANEFLOW_ variables are reserved for Paneflow".to_string(),
                HintTone::Error
            ))
        );
        assert_eq!(
            env_row_hint("GH_CONFIG_DIR", "gh", Some(&home)),
            Some(("Must be an absolute path".to_string(), HintTone::Error))
        );
        assert_eq!(
            env_row_hint("GH_TOKEN", "ghp_x", Some(&home)),
            Some((
                crate::space_env::PLAIN_TEXT_WARNING.to_string(),
                HintTone::Warning
            ))
        );
        assert_eq!(
            env_row_hint("CLAUDE_CONFIG_DIR", "~/.claude-work", Some(&home)),
            Some((
                home.join(".claude-work").display().to_string(),
                HintTone::Preview
            ))
        );
        assert_eq!(env_row_hint("GIT_AUTHOR_NAME", "Me", Some(&home)), None);
    }

    #[test]
    fn a_space_with_env_warns_about_inherited_credentials_and_plain_text_secrets() {
        let env = BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), "/work/.claude".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-work".to_string()),
        ]);
        let warnings = space_env_warnings(&env, |key| {
            key == "GITHUB_TOKEN" || key == "ANTHROPIC_API_KEY"
        });
        assert_eq!(
            warnings,
            vec![
                SpaceWarning {
                    text: "ANTHROPIC_API_KEY is set in Paneflow's environment, so Claude Code uses it instead of this space's account.".to_string(),
                    turn_off: Some("ANTHROPIC_API_KEY"),
                },
                SpaceWarning {
                    text: format!("OPENAI_API_KEY: {}", crate::space_env::PLAIN_TEXT_WARNING),
                    turn_off: None,
                },
            ],
            "GITHUB_TOKEN stays silent: this space sets no GitHub account"
        );
        let turned_off = BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), "/work/.claude".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), String::new()),
        ]);
        assert!(space_env_warnings(&turned_off, |key| key == "ANTHROPIC_API_KEY").is_empty());
        assert!(space_env_warnings(&BTreeMap::new(), |_| true).is_empty());
    }

    #[test]
    fn spaces_sharing_their_agent_directories_are_refreshed_as_one_group() {
        let home = std::path::Path::new("/state/.paneflow");
        let claude = |dir: &str| {
            Some(paneflow_agent_config::ClaudePaths {
                config_dir: std::path::PathBuf::from(dir),
                global_config: std::path::PathBuf::from(dir).join(".claude.json"),
            })
        };
        let target = |n: u8, dir: &str| {
            let id = space(n, "x").id;
            let target = paneflow_mcp_install::IntegrationTarget::for_space(
                home,
                &id.to_string(),
                claude(dir),
                None,
            );
            (id, target)
        };
        let groups = launch_refresh_groups(vec![
            target(1, "/work/.claude"),
            target(2, "/perso/.claude"),
            target(3, "/work/.claude"),
        ]);
        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 1]);
        assert_eq!(groups[0][1].0, space(3, "x").id);
    }

    #[test]
    fn deleting_a_space_names_how_many_workspaces_move() {
        assert_eq!(space_delete_message(3), "Its 3 workspaces move to Default");
        assert_eq!(space_delete_message(1), "Its 1 workspace moves to Default");
        assert_eq!(space_delete_message(0), "It has no workspaces");
    }

    #[test]
    fn a_collapsed_space_summarizes_its_workspaces_and_variables() {
        assert_eq!(space_meta_label(0, &[], 0), "No workspaces");
        assert_eq!(space_meta_label(1, &[], 0), "1 workspace");
        assert_eq!(space_meta_label(4, &[], 1), "4 workspaces · 1 variable");
        assert_eq!(space_meta_label(0, &[], 3), "No workspaces · 3 variables");
        assert_eq!(
            space_meta_label(2, &["pro@example.com".to_string()], 1),
            "2 workspaces · pro@example.com · 1 variable"
        );
    }
}
