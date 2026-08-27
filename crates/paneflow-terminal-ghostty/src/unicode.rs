//! Terminal-accurate Unicode width helpers.
//!
//! These use the exact width tables libghostty applies when it lays out
//! printed text, so callers can predict column layout for text that has not
//! reached the terminal yet: IME preedit overlays, prompt padding, or
//! truncation of a line before it is written.

use paneflow_libghostty_sys as sys;

/// The display width of `codepoint` in grid cells: 0, 1, or 2.
///
/// This is per-codepoint and therefore cannot account for cluster-level rules
/// such as VS15/VS16 presentation selectors. Summing it over a string is only
/// correct when grapheme clustering (mode 2027) is disabled; use
/// [`text_width`] otherwise.
#[must_use]
pub fn codepoint_width(codepoint: u32) -> u8 {
    // SAFETY: the function is pure, total, and takes the codepoint by value.
    unsafe { sys::ghostty_unicode_codepoint_width(codepoint) }
}

/// The first grapheme cluster of `codepoints`, measured with the same
/// segmentation rules the terminal uses under mode 2027.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphemeCluster {
    /// Number of codepoints the cluster consumed. Zero only when the input
    /// was empty.
    pub consumed: usize,
    /// Cluster display width in cells: 0, 1, or 2.
    pub width: u8,
}

/// Measure the first grapheme cluster in `codepoints`.
///
/// The slice must hold a complete cluster or the logical end of the text: a
/// later codepoint can still extend a cluster and change its width, so a
/// streaming caller must keep buffering while `consumed == codepoints.len()`.
#[must_use]
pub fn grapheme_width(codepoints: &[u32]) -> GraphemeCluster {
    if codepoints.is_empty() {
        return GraphemeCluster {
            consumed: 0,
            width: 0,
        };
    }
    let mut width = 0u8;
    // SAFETY: the pointer and length describe a live slice, and `width` is
    // valid writable storage for the out-parameter.
    let consumed = unsafe {
        sys::ghostty_unicode_grapheme_width(codepoints.as_ptr(), codepoints.len(), &mut width)
    };
    GraphemeCluster { consumed, width }
}

/// The total display width of `text` in grid cells, segmenting it into
/// grapheme clusters exactly as the terminal would under mode 2027.
#[must_use]
pub fn text_width(text: &str) -> usize {
    let codepoints: Vec<u32> = text.chars().map(u32::from).collect();
    let mut total = 0usize;
    let mut index = 0usize;
    while index < codepoints.len() {
        let cluster = grapheme_width(&codepoints[index..]);
        // `grapheme_width` consumes at least one codepoint on a non-empty
        // slice, but a defensive step keeps a future ABI change from spinning
        // here forever.
        index += cluster.consumed.max(1);
        total += usize::from(cluster.width);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepoint_widths_follow_the_terminal_tables() {
        assert_eq!(codepoint_width(u32::from('a')), 1);
        // CJK ideographs are wide.
        assert_eq!(codepoint_width(0x4e16), 2);
        // Combining acute accent is zero width.
        assert_eq!(codepoint_width(0x0301), 0);
        // C0 controls are zero width.
        assert_eq!(codepoint_width(0x07), 0);
    }

    #[test]
    fn clusters_fold_variation_selectors_into_one_width() {
        // U+2764 U+FE0F is a single cluster forced wide by VS16.
        let heart = [0x2764u32, 0xfe0f];
        let cluster = grapheme_width(&heart);
        assert_eq!(cluster.consumed, 2);
        assert_eq!(cluster.width, 2);

        // Summing per-codepoint widths would report 1 here, which is why
        // `text_width` exists.
        assert_eq!(text_width("\u{2764}\u{fe0f}"), 2);
        assert_eq!(text_width("ab"), 2);
        assert_eq!(text_width(""), 0);
    }

    #[test]
    fn an_empty_slice_consumes_nothing() {
        assert_eq!(
            grapheme_width(&[]),
            GraphemeCluster {
                consumed: 0,
                width: 0
            }
        );
    }
}
