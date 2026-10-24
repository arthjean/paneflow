use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px, svg,
};
use paneflow_config::schema::{
    DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceConfig, SpaceId, validate_space_name,
};
use paneflow_mcp_install::{IntegrationState, IntegrationStatus};

use std::collections::BTreeMap;

use crate::PaneFlowApp;
use crate::app::spaces::SPACE_LIMIT_REASON;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, destructive_button, destructive_color,
    icon_button as labeled_icon_button, secondary_button, setting_card, with_alpha,
};
use crate::settings::search::Block;
use crate::settings::tabs::agents::{
    EnvRowInputs, collect_env, icon_button, input_box, new_env_row, validate_env_values,
};
use crate::ui_primitives::{
    AccessibleControlExt, BODY, BODY_EMPHASIS, LABEL_SM, ROW_RADIUS, TooltipDelayExt,
    squircle_skin, text_tooltip,
};

const NAME_MAX_WIDTH: f32 = 260.;
const ENV_NAME_WIDTH: f32 = 180.;
const INPUT_HEIGHT: f32 = 28.;
const LIST_INSET: f32 = 6.;
const ITEM_PADDING_X: f32 = 10.;
const ITEM_PADDING_Y: f32 = 8.;
const ITEM_GAP: f32 = 12.;
const AVATAR_SIZE: f32 = 20.;
const AVATAR_ICON_SIZE: f32 = 12.;
const AVATAR_SCRIM_ALPHA: f32 = 0.5;
const DETAIL_INDENT: f32 = ITEM_PADDING_X + AVATAR_SIZE + ITEM_GAP;
const FIELD_LABEL_WIDTH: f32 = 92.;
const PICKER_TARGET: f32 = 24.;
const PICKER_SWATCH: f32 = 16.;
const SPACES_INTRO: &str = "Group workspaces into contexts, such as work and personal projects. \
     A space can set its own environment variables, for example to use a separate agent account.";

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

pub(crate) fn space_env_warnings(
    env: &BTreeMap<String, String>,
    inherited: impl Fn(&str) -> bool,
) -> Vec<String> {
    if env.is_empty() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let shadowing = crate::space_env::shadowing_credentials(env, inherited);
    match shadowing.as_slice() {
        [] => {}
        [key] => warnings.push(format!(
            "{key} is set in Paneflow's environment and takes precedence over this space's accounts. Redefine it here to override it."
        )),
        keys => warnings.push(format!(
            "{} are set in Paneflow's environment and take precedence over this space's accounts. Redefine them here to override them.",
            keys.join(", ")
        )),
    }
    warnings.extend(
        env.keys()
            .filter(|key| crate::space_env::stores_secret(key))
            .map(|key| format!("{key}: {}", crate::space_env::PLAIN_TEXT_WARNING)),
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

fn env_summary(env: &BTreeMap<String, String>) -> String {
    match env.len() {
        0 => "No variables".to_string(),
        1 => format!(
            "1 variable: {}",
            env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
        n => format!(
            "{n} variables: {}",
            env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    }
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
        self.space_settings_expanded = Some(id.clone());
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
                self.settings_focus.focus(window, cx);
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
        if self.space_settings_expanded.as_ref() == Some(id) {
            self.space_settings_expanded = None;
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

    fn retry_space_integration(&mut self, id: &SpaceId, slug: &str, cx: &mut Context<Self>) {
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

    fn open_space_env_editor(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        let env = self
            .cached_config
            .space(id)
            .map(|space| space.env.clone())
            .unwrap_or_default();
        let mut rows: Vec<EnvRowInputs> = env
            .iter()
            .map(|(key, value)| new_env_row(key, value, cx))
            .collect();
        if rows.is_empty() {
            rows.push(new_env_row("", "", cx));
        }
        self.space_env_editor = Some(SpaceEnvEditor {
            id: id.clone(),
            rows,
            error: None,
        });
        self.space_delete_armed = None;
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
        let env = match validated_space_env(&rows, dirs::home_dir().as_deref()) {
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
        if let Some(space) = spaces.iter_mut().find(|space| space.id == id)
            && space.env != env
        {
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

    pub(crate) fn render_spaces_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let spaces = &self.cached_config.spaces;
        let mut list = setting_card(ui)
            .p(px(LIST_INSET))
            .gap(px(2.))
            .child(self.render_default_space_item(ui));
        for space in spaces {
            list = list.child(self.render_space_item(space, ui, cx));
        }

        div().flex().flex_col().child(
            Block::new("Spaces")
                .child(render_spaces_intro(spaces.len() >= MAX_SPACES, ui, cx))
                .child(list)
                .finish(),
        )
    }

    fn toggle_space_settings(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        self.space_settings_expanded = if self.space_settings_expanded.as_ref() == Some(id) {
            None
        } else {
            Some(id.clone())
        };
        self.space_delete_armed = None;
        cx.notify();
    }

    fn render_default_space_item(&self, ui: crate::theme::UiColors) -> AnyElement {
        let count = self.space_workspace_count(None);
        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(ITEM_PADDING_X))
            .py(px(ITEM_PADDING_Y))
            .child(space_summary(
                crate::app::sidebar::default_space_mark(AVATAR_SIZE, ui),
                DEFAULT_SPACE_NAME.into(),
                workspace_count_label(count, "Workspaces without a space"),
                ui,
            ))
            .into_any_element()
    }

    fn render_space_item(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.space_settings_expanded.as_ref() == Some(&space.id);
        let count = self.space_workspace_count(Some(&space.id));
        let key = space.id.to_string();
        let id = space.id.clone();
        let header = squircle_skin(
            div()
                .id(SharedString::from(format!("space-item-{key}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(ITEM_GAP))
                .px(px(ITEM_PADDING_X))
                .py(px(ITEM_PADDING_Y))
                .cursor(CursorStyle::PointingHand),
            format!("space-item-{key}"),
            ROW_RADIUS,
            None,
            (!expanded).then(|| with_alpha(ui.text, 0.05)),
        )
        .role(gpui::accesskit::Role::Button)
        .aria_label(format!("Space {}", space.name))
        .aria_expanded(expanded)
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_space_settings(&id, cx);
        }))
        .child(space_summary(
            self.render_space_avatar_button(space, ui, cx),
            space.name.clone().into(),
            space_meta_label(count, space.env.len()),
            ui,
        ))
        .when(!expanded, |header| {
            header.children(self.render_space_status(space, ui))
        })
        .child(
            svg()
                .size(px(12.))
                .flex_none()
                .path(if expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .text_color(ui.muted),
        );

        div()
            .flex()
            .flex_col()
            .child(header)
            .when(expanded, |item| {
                item.child(self.render_space_details(space, count, ui, cx))
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
            .size(px(AVATAR_SIZE))
            .cursor(CursorStyle::PointingHand)
            .role(gpui::accesskit::Role::Button)
            .aria_label(format!("{action} for space {}", space.name))
            .delayed_tooltip(text_tooltip(action))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                cx.stop_propagation();
                this.choose_space_avatar(&id, cx);
            }))
            .child(crate::app::sidebar::space_mark(
                space.color,
                space.avatar.as_deref(),
                AVATAR_SIZE,
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
                        crate::ui_primitives::comet_spinner(
                            SharedString::from(format!("space-avatar-spinner-{}", space.id)),
                            px(AVATAR_ICON_SIZE),
                            gpui::white(),
                        )
                    } else {
                        svg()
                            .size(px(AVATAR_ICON_SIZE))
                            .flex_none()
                            .path("icons/pencil-edit-02.svg")
                            .text_color(gpui::white())
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn render_space_image_field(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let error = self
            .space_avatar_import
            .error
            .as_ref()
            .filter(|(id, _)| id == &space.id)
            .map(|(_, error)| error.clone());
        let controls = if self.space_avatar_import.busy.as_ref() == Some(&space.id) {
            div()
                .text_size(BODY)
                .text_color(ui.muted)
                .child("Choosing an image…")
        } else {
            let id = space.id.clone();
            let mut row =
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .child(secondary_button(
                        SharedString::from(format!("space-avatar-choose-{}", space.id)),
                        if space.avatar.is_some() {
                            "Replace image"
                        } else {
                            "Upload image"
                        },
                        ui,
                        cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.choose_space_avatar(&id, cx);
                        }),
                    ));
            if space.avatar.is_some() {
                let id = space.id.clone();
                row = row.child(secondary_button(
                    SharedString::from(format!("space-avatar-remove-{}", space.id)),
                    "Remove image",
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_space_avatar(&id, None, cx);
                    }),
                ));
            }
            row
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(controls)
            .when_some(error, |column, error| {
                column.child(render_hint(error, HintTone::Error, ui))
            })
            .into_any_element()
    }

    fn render_space_status(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        let failed = self
            .space_integrations
            .errors
            .keys()
            .any(|(id, _)| id == &space.id);
        let (icon, icon_color, label, label_color) = if failed {
            (
                "icons/x_circle.svg",
                ui.vc_deleted,
                "Agent setup failed".to_string(),
                ui.vc_deleted,
            )
        } else {
            let warnings =
                space_env_warnings(&space.env, crate::space_env::inherited_from_paneflow).len();
            if warnings == 0 {
                return None;
            }
            (
                "icons/triangle-alert.svg",
                ui.vc_modified,
                warning_count_label(warnings),
                ui.muted,
            )
        };
        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .text_size(LABEL_SM)
                .text_color(label_color)
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path(icon)
                        .text_color(icon_color),
                )
                .child(label)
                .into_any_element(),
        )
    }

    fn render_space_details(
        &self,
        space: &SpaceConfig,
        count: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pl(px(DETAIL_INDENT))
            .pr(px(ITEM_PADDING_X))
            .pt(px(6.))
            .pb(px(14.))
            .child(field_row(
                "Name",
                self.render_space_name_field(space, ui, cx),
                ui,
            ))
            .child(field_row(
                "Image",
                self.render_space_image_field(space, ui, cx),
                ui,
            ))
            .child(field_row(
                "Color",
                self.render_space_colors(space, ui, cx),
                ui,
            ))
            .child(field_row(
                "Environment",
                self.render_space_env(space, ui, cx),
                ui,
            ))
            .when_some(
                self.render_space_integrations(space, ui, cx),
                |details, integrations| details.child(field_row("Agent setup", integrations, ui)),
            )
            .child(self.render_space_delete(space, count, ui, cx))
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
        let field = div()
            .id(SharedString::from(format!("space-name-{}", space.id)))
            .w_full()
            .max_w(px(NAME_MAX_WIDTH))
            .h(px(INPUT_HEIGHT))
            .flex()
            .items_center()
            .px(px(10.))
            .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
            .bg(ui.subtle)
            .text_size(BODY)
            .text_color(ui.text);
        let field = match editing {
            Some(_) => field
                .border_1()
                .border_color(with_alpha(ui.text, 0.25))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
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
                .child(self.space_name_input.clone()),
            None => {
                let id = space.id.clone();
                field
                    .cursor(CursorStyle::IBeam)
                    .accessible_control(
                        gpui::accesskit::Role::TextInput,
                        format!("Name of space {}", space.name),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.edit_space_name(id.clone(), window, cx);
                    }))
                    .child(div().truncate().child(space.name.clone()))
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(field)
            .when_some(editing.and_then(|editor| editor.error), |column, error| {
                column.child(render_hint(error, HintTone::Error, ui))
            })
            .into_any_element()
    }

    fn render_space_colors(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = space.id.to_string();
        let mut swatches = div().flex().flex_row().items_center().gap(px(4.));
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
                        target.border_1().border_color(with_alpha(ui.text, 0.6))
                    })
                    .when(!selected, |target| {
                        target.hover(|style| style.bg(with_alpha(ui.text, 0.06)))
                    })
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(label)
                    .aria_toggled(if selected {
                        gpui::accesskit::Toggled::True
                    } else {
                        gpui::accesskit::Toggled::False
                    })
                    .delayed_tooltip(text_tooltip(label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
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

    fn render_space_env(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(editor) = self
            .space_env_editor
            .as_ref()
            .filter(|editor| editor.id == space.id)
        {
            return self.render_space_env_editor(editor, ui, cx);
        }
        let id = space.id.clone();
        let mut column = div().flex().flex_col().gap(px(6.)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(BODY)
                        .text_color(if space.env.is_empty() {
                            ui.muted
                        } else {
                            ui.text
                        })
                        .child(env_summary(&space.env)),
                )
                .child(secondary_button(
                    SharedString::from(format!("space-env-edit-{}", space.id)),
                    "Edit",
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.open_space_env_editor(&id, cx);
                    }),
                )),
        );
        for warning in space_env_warnings(&space.env, crate::space_env::inherited_from_paneflow) {
            column = column.child(render_hint(warning, HintTone::Warning, ui));
        }
        column.into_any_element()
    }

    fn render_space_env_editor(
        &self,
        editor: &SpaceEnvEditor,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = dirs::home_dir();
        let mut rows = div().flex().flex_col().gap(px(6.)).w_full().child(
            div().text_size(LABEL_SM).text_color(ui.muted).child(
                "Set on every terminal of this space's workspaces, above terminal.env and \
                     below agent profiles. A leading ~ is your home, and $NAME takes the value \
                     from Paneflow's environment.",
            ),
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
                    .gap(px(3.))
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
                            .when(removable, |d| {
                                d.child(
                                    icon_button(
                                        SharedString::from(format!("space-env-remove-{row_idx}")),
                                        "icons/trash.svg",
                                        ui.muted,
                                        destructive_color(),
                                        ui,
                                    )
                                    .accessible_control(
                                        gpui::accesskit::Role::Button,
                                        "Remove variable",
                                    )
                                    .on_click(cx.listener(
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
                    .when_some(hint, |d, (text, tone)| {
                        d.child(
                            div()
                                .pl(px(ENV_NAME_WIDTH + 6.))
                                .child(render_hint(text, tone, ui)),
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
            .pt(px(4.))
            .child(
                div()
                    .id("space-env-add")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .cursor(CursorStyle::PointingHand)
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .hover(move |style| style.bg(with_alpha(ui.text, 0.05)))
                    .accessible_control(gpui::accesskit::Role::Button, "Add variable")
                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                        let row = new_env_row("", "", cx);
                        if let Some(editor) = this.space_env_editor.as_mut() {
                            editor.rows.push(row);
                        }
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .size(px(12.))
                            .flex_none()
                            .path("icons/plus.svg")
                            .text_color(ui.muted),
                    )
                    .child("Add variable"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .child(secondary_button(
                        "space-env-cancel",
                        "Cancel",
                        ui,
                        cx.listener(|this, _: &ClickEvent, _w, cx| {
                            this.space_env_editor = None;
                            cx.notify();
                        }),
                    ))
                    .child(secondary_button(
                        "space-env-save",
                        "Save",
                        ui,
                        cx.listener(|this, _: &ClickEvent, _w, cx| {
                            this.save_space_env(cx);
                        }),
                    )),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(rows)
            .when_some(editor.error.clone(), |column, error| {
                column.child(render_hint(error, HintTone::Error, ui))
            })
            .child(footer)
            .into_any_element()
    }

    fn render_space_integrations(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let statuses = self
            .space_integrations
            .status
            .get(&space.id)
            .filter(|statuses| !statuses.is_empty())?;
        let mut column = div().flex().flex_col().gap(px(6.));
        for status in statuses {
            let key = (space.id.clone(), status.slug.to_string());
            let error = self.space_integrations.errors.get(&key);
            let busy = self.space_integrations.busy.as_ref() == Some(&key);
            let mut line = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .min_h(px(INPUT_HEIGHT))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .truncate()
                                .text_size(BODY)
                                .text_color(ui.text)
                                .child(format!("{} hooks and MCP", status.label)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(LABEL_SM)
                                .text_color(if error.is_some() {
                                    ui.vc_deleted
                                } else {
                                    ui.muted
                                })
                                .child(integration_state_label(
                                    busy,
                                    error.is_some(),
                                    status.state,
                                )),
                        ),
                );
            if !busy && (error.is_some() || status.state == IntegrationState::NotInstalled) {
                let id = space.id.clone();
                let slug = status.slug;
                line = line.child(secondary_button(
                    SharedString::from(format!("space-integration-retry-{}-{slug}", space.id)),
                    if error.is_some() { "Retry" } else { "Install" },
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.retry_space_integration(&id, slug, cx);
                    }),
                ));
            }
            column = column.child(line);
            if let Some(error) = error {
                column = column.child(render_hint(error.clone(), HintTone::Error, ui));
            }
        }
        Some(column.into_any_element())
    }

    fn render_space_delete(
        &self,
        space: &SpaceConfig,
        count: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = space.id.clone();
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .pt(px(4.));
        if self.space_delete_armed.as_ref() != Some(&space.id) {
            return row
                .child(secondary_button(
                    SharedString::from(format!("space-delete-{}", space.id)),
                    "Delete space",
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.space_delete_armed = Some(id.clone());
                        this.space_name_editor = None;
                        cx.notify();
                    }),
                ))
                .into_any_element();
        }
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(BODY)
                .text_color(ui.text)
                .child(format!(
                    "Delete {}? {}.",
                    space.name,
                    space_delete_message(count)
                )),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(px(6.))
                .child(secondary_button(
                    "space-delete-cancel",
                    "Cancel",
                    ui,
                    cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.space_delete_armed = None;
                        cx.notify();
                    }),
                ))
                .child(
                    destructive_button("space-delete-confirm", "Delete space").on_click(
                        cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.delete_space(&id, cx);
                        }),
                    ),
                ),
        )
        .into_any_element()
    }
}

fn render_spaces_intro(
    at_limit: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let new_space = labeled_icon_button(
        "space-new",
        "New space",
        "icons/plus.svg",
        ui,
        false,
        !at_limit,
    )
    .when(!at_limit, |button| {
        button.on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            if let Some(id) = this.add_space(cx) {
                this.edit_space_name(id, window, cx);
            }
        }))
    });
    div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(16.))
        .px(px(2.))
        .pb(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .max_w(px(520.))
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(SPACES_INTRO),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .when(at_limit, |action| {
                    action.child(
                        div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(SPACE_LIMIT_REASON),
                    )
                })
                .child(new_space),
        )
        .into_any_element()
}

fn space_summary(
    mark: AnyElement,
    name: SharedString,
    meta: impl Into<SharedString>,
    ui: crate::theme::UiColors,
) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ITEM_GAP))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .truncate()
                        .text_size(BODY_EMPHASIS)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .child(name),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(LABEL_SM)
                        .text_color(ui.muted)
                        .child(meta.into()),
                ),
        )
}

fn discard_avatar_in_background(file_name: String, cx: &mut Context<PaneFlowApp>) {
    cx.background_spawn(async move { crate::space_avatar::discard_avatar(&file_name) })
        .detach();
}

fn field_row(
    label: &'static str,
    control: impl IntoElement,
    ui: crate::theme::UiColors,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .flex_none()
                .w(px(FIELD_LABEL_WIDTH))
                .h(px(INPUT_HEIGHT))
                .flex()
                .items_center()
                .text_size(BODY)
                .text_color(ui.muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h(px(INPUT_HEIGHT))
                .flex()
                .flex_col()
                .justify_center()
                .child(control),
        )
}

fn render_hint(
    text: impl Into<SharedString>,
    tone: HintTone,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.))
        .text_size(LABEL_SM)
        .text_color(match tone {
            HintTone::Error => ui.vc_deleted,
            HintTone::Warning => ui.text,
            HintTone::Preview => ui.muted,
        })
        .when(tone == HintTone::Warning, |hint| {
            hint.child(
                svg()
                    .mt(px(1.))
                    .size(px(12.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(ui.vc_modified),
            )
        })
        .child(div().flex_1().min_w_0().child(text.into()))
        .into_any_element()
}

fn workspace_count_label(count: usize, empty: &'static str) -> String {
    match count {
        0 => empty.to_string(),
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    }
}

fn space_meta_label(workspaces: usize, variables: usize) -> String {
    let workspaces = workspace_count_label(workspaces, "No workspaces");
    match variables {
        0 => workspaces,
        1 => format!("{workspaces} · 1 variable"),
        n => format!("{workspaces} · {n} variables"),
    }
}

fn warning_count_label(count: usize) -> String {
    match count {
        1 => "1 warning".to_string(),
        n => format!("{n} warnings"),
    }
}

fn integration_state_label(busy: bool, failed: bool, state: IntegrationState) -> String {
    if busy {
        return "Installing…".to_string();
    }
    if failed {
        return "Failed".to_string();
    }
    let label = paneflow_mcp_install::state_label(state);
    let mut chars = label.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
        let warnings =
            space_env_warnings(&env, |key| key == "GITHUB_TOKEN" || key == "OPENAI_API_KEY");
        assert_eq!(
            warnings,
            vec![
                "GITHUB_TOKEN is set in Paneflow's environment and takes precedence over this space's accounts. Redefine it here to override it.".to_string(),
                format!("OPENAI_API_KEY: {}", crate::space_env::PLAIN_TEXT_WARNING),
            ]
        );
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
        assert_eq!(space_meta_label(0, 0), "No workspaces");
        assert_eq!(space_meta_label(1, 0), "1 workspace");
        assert_eq!(space_meta_label(4, 1), "4 workspaces · 1 variable");
        assert_eq!(space_meta_label(0, 3), "No workspaces · 3 variables");
    }

    #[test]
    fn an_integration_state_reads_in_sentence_case() {
        assert_eq!(
            integration_state_label(false, false, IntegrationState::NotInstalled),
            "Not installed"
        );
        assert_eq!(
            integration_state_label(false, true, IntegrationState::Installed),
            "Failed"
        );
        assert_eq!(
            integration_state_label(true, true, IntegrationState::Installed),
            "Installing…"
        );
    }

    #[test]
    fn the_space_rows_sit_concentric_in_the_settings_card() {
        assert_eq!(
            crate::app::constants::SETTINGS_CARD_RADIUS,
            ROW_RADIUS + px(LIST_INSET)
        );
    }
}
