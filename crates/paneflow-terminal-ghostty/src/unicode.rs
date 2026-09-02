use paneflow_libghostty_sys as sys;

#[must_use]
pub fn codepoint_width(codepoint: u32) -> u8 {
    unsafe { sys::ghostty_unicode_codepoint_width(codepoint) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphemeCluster {
    pub consumed: usize,
    pub width: u8,
}

#[must_use]
pub fn grapheme_width(codepoints: &[u32]) -> GraphemeCluster {
    if codepoints.is_empty() {
        return GraphemeCluster {
            consumed: 0,
            width: 0,
        };
    }
    let mut width = 0u8;
    let consumed = unsafe {
        sys::ghostty_unicode_grapheme_width(codepoints.as_ptr(), codepoints.len(), &mut width)
    };
    GraphemeCluster { consumed, width }
}

#[must_use]
pub fn text_width(text: &str) -> usize {
    let codepoints: Vec<u32> = text.chars().map(u32::from).collect();
    let mut total = 0usize;
    let mut index = 0usize;
    while index < codepoints.len() {
        let cluster = grapheme_width(&codepoints[index..]);
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
        assert_eq!(codepoint_width(0x4e16), 2);
        assert_eq!(codepoint_width(0x0301), 0);
        assert_eq!(codepoint_width(0x07), 0);
    }

    #[test]
    fn clusters_fold_variation_selectors_into_one_width() {
        let heart = [0x2764u32, 0xfe0f];
        let cluster = grapheme_width(&heart);
        assert_eq!(cluster.consumed, 2);
        assert_eq!(cluster.width, 2);

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
