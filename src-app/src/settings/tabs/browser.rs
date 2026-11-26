use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px,
};
use paneflow_config::schema::{SearchEngine, ZOOM_LEVELS, search_template_host};

use crate::settings::components::{
    deferred_select_menu, select_chevron, select_menu, select_option, select_trigger,
};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::AccessibleControlExt;
use crate::{BrowserDropdown, PaneFlowApp};

const BROWSER_INTRO: &str = "Where local links open, how the address field searches, how big pages show, and what is remembered.";

pub(crate) const CLEAR_SPANS: [(&str, u64); 5] = [
    ("Last hour", 60 * 60),
    ("Last 24 hours", 24 * 60 * 60),
    ("Last 7 days", 7 * 24 * 60 * 60),
    ("Last 4 weeks", 28 * 24 * 60 * 60),
    ("All time", u64::MAX),
];

pub(crate) const CLEAR_HISTORY_BUTTON: &str = "browser-clear-history";
pub(crate) const RESET_ZOOMS_BUTTON: &str = "browser-reset-zooms";

type Pick = Rc<dyn Fn(&mut PaneFlowApp, &mut Context<PaneFlowApp>)>;

struct Choice {
    label: String,
    selected: bool,
    pick: Pick,
}

impl Choice {
    fn new(
        label: impl Into<String>,
        selected: bool,
        pick: impl Fn(&mut PaneFlowApp, &mut Context<PaneFlowApp>) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            selected,
            pick: Rc::new(pick),
        }
    }
}

pub(crate) fn pages_kept(count: usize, in_range: usize) -> String {
    match count {
        0 => "Nothing kept yet.".to_string(),
        1 => format!("1 page kept, {in_range} in this range."),
        count => format!("{count} pages kept, {in_range} in this range."),
    }
}

pub(crate) fn clear_since(span: u64, now: u64) -> u64 {
    now.saturating_sub(span)
}

impl PaneFlowApp {
    pub(crate) fn render_browser_heading(&self, title: Div) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(ink, BROWSER_INTRO))
            .into_any_element()
    }

    pub(crate) fn seed_browser_settings(&mut self, cx: &mut Context<Self>) {
        let stored = self
            .cached_config
            .browser
            .custom_search
            .clone()
            .unwrap_or_default();
        self.browser_custom_search_input.update(cx, |input, cx| {
            if input.value() != stored {
                input.set_value(stored, cx);
            }
        });
    }

    pub(crate) fn browser_custom_search_edited(&mut self, cx: &mut Context<Self>) {
        if self.settings_section != Some(crate::SettingsSection::Browser) {
            return;
        }
        let typed = self.browser_custom_search_input.read(cx).value();
        let typed = typed.trim();
        let stored = self.cached_config.browser.custom_search.as_deref();
        if stored.unwrap_or_default() == typed {
            return;
        }
        let value = (!typed.is_empty()).then(|| typed.to_string());
        self.persist_browser(move |browser| browser.custom_search = value, cx);
    }

    pub(crate) fn render_browser_content(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::browser::omnibox::start(cx);
        crate::browser::zoom::start(cx);
        let ink = Ink::of(crate::theme::ui_colors());
        let browser = self.cached_config.browser.clone();
        let mut first = true;
        let mut page = div().flex().flex_col();

        let engine = browser.search_engine();
        let engines = SearchEngine::ALL
            .iter()
            .map(|choice| {
                let choice = *choice;
                Choice::new(choice.name(), choice == engine, move |this, cx| {
                    this.persist_browser(|browser| browser.search_engine = Some(choice), cx);
                    if choice == SearchEngine::Custom {
                        this.seed_browser_settings(cx);
                    }
                })
            })
            .collect();
        let mut search_card = SearchCard::new()
            .row(
                &search::BROWSER_LOCAL_LINKS,
                self.browser_switch(
                    "browser-local-links",
                    &search::BROWSER_LOCAL_LINKS,
                    browser.opens_local_links(),
                    |browser, on| browser.open_local_links = Some(on),
                    ink,
                    cx,
                ),
            )
            .row(
                &search::BROWSER_SEARCH_ENGINE,
                self.browser_select(
                    BrowserDropdown::Engine,
                    &search::BROWSER_SEARCH_ENGINE,
                    engine.name().to_string(),
                    engines,
                    ink,
                    cx,
                ),
            );
        if engine == SearchEngine::Custom {
            search_card = search_card.row(
                &search::BROWSER_CUSTOM_SEARCH,
                self.browser_custom_search_row(ink, cx),
            );
        }
        page = page.child(section(ink, "Links and search", search_card, &mut first));

        let default_zoom = browser.default_zoom();
        let zooms = ZOOM_LEVELS
            .iter()
            .map(|level| {
                let level = *level;
                Choice::new(
                    format!("{level}%"),
                    level == default_zoom,
                    move |this, cx| {
                        this.persist_browser(|browser| browser.default_zoom = Some(level), cx);
                    },
                )
            })
            .collect();
        let mut zoom_card = SearchCard::new().row(
            &search::BROWSER_DEFAULT_ZOOM,
            self.browser_select(
                BrowserDropdown::Zoom,
                &search::BROWSER_DEFAULT_ZOOM,
                format!("{default_zoom}%"),
                zooms,
                ink,
                cx,
            ),
        );
        let sites = crate::browser::zoom::snapshot(cx).list();
        if !sites.is_empty() {
            zoom_card = zoom_card.row(&search::BROWSER_SITE_ZOOM, site_zooms(ink, sites, cx));
        }
        page = page.child(section(ink, "Zoom", zoom_card, &mut first));

        let history = SearchCard::new()
            .row(
                &search::BROWSER_KEEP_HISTORY,
                self.browser_switch(
                    "browser-keep-history",
                    &search::BROWSER_KEEP_HISTORY,
                    browser.keeps_history(),
                    |browser, on| browser.keep_history = Some(on),
                    ink,
                    cx,
                ),
            )
            .row(
                &search::BROWSER_CLEAR_HISTORY,
                self.browser_clear_row(ink, cx),
            );
        page = page.child(section(ink, "History", history, &mut first));
        page.into_any_element()
    }

    fn browser_switch(
        &self,
        id: &'static str,
        copy: &'static search::SettingCopy,
        on: bool,
        set: fn(&mut paneflow_config::schema::BrowserConfig, bool),
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state: SharedString = format!("{id}:{}", if on { "on" } else { "off" }).into();
        kit::row(
            ink,
            None,
            setting_text(copy, ink),
            Some(
                kit::switch(id, copy.title, on, ink)
                    .debug_selector(move || state.to_string())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.persist_browser(|browser| set(browser, !on), cx);
                    }))
                    .into_any_element(),
            ),
        )
        .into_any_element()
    }

    fn browser_select(
        &self,
        which: BrowserDropdown,
        copy: &'static search::SettingCopy,
        current: String,
        choices: Vec<Choice>,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        kit::row(
            ink,
            None,
            setting_text(copy, ink),
            Some(self.browser_dropdown(which, copy.title, current, choices, ink, cx)),
        )
        .into_any_element()
    }

    fn browser_dropdown(
        &self,
        which: BrowserDropdown,
        name: &'static str,
        current: String,
        choices: Vec<Choice>,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let is_open = self.browser_dropdown == Some(which);
        let key = dropdown_key(which);
        let mut trigger = select_trigger(SharedString::from(key), name, is_open, ui)
            .on_press(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.browser_dropdown = if is_open { None } else { Some(which) };
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(kit::SIZE_SM))
                    .text_color(ink.primary)
                    .truncate()
                    .child(current),
            )
            .child(select_chevron(ui));
        if is_open {
            let mut menu = select_menu(SharedString::from(format!("{key}-list")), ui)
                .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                    if this.browser_dropdown == Some(which) {
                        this.browser_dropdown = None;
                        cx.notify();
                    }
                }));
            for (index, choice) in choices.into_iter().enumerate() {
                let item = select_option((key, index), choice.selected, ui)
                    .cursor(CursorStyle::Arrow)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.primary)
                            .child(choice.label),
                    );
                let pick = choice.pick;
                menu = menu.option(
                    item,
                    cx.listener(move |this, _: &(), _, cx| {
                        this.browser_dropdown = None;
                        pick(this, cx);
                        cx.notify();
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }
        trigger.into_any_element()
    }

    fn browser_custom_search_row(&self, ink: Ink, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let typed = self.browser_custom_search_input.read(cx).value();
        let unusable = !typed.trim().is_empty() && search_template_host(typed.trim()).is_none();
        let line = if unusable {
            kit::line_in(
                ink.danger,
                "Use a web address with %s after the site, such as \
                 https://example.com/search?q=%s. Until then, DuckDuckGo searches.",
            )
        } else {
            kit::line(ink, search::BROWSER_CUSTOM_SEARCH.description)
        };
        let field = crate::settings::components::input_frame(
            div().w_full().h(px(28.)).flex().items_center().px(px(10.)),
            &self.browser_custom_search_input,
            unusable,
            ui,
        )
        .child(self.browser_custom_search_input.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(kit::wide_row(
                ink,
                None,
                Text::new(search::BROWSER_CUSTOM_SEARCH.title).line(line),
                None,
            ))
            .child(div().px(px(kit::TEXT_INSET)).pb(px(12.)).child(field))
            .into_any_element()
    }

    fn browser_clear_row(&self, ink: Ink, cx: &mut Context<Self>) -> AnyElement {
        let history = crate::browser::omnibox::history_snapshot(cx);
        let loaded = crate::browser::omnibox::loaded(cx);
        let span = self.browser_clear_span.min(CLEAR_SPANS.len() - 1);
        let line = if loaded {
            let since = clear_since(CLEAR_SPANS[span].1, crate::browser::omnibox::now());
            pages_kept(history.len(), history.kept_since(since))
        } else {
            "Reading the history\u{2026}".to_string()
        };
        let spans = CLEAR_SPANS
            .iter()
            .enumerate()
            .map(|(index, (label, _))| {
                Choice::new(*label, index == span, move |this, _| {
                    this.browser_clear_span = index;
                })
            })
            .collect();
        let range = self.browser_dropdown(
            BrowserDropdown::ClearSpan,
            "Time range",
            CLEAR_SPANS[span].0.to_string(),
            spans,
            ink,
            cx,
        );
        let clear = kit::button(
            CLEAR_HISTORY_BUTTON,
            "Clear",
            Tone::Secondary,
            Size::Small,
            ink,
        )
        .debug_selector(|| CLEAR_HISTORY_BUTTON.into())
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            let since = clear_since(CLEAR_SPANS[span].1, crate::browser::omnibox::now());
            crate::browser::omnibox::clear_since(since, cx);
            this.show_toast("History cleared", cx);
            cx.notify();
        }));
        kit::row(
            ink,
            None,
            Text::new(search::BROWSER_CLEAR_HISTORY.title).line(kit::line(ink, line)),
            Some(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().child(range))
                    .child(clear)
                    .into_any_element(),
            ),
        )
        .into_any_element()
    }
}

fn dropdown_key(which: BrowserDropdown) -> &'static str {
    match which {
        BrowserDropdown::Engine => "browser-dd-engine",
        BrowserDropdown::Zoom => "browser-dd-zoom",
        BrowserDropdown::ClearSpan => "browser-dd-clear-span",
    }
}

fn site_zooms(ink: Ink, sites: Vec<(String, u16)>, cx: &mut Context<PaneFlowApp>) -> AnyElement {
    let count = sites.len();
    let title = if count == 1 {
        "1 site with its own zoom".to_string()
    } else {
        format!("{count} sites with their own zoom")
    };
    let reset_all = kit::button(
        RESET_ZOOMS_BUTTON,
        "Reset all",
        Tone::Ghost,
        Size::Small,
        ink,
    )
    .debug_selector(|| RESET_ZOOMS_BUTTON.into())
    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
        crate::browser::zoom::reset_all(cx);
        cx.notify();
    }));
    let mut list = div().flex().flex_col().child(kit::row(
        ink,
        None,
        Text::new(title).line(kit::line(ink, search::BROWSER_SITE_ZOOM.description)),
        Some(reset_all.into_any_element()),
    ));
    for (index, (site, percent)) in sites.into_iter().enumerate() {
        let host = site.clone();
        let reset = kit::icon_button(
            ("browser-reset-zoom", index),
            SharedString::from(format!("Reset zoom for {site}")),
            "icons/close.svg",
            ink,
        )
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            crate::browser::zoom::reset(&host, cx);
            cx.notify();
        }));
        list = list.child(kit::row(
            ink,
            None,
            Text::new(site),
            Some(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(kit::SIZE_SM))
                            .text_color(ink.secondary)
                            .child(format!("{percent}%")),
                    )
                    .child(reset)
                    .into_any_element(),
            ),
        ));
    }
    list.into_any_element()
}

fn section(ink: Ink, title: &'static str, card: SearchCard, first: &mut bool) -> AnyElement {
    let (matched, rows) = card.into_rows();
    Block::new(title)
        .top_gap(kit::section_gap(first))
        .found(matched)
        .child(kit::section(ink, title, None, kit::search_group(ink, rows)))
        .finish()
}

fn setting_text(copy: &search::SettingCopy, ink: Ink) -> Text {
    Text::new(copy.title).line(kit::line(ink, copy.description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clear_range_starts_its_span_before_now_and_all_time_starts_at_zero() {
        let now = 1_800_000_000;
        assert_eq!(clear_since(CLEAR_SPANS[0].1, now), now - 3600);
        assert_eq!(clear_since(CLEAR_SPANS[4].1, now), 0);
        assert_eq!(pages_kept(0, 0), "Nothing kept yet.");
        assert_eq!(pages_kept(1, 0), "1 page kept, 0 in this range.");
        assert_eq!(pages_kept(12, 3), "12 pages kept, 3 in this range.");
    }
}
