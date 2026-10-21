use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
    svg,
};
use paneflow_config::schema::{
    DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceConfig, SpaceId, validate_space_name,
};
use paneflow_mcp_install::{IntegrationState, IntegrationStatus};

use std::collections::BTreeMap;

use crate::PaneFlowApp;
use crate::app::spaces::SPACE_LIMIT_REASON;
use crate::settings::components::{
    CARD_PADDING_X, CARD_PADDING_Y, SETTINGS_CONTROL_CORNER_RADIUS, destructive_button,
    destructive_color, hairline_inset, secondary_button, section_title, setting_card, with_alpha,
};
use crate::settings::search::Block;
use crate::settings::tabs::agents::{
    EnvRowInputs, collect_env, icon_button, input_box, new_env_row, validate_env_values,
};
use crate::ui_primitives::{
    AccessibleControlExt, BODY, LABEL_SM, ROW_RADIUS, TooltipDelayExt, text_tooltip,
};

const NAME_WIDTH: f32 = 220.;
const ENV_NAME_WIDTH: f32 = 200.;
const ENV_INDENT: f32 = LETTER_SIZE + 12.;
const INPUT_HEIGHT: f32 = 28.;
const SWATCH_SIZE: f32 = 16.;
const LETTER_SIZE: f32 = 22.;

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
        self.persist_spaces(spaces, cx);
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

    fn render_space_integrations(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(statuses) = self.space_integrations.status.get(&space.id) else {
            return div().into_any_element();
        };
        let mut column = div().flex().flex_col().gap(px(4.)).pt(px(2.));
        for status in statuses {
            let key = (space.id.clone(), status.slug.to_string());
            let error = self.space_integrations.errors.get(&key);
            let busy = self.space_integrations.busy.as_ref() == Some(&key);
            let label = match (busy, error) {
                (true, _) => "installing".to_string(),
                (false, Some(error)) => format!("error: {error}"),
                (false, None) => paneflow_mcp_install::state_label(status.state).to_string(),
            };
            let mut line = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_size(LABEL_SM)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if error.is_some() {
                            destructive_color()
                        } else {
                            ui.muted
                        })
                        .child(format!("{} hooks and MCP: {label}", status.label)),
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
        }
        column.into_any_element()
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
        let mut card = setting_card(ui).py(px(CARD_PADDING_Y));
        card = card.child(self.render_default_space_row(ui));
        for space in spaces {
            card = card.child(hairline_inset(ui));
            card = card.child(self.render_space_row(space, ui, cx));
        }
        card = card.child(hairline_inset(ui));
        card = card.child(render_new_space_row(spaces.len() >= MAX_SPACES, ui, cx));

        div().flex().flex_col().child(
            Block::new("Spaces")
                .child(section_title(
                    ui,
                    "Spaces",
                    Some(
                        "Group workspaces into contexts, such as work and personal projects. \
                         A space organizes your workspaces; it does not isolate them.",
                    ),
                ))
                .child(card)
                .finish(),
        )
    }

    fn render_default_space_row(&self, ui: crate::theme::UiColors) -> AnyElement {
        let count = self.space_workspace_count(None);
        space_row_shell()
            .child(space_letter(DEFAULT_SPACE_NAME, None, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(BODY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .child(DEFAULT_SPACE_NAME),
                    )
                    .child(
                        div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(workspace_count_label(count, "Workspaces without a space")),
                    ),
            )
            .into_any_element()
    }

    fn render_space_row(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.space_workspace_count(Some(&space.id));
        let editing = self
            .space_name_editor
            .as_ref()
            .filter(|editor| editor.id == space.id);
        let armed = self.space_delete_armed.as_ref() == Some(&space.id);
        let key = space.id.to_string();

        let name_field = match editing {
            Some(_) => div()
                .id(SharedString::from(format!("space-name-{key}")))
                .w(px(NAME_WIDTH))
                .h(px(INPUT_HEIGHT))
                .flex()
                .items_center()
                .px(px(10.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.subtle)
                .border_1()
                .border_color(with_alpha(ui.text, 0.25))
                .text_size(BODY)
                .text_color(ui.text)
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
                .child(self.space_name_input.clone())
                .into_any_element(),
            None => {
                let id = space.id.clone();
                div()
                    .id(SharedString::from(format!("space-name-{key}")))
                    .w(px(NAME_WIDTH))
                    .h(px(INPUT_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(ui.subtle)
                    .cursor(CursorStyle::IBeam)
                    .text_size(BODY)
                    .text_color(ui.text)
                    .accessible_control(
                        gpui::accesskit::Role::TextInput,
                        format!("Name of space {}", space.name),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.edit_space_name(id.clone(), window, cx);
                    }))
                    .child(div().truncate().child(space.name.clone()))
                    .into_any_element()
            }
        };

        let id = space.id.clone();
        let mut row = space_row_shell()
            .child(space_letter(&space.name, space.color, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(name_field)
                    .child(match editing.and_then(|editor| editor.error) {
                        Some(error) => div()
                            .text_size(LABEL_SM)
                            .text_color(destructive_color())
                            .child(error),
                        None => div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(workspace_count_label(count, "No workspaces")),
                    }),
            )
            .child(self.render_space_colors(space, ui, cx))
            .child(
                div()
                    .id(SharedString::from(format!("space-delete-{key}")))
                    .flex_none()
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(with_alpha(ui.text, 0.06)))
                    .role(gpui::accesskit::Role::Button)
                    .aria_label(format!("Delete space {}", space.name))
                    .delayed_tooltip(text_tooltip("Delete space"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.space_delete_armed = Some(id.clone());
                        this.space_name_editor = None;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/trash.svg")
                            .text_color(if armed { destructive_color() } else { ui.muted }),
                    ),
            );

        if armed {
            let id = space.id.clone();
            row = row.flex_wrap();
            let confirm = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .pt(px(8.))
                .child(
                    div()
                        .text_size(LABEL_SM)
                        .text_color(ui.text)
                        .child(space_delete_message(count)),
                )
                .child(
                    div()
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
                            destructive_button("space-delete-confirm", "Delete").on_click(
                                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                    this.delete_space(&id, cx);
                                }),
                            ),
                        ),
                );
            row = row.child(confirm);
        }
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(self.render_space_env(space, ui, cx))
            .into_any_element()
    }

    fn render_space_env(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = space.id.to_string();
        let editing = self
            .space_env_editor
            .as_ref()
            .filter(|editor| editor.id == space.id);
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pl(px(CARD_PADDING_X + ENV_INDENT))
            .pr(px(CARD_PADDING_X))
            .pb(px(10.));
        if let Some(editor) = editing {
            return column
                .child(self.render_space_env_editor(editor, ui, cx))
                .into_any_element();
        }
        let id = space.id.clone();
        column = column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_size(LABEL_SM)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.muted)
                        .child(format!("Environment: {}", env_summary(&space.env))),
                )
                .child(secondary_button(
                    SharedString::from(format!("space-env-edit-{key}")),
                    "Edit environment",
                    ui,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.open_space_env_editor(&id, cx);
                    }),
                )),
        );
        for warning in space_env_warnings(&space.env, crate::space_env::inherited_from_paneflow) {
            column = column.child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.vc_modified)
                    .child(warning),
            );
        }
        column = column.child(self.render_space_integrations(space, ui, cx));
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
                                .text_size(LABEL_SM)
                                .truncate()
                                .text_color(match tone {
                                    HintTone::Error => destructive_color(),
                                    HintTone::Warning => ui.vc_modified,
                                    HintTone::Preview => ui.muted,
                                })
                                .child(text),
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
                    .child("+ Add variable"),
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
        let mut column = div().flex().flex_col().gap(px(6.)).child(rows);
        if let Some(error) = editor.error.as_ref() {
            column = column.child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(destructive_color())
                    .child(error.clone()),
            );
        }
        column.child(footer).into_any_element()
    }

    fn render_space_colors(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = space.id.to_string();
        let mut swatches = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.));
        let choices = std::iter::once(None).chain(SPACE_COLORS.into_iter().map(Some));
        for choice in choices {
            let selected = space.color == choice;
            let id = space.id.clone();
            let label = choice.map_or("No color", space_color_name);
            let tint = crate::app::sidebar::space_tint(choice, ui);
            let swatch_id = format!(
                "space-color-{key}-{}",
                choice.map_or("none", |color| space_color_name(color))
            );
            swatches = swatches.child(
                div()
                    .id(SharedString::from(swatch_id))
                    .size(px(SWATCH_SIZE + 6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor(CursorStyle::PointingHand)
                    .when(selected, |swatch| {
                        swatch.border_1().border_color(with_alpha(ui.text, 0.6))
                    })
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(format!(
                        "{label}{}",
                        if selected { ", selected" } else { "" }
                    ))
                    .delayed_tooltip(text_tooltip(label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_space_color(&id, choice, cx);
                    }))
                    .child(match choice {
                        Some(_) => div()
                            .size(px(SWATCH_SIZE))
                            .rounded_full()
                            .bg(tint)
                            .into_any_element(),
                        None => div()
                            .size(px(SWATCH_SIZE))
                            .rounded_full()
                            .border_1()
                            .border_color(ui.muted)
                            .into_any_element(),
                    }),
            );
        }
        swatches.into_any_element()
    }
}

fn space_row_shell() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .px(px(CARD_PADDING_X))
        .py(px(10.))
}

fn space_letter(name: &str, color: Option<SpaceColor>, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .size(px(LETTER_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(ROW_RADIUS)
        .bg(with_alpha(ui.text, 0.06))
        .text_size(LABEL_SM)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(crate::app::sidebar::space_tint(color, ui))
        .child(crate::app::sidebar::space_initial(name))
        .into_any_element()
}

fn workspace_count_label(count: usize, empty: &'static str) -> String {
    match count {
        0 => empty.to_string(),
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    }
}

fn render_new_space_row(
    at_limit: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let row = div()
        .id("space-new")
        .h(px(36.))
        .px(px(CARD_PADDING_X))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .text_size(BODY)
        .text_color(ui.muted)
        .child("+ New space");
    if at_limit {
        return row
            .opacity(0.5)
            .accessible_control(
                gpui::accesskit::Role::Button,
                format!("New space, unavailable: {SPACE_LIMIT_REASON}"),
            )
            .child(div().text_size(LABEL_SM).child(SPACE_LIMIT_REASON))
            .into_any_element();
    }
    row.cursor(CursorStyle::PointingHand)
        .accessible_control(gpui::accesskit::Role::Button, "New space")
        .hover(move |style| style.text_color(ui.text).bg(with_alpha(ui.text, 0.05)))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            if let Some(id) = this.add_space(cx) {
                this.edit_space_name(id, window, cx);
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(n: u8, name: &str) -> SpaceConfig {
        SpaceConfig {
            id: SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap(),
            name: name.to_string(),
            color: None,
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
    }
}
