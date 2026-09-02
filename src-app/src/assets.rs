use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*"]
#[include = "agents/**/*"]
#[include = "fonts/**/*"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| {
                if p.starts_with(path) {
                    Some(SharedString::from(p.to_string()))
                } else {
                    None
                }
            })
            .collect())
    }
}

impl Assets {
    pub fn load_fonts(&self, cx: &App) -> Result<()> {
        let font_paths = self.list("fonts/")?;
        let mut embedded_fonts = Vec::with_capacity(font_paths.len());
        for path in &font_paths {
            let lower = path.to_lowercase();
            if !lower.ends_with(".ttf") && !lower.ends_with(".otf") {
                continue;
            }
            let data = self
                .load(path)?
                .ok_or_else(|| anyhow::anyhow!("embedded font {path} listed but not loadable"))?;
            embedded_fonts.push(data);
        }
        if embedded_fonts.is_empty() {
            log::warn!(
                "Assets::load_fonts: no .ttf/.otf found under fonts/ - \
                 the rust-embed include set may have drifted"
            );
            return Ok(());
        }
        let count = embedded_fonts.len();
        cx.text_system().add_fonts(embedded_fonts)?;
        log::info!("Assets::load_fonts: registered {count} embedded font file(s) with GPUI");
        Ok(())
    }
}

#[derive(RustEmbed)]
#[folder = "target/embed/bin"]
#[prefix = "bin/"]
pub struct Bins;
