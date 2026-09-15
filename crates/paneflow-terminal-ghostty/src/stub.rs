use crate::{
    BackendEvent, Content, FocusEvent, GhosttyError, Hyperlink, KeyInput, Modes, MouseInput, Point,
    Result, Scroll, SearchResult, SelectionRange, TerminalAppearance, WindowSize,
};

pub struct DisplayTerminal;

impl DisplayTerminal {
    pub fn new(_: WindowSize, _: usize, _: TerminalAppearance) -> Result<Self> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn feed(&mut self, _: &[u8]) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn resize(&mut self, _: WindowSize) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn reset(&mut self) {}

    pub fn clear_screen_and_scrollback(&mut self) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn snapshot(&mut self) -> Result<Content> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn modes(&self) -> Result<Modes> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn synchronized_output(&self) -> Result<bool> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn scroll(&mut self, _: Scroll) {}

    pub fn scroll_to_viewport_row(&mut self, _: usize) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn drain_events(&mut self) -> Vec<BackendEvent> {
        Vec::new()
    }

    pub fn encode_key(&mut self, _: &KeyInput) -> Result<Vec<u8>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn encode_mouse(&mut self, _: MouseInput) -> Result<Vec<u8>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn encode_focus(&self, _: FocusEvent) -> Result<Vec<u8>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn search(&self, _: &str, _: bool) -> Result<SearchResult> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn set_search_query(&mut self, _: &str) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn clear_search(&mut self) {}

    pub fn search_step(&mut self) -> Result<Option<crate::NativeSearchSnapshot>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn search_step_with_rail_refresh(
        &mut self,
        _: bool,
    ) -> Result<Option<crate::NativeSearchSnapshot>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn search_select(&mut self, _: bool) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn set_selection(&mut self, _: SelectionRange) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn select_word(&mut self, _: Point) -> Result<bool> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn select_line(&mut self, _: Point) -> Result<bool> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn selection_text(&self) -> Result<Option<String>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn clear_selection(&mut self) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn select_all(&mut self) -> Result<bool> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn selection_range(&self) -> Result<Option<crate::SelectionRange>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn hyperlink_at(&self, _: Point) -> Result<Option<Hyperlink>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn extract_scrollback(&self) -> Result<Option<String>> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn restore_scrollback(&mut self, _: &str) -> Result<()> {
        Err(GhosttyError::UnsupportedPlatform)
    }

    pub fn paste(
        &mut self,
        _: &[crate::PasteRepresentation<'_>],
        _: crate::ClipboardLocation,
        _: bool,
    ) -> Result<bool> {
        Err(GhosttyError::UnsupportedPlatform)
    }
}
