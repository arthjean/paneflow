//! Pulldown-cmark event walker → owned markdown AST.
//!
//! pulldown-cmark 0.10 emits a flat event stream (Start/End wrap blocks and
//! inline ranges). We walk those events with a stack of in-progress block
//! contexts, accumulating inline `Span`s into the open block, and pop a
//! finished node into the parent (or into the root list) on each `Event::End`.
//!
//! The AST is fully owned (`String`, not `CowStr`) so `MarkdownView` can keep
//! it across re-renders without borrowing from the original input.

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd, TextMergeStream,
};

/// Hard upper bound on input size accepted by `parse_with_limit`. Chosen per
/// PRD AC: 10 MB → return a `TooLarge` error so the view shows a warning
/// instead of allocating a many-million-element AST that would thrash GPUI
/// layout. 100 KB (the AC's "fast path" budget) parses in well under 5 ms.
pub const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;

/// Defensive cap on AST node count to bound heap usage on adversarial
/// inputs. A 9.99 MB file consisting entirely of `- ` bullet markers parses
/// inside `MAX_INPUT_BYTES` but would otherwise produce ~5M `MdNode`
/// entries (~250 MB heap). 100 K nodes covers any realistic markdown
/// document - the `cmark` test suite's longest spec example produces ~1500.
pub const MAX_AST_NODES: usize = 100_000;

/// Errors returned by `parse_with_limit`.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Input exceeded `MAX_INPUT_BYTES`. Carries the actual byte length.
    TooLarge { bytes: usize, limit: usize },
}

/// Inline character styling. Combined as we descend into nested
/// emphasis/strong/strikethrough/code spans, recovered as we ascend.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SpanStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub strikethrough: bool,
    /// Inline `code` span. Mutually exclusive with the surrounding paragraph
    /// styling - code spans render as monospace on a code-cell background.
    pub code: bool,
}

/// Inline run of text with a uniform style. Adjacent spans with identical
/// style are coalesced opportunistically by the walker; consumers can rely on
/// each span being a single styled run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
    /// `Some(url)` when this span is the visible label of a `Tag::Link`.
    /// The view layer renders these underlined and routes clicks through
    /// `open::that` (cross-platform URL/file handler).
    pub link_url: Option<String>,
}

/// Owned markdown AST node. Each variant maps 1:1 to a block-level pulldown
/// event pair (`Start` … `End`) plus the inline spans that arrived between.
///
/// Inline-only structures (Strong, Emphasis, Strikethrough, Link) are folded
/// into `Span` rather than appearing as separate nodes - they only affect
/// styling, not block layout.
///
/// `Eq` is not derived because `pulldown_cmark::Alignment` only implements
/// `PartialEq`; tests use `matches!` patterns rather than `assert_eq!` for
/// nodes that carry it.
#[derive(Clone, Debug, PartialEq)]
pub enum MdNode {
    Heading {
        level: HeadingLevel,
        spans: Vec<Span>,
    },
    Paragraph {
        spans: Vec<Span>,
    },
    /// Verbatim code block. `lang` is `Some` for fenced blocks with an info
    /// string (` ```rust `), `None` for indented blocks or fences without a
    /// language tag. Syntax highlighting is intentionally deferred (P2).
    CodeBlock {
        lang: Option<String>,
        text: String,
    },
    BlockQuote {
        children: Vec<MdNode>,
    },
    List {
        /// `Some(start_num)` for ordered lists with that starting number;
        /// `None` for bulleted lists.
        ordered_start: Option<u64>,
        items: Vec<Vec<MdNode>>,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// `<hr/>` - horizontal rule.
    Rule,
    /// Footnote definition body. `label` is the user-supplied identifier,
    /// `children` is the rendered contents.
    Footnote {
        label: String,
        children: Vec<MdNode>,
    },
}

/// Parse `input` into an owned AST.
///
/// Returns `Err(ParseError::TooLarge)` when the input exceeds
/// `MAX_INPUT_BYTES`. Smaller inputs always succeed - pulldown-cmark itself
/// is total: any byte sequence parses (mis-formed markdown turns into a
/// stream of plain paragraphs/text rather than an error).
pub fn parse_with_limit(input: &str) -> Result<Vec<MdNode>, ParseError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(ParseError::TooLarge {
            bytes: input.len(),
            limit: MAX_INPUT_BYTES,
        });
    }
    Ok(parse_inner(input))
}

fn parse_inner(input: &str) -> Vec<MdNode> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let events = TextMergeStream::new(Parser::new_ext(input, opts));
    Walker::default().drive(events)
}

// ---------------------------------------------------------------------------
// Walker - converts the event stream to nested `MdNode` values.
// ---------------------------------------------------------------------------

/// One frame on the block-context stack. The walker pushes a frame on
/// `Event::Start(<block tag>)` and pops it on the matching `Event::End` to
/// install a finished `MdNode` in the parent frame (or in the root vector).
enum Frame {
    Paragraph(Vec<Span>),
    Heading(HeadingLevel, Vec<Span>),
    Code {
        lang: Option<String>,
        text: String,
    },
    Quote(Vec<MdNode>),
    List {
        ordered_start: Option<u64>,
        items: Vec<Vec<MdNode>>,
    },
    /// Currently-open list item. Inline text accumulates into a virtual
    /// paragraph child unless a nested block opens.
    Item {
        children: Vec<MdNode>,
        inline: Vec<Span>,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
        in_head: bool,
        current_row: Vec<Vec<Span>>,
        current_cell: Vec<Span>,
    },
    Footnote {
        label: String,
        children: Vec<MdNode>,
    },
}

#[derive(Default)]
struct Walker {
    stack: Vec<Frame>,
    style: SpanStyle,
    /// When `Some`, every text/code event is also given this URL so the view
    /// layer can render it as a hyperlink.
    link_url: Option<String>,
    /// Pulldown emits the image alt text as nested inline events after
    /// `Tag::Image`. We render one placeholder for the image itself, so the
    /// nested alt stream must be drained instead of appended a second time.
    image_depth: usize,
    output: Vec<MdNode>,
    /// Running tally of `MdNode` instances installed (in `output` or in any
    /// child `Vec<MdNode>`). When this exceeds `MAX_AST_NODES`, the walker
    /// stops accepting further block-installs and lets the remaining events
    /// drain into the abyss - the partial AST we already built is what the
    /// viewer will render.
    node_count: usize,
    /// Set once `node_count` has crossed `MAX_AST_NODES`. Used to append a
    /// single truncation-notice paragraph at the end of `output`.
    truncated: bool,
}

impl Walker {
    fn reserve_synthetic_node(&mut self) -> bool {
        if self.node_count >= MAX_AST_NODES {
            self.truncated = true;
            false
        } else {
            self.node_count += 1;
            true
        }
    }

    fn drive<'a, I: Iterator<Item = Event<'a>>>(mut self, events: I) -> Vec<MdNode> {
        for event in events {
            self.on_event(event);
        }
        if self.truncated {
            self.output.push(MdNode::Paragraph {
                spans: vec![Span {
                    text: format!(
                        "[markdown viewer: document truncated after {} nodes]",
                        MAX_AST_NODES
                    ),
                    style: SpanStyle::default(),
                    link_url: None,
                }],
            });
        }
        self.output
    }

    fn on_event(&mut self, event: Event<'_>) {
        if self.image_depth > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => {
                    self.image_depth += 1;
                }
                Event::End(TagEnd::Image) => {
                    self.image_depth = self.image_depth.saturating_sub(1);
                }
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(tag) => self.on_start(tag),
            Event::End(end) => self.on_end(end),
            Event::Text(text) => self.push_text(text.into_string()),
            Event::Code(text) => {
                let mut style = self.style;
                style.code = true;
                // U-019: inline code reaches the renderer via `push_span`, NOT
                // `push_text`, so it must be bidi/zero-width-stripped here too -
                // otherwise `` `\u{202E}txet.exe` `` renders reversed and the AC's
                // "every rendered span" guarantee leaks through code spans.
                self.push_span(Span {
                    text: strip_bidi_zero_width(text.into_string()),
                    style,
                    link_url: self.link_url.clone(),
                });
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                // HTML-inside-block: append as plain text. HTML *blocks* arrive
                // wrapped in `Tag::HtmlBlock` and are handled in on_start.
                self.push_text(text.into_string());
            }
            Event::FootnoteReference(label) => {
                self.push_text(format!("[^{}]", label));
            }
            Event::SoftBreak => self.push_text(" ".to_string()),
            Event::HardBreak => self.push_text("\n".to_string()),
            Event::Rule => self.install(MdNode::Rule),
            Event::TaskListMarker(checked) => {
                let glyph = if checked { "[x] " } else { "[ ] " };
                self.push_text(glyph.to_string());
            }
        }
    }

    fn on_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.stack.push(Frame::Paragraph(Vec::new())),
            Tag::Heading { level, .. } => self.stack.push(Frame::Heading(level, Vec::new())),
            Tag::BlockQuote => self.stack.push(Frame::Quote(Vec::new())),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.into_string()),
                    _ => None,
                };
                self.stack.push(Frame::Code {
                    lang,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => self.stack.push(Frame::Code {
                // Render HTML blocks as opaque code blocks tagged "html". The
                // view layer will style them as monospace on the code-cell
                // background - no execution, no interpretation.
                lang: Some("html".to_string()),
                text: String::new(),
            }),
            Tag::List(start) => self.stack.push(Frame::List {
                ordered_start: start,
                items: Vec::new(),
            }),
            Tag::Item => self.stack.push(Frame::Item {
                children: Vec::new(),
                inline: Vec::new(),
            }),
            Tag::Table(alignments) => self.stack.push(Frame::Table {
                alignments,
                header: Vec::new(),
                rows: Vec::new(),
                in_head: false,
                current_row: Vec::new(),
                current_cell: Vec::new(),
            }),
            Tag::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow | Tag::TableCell => {
                // No frame - TableRow/Cell content accumulates into the
                // current_row / current_cell fields of the open Table frame.
            }
            Tag::Emphasis => self.style.emphasis = true,
            Tag::Strong => self.style.strong = true,
            Tag::Strikethrough => self.style.strikethrough = true,
            Tag::Link { dest_url, .. } => {
                self.link_url = Some(dest_url.into_string());
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                // We don't render images inline - show "[image: <title or url>]"
                // as a placeholder span so the user knows something was elided.
                // `title`/`dest_url` are attacker-controlled `.md` content, so
                // sanitise before embedding: strip bidi/zero-width code points
                // (a U+202E override could reorder the visible label to spoof a
                // benign path) and cap the length (US-044).
                let raw = if title.is_empty() {
                    dest_url.as_ref()
                } else {
                    title.as_ref()
                };
                self.push_text(format!("[image: {}]", sanitize_placeholder(raw)));
                self.image_depth = 1;
            }
            Tag::FootnoteDefinition(label) => self.stack.push(Frame::Footnote {
                label: label.into_string(),
                children: Vec::new(),
            }),
            Tag::MetadataBlock(_) => {
                // YAML / TOML frontmatter - push a Paragraph frame so contents
                // accumulate as plain text rather than panic.
                self.stack.push(Frame::Paragraph(Vec::new()));
            }
        }
    }

    fn on_end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Emphasis => self.style.emphasis = false,
            TagEnd::Strong => self.style.strong = false,
            TagEnd::Strikethrough => self.style.strikethrough = false,
            TagEnd::Link => self.link_url = None,
            TagEnd::Image => {
                // Handled by the `image_depth` drain at the top of `on_event`.
            }
            TagEnd::TableHead => {
                let keep_row = self.reserve_synthetic_node();
                if let Some(Frame::Table {
                    in_head,
                    header,
                    current_row,
                    ..
                }) = self.stack.last_mut()
                {
                    if keep_row {
                        *header = std::mem::take(current_row);
                    } else {
                        current_row.clear();
                    }
                    *in_head = false;
                }
            }
            TagEnd::TableRow => {
                let keep_row = self.reserve_synthetic_node();
                if let Some(Frame::Table {
                    in_head,
                    rows,
                    current_row,
                    ..
                }) = self.stack.last_mut()
                {
                    let row = std::mem::take(current_row);
                    if keep_row && !*in_head {
                        rows.push(row);
                    }
                }
            }
            TagEnd::TableCell => {
                let keep_cell = self.reserve_synthetic_node();
                if let Some(Frame::Table {
                    current_row,
                    current_cell,
                    ..
                }) = self.stack.last_mut()
                {
                    if keep_cell {
                        current_row.push(std::mem::take(current_cell));
                    } else {
                        current_cell.clear();
                    }
                }
            }
            TagEnd::MetadataBlock(_) => {
                // Discard frontmatter - pop without installing.
                self.stack.pop();
            }
            // Closing block tags: install into parent.
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::FootnoteDefinition => {
                // Stack underflow on a malformed event stream is not a panic
                // condition - the workspace lints `panic = "deny"` and the
                // input is untrusted (any 10 MB of bytes parses). Tolerate by
                // dropping the stray end event silently, matching the rest of
                // the walker's malformed-input behavior.
                if let Some(frame) = self.stack.pop()
                    && let Some(n) = self.finish_block(frame)
                {
                    self.install(n);
                }
            }
        }
    }

    fn finish_block(&mut self, frame: Frame) -> Option<MdNode> {
        match frame {
            Frame::Paragraph(spans) => Some(MdNode::Paragraph { spans }),
            Frame::Heading(level, spans) => Some(MdNode::Heading { level, spans }),
            Frame::Code { lang, text } => Some(MdNode::CodeBlock { lang, text }),
            Frame::Quote(children) => Some(MdNode::BlockQuote { children }),
            Frame::List {
                ordered_start,
                items,
            } => Some(MdNode::List {
                ordered_start,
                items,
            }),
            Frame::Item {
                mut children,
                inline,
            } => {
                let keep_item = self.reserve_synthetic_node();
                // The synthesised Paragraph for inline-only items is a node
                // that bypasses `install()` - count it here so adversarial
                // bullet-only inputs respect `MAX_AST_NODES`.
                if keep_item && !inline.is_empty() {
                    if self.node_count >= MAX_AST_NODES {
                        self.truncated = true;
                    } else {
                        self.node_count += 1;
                        children.push(MdNode::Paragraph { spans: inline });
                    }
                }
                if keep_item && let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(children);
                }
                None
            }
            Frame::Table {
                alignments,
                header,
                rows,
                ..
            } => Some(MdNode::Table {
                alignments,
                header,
                rows,
            }),
            Frame::Footnote { label, children } => Some(MdNode::Footnote { label, children }),
        }
    }

    /// Append text as a span to the deepest open block that accepts inline
    /// content. CodeBlock frames receive raw text concatenated into `text`;
    /// table cells append into `current_cell`; everything else uses `push_span`.
    fn push_text(&mut self, text: String) {
        // U-019: strip bidi-control / zero-width code points from ALL ingress
        // text (paragraphs, headings, list items, table cells, code) so a
        // hostile .md can't visually reorder or hide any rendered span - not
        // just the `[image:]` placeholder.
        let text = strip_bidi_zero_width(text);
        if text.is_empty() {
            return;
        }
        if let Some(frame) = self.stack.last_mut() {
            match frame {
                Frame::Code { text: buf, .. } => {
                    buf.push_str(&text);
                    return;
                }
                Frame::Table { current_cell, .. } => {
                    push_or_extend(current_cell, text, self.style, self.link_url.as_deref());
                    return;
                }
                _ => {}
            }
        }
        let span = Span {
            text,
            style: self.style,
            link_url: self.link_url.clone(),
        };
        self.push_span(span);
    }

    fn push_span(&mut self, span: Span) {
        let Some(frame) = self.stack.last_mut() else {
            // Stray inline content at root - wrap in a one-off paragraph.
            self.output.push(MdNode::Paragraph { spans: vec![span] });
            return;
        };
        match frame {
            Frame::Paragraph(spans) | Frame::Heading(_, spans) => {
                merge_or_push(spans, span);
            }
            Frame::Item { inline, .. } => {
                merge_or_push(inline, span);
            }
            Frame::Table { current_cell, .. } => {
                merge_or_push(current_cell, span);
            }
            // CodeBlock takes plain text via `push_text`; spans don't apply.
            Frame::Code { .. } => {}
            // Lists and BlockQuotes don't carry inline content directly -
            // their text arrives wrapped in a child Paragraph/Item frame.
            Frame::List { .. } | Frame::Quote(_) | Frame::Footnote { .. } => {}
        }
    }

    /// Install a finished node in the deepest open block, or in the root
    /// output vector if the stack is empty. Drops the node (sets the
    /// `truncated` flag) once the walker has installed `MAX_AST_NODES` nodes,
    /// to bound heap usage on adversarial inputs.
    fn install(&mut self, node: MdNode) {
        if self.node_count >= MAX_AST_NODES {
            self.truncated = true;
            return;
        }
        self.node_count += 1;
        let Some(frame) = self.stack.last_mut() else {
            self.output.push(node);
            return;
        };
        match frame {
            Frame::Quote(children) | Frame::Footnote { children, .. } => {
                children.push(node);
            }
            Frame::Item { children, inline } => {
                if !inline.is_empty() {
                    if self.node_count >= MAX_AST_NODES {
                        self.truncated = true;
                    } else {
                        self.node_count += 1;
                        let spans = std::mem::take(inline);
                        children.push(MdNode::Paragraph { spans });
                    }
                }
                children.push(node);
            }
            // Other frames don't accept block children. A stray block (e.g. a
            // nested heading mid-paragraph from malformed input) would land
            // here; route it to the root rather than panic.
            _ => self.output.push(node),
        }
    }
}

/// Max characters kept from an untrusted image `title`/`dest_url` in the
/// `[image: …]` placeholder. A real caption/URL is short; the cap is
/// defence-in-depth so a multi-kilobyte string can't bloat a one-line marker.
const MAX_PLACEHOLDER_CHARS: usize = 256;

/// Remove bidi-control / zero-width code points from `text` (U-019). Shared by
/// `push_text` and the inline-`Event::Code` arm so EVERY rendered span - body,
/// heading, list item, table cell, image placeholder, AND inline code - is
/// sanitized, not just the `[image:]` placeholder. No length cap (unlike
/// `sanitize_placeholder`); only re-allocates when such a char is actually
/// present, so the common (clean) path is allocation-free.
pub(crate) fn strip_bidi_zero_width(text: String) -> String {
    if text.chars().any(is_bidi_or_zero_width) {
        text.chars()
            .filter(|&c| !is_bidi_or_zero_width(c))
            .collect()
    } else {
        text
    }
}

/// True for Unicode bidi-control and zero-width code points. These carry no
/// legitimate meaning inside a `[image: …]` elision marker but let hostile
/// `.md` content reorder the visible label (bidi overrides) or hide/inflate it
/// (zero-width). Stripped before the placeholder is built.
fn is_bidi_or_zero_width(c: char) -> bool {
    matches!(c,
        // Zero-width: ZWSP, ZWNJ, ZWJ, word joiner, BOM/ZWNBSP.
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
        // Directional marks: LRM, RLM, ALM.
        | '\u{200E}' | '\u{200F}' | '\u{061C}'
        // Embeddings/overrides: LRE, RLE, PDF, LRO, RLO.
        | '\u{202A}'..='\u{202E}'
        // Isolates: LRI, RLI, FSI, PDI.
        | '\u{2066}'..='\u{2069}'
        // Deprecated bidi/shaping format chars (still processed by some text
        // engines): inhibit/activate symmetric swapping + Arabic form shaping,
        // national/nominal digit shapes.
        | '\u{206A}'..='\u{206F}'
        // Interlinear annotation anchor/separator/terminator - can hide/shift
        // text in renderers that honour them.
        | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// Sanitise untrusted text for the `[image: …]` placeholder: drop bidi/
/// zero-width code points, then truncate to [`MAX_PLACEHOLDER_CHARS`]
/// (char-counted, never splitting a UTF-8 sequence).
fn sanitize_placeholder(raw: &str) -> String {
    raw.chars()
        .filter(|&c| !is_bidi_or_zero_width(c))
        .take(MAX_PLACEHOLDER_CHARS)
        .collect()
}

/// Append `span` to `spans`, merging into the previous span when both styles
/// (and link state) match - keeps the AST compact for downstream rendering.
fn merge_or_push(spans: &mut Vec<Span>, span: Span) {
    if let Some(last) = spans.last_mut()
        && last.style == span.style
        && last.link_url == span.link_url
    {
        last.text.push_str(&span.text);
        return;
    }
    spans.push(span);
}

fn push_or_extend(spans: &mut Vec<Span>, text: String, style: SpanStyle, link: Option<&str>) {
    if let Some(last) = spans.last_mut()
        && last.style == style
        && last.link_url.as_deref() == link
    {
        last.text.push_str(&text);
        return;
    }
    spans.push(Span {
        text,
        style,
        link_url: link.map(str::to_owned),
    });
}

// ---------------------------------------------------------------------------
// Tests - pure-Rust, GPUI-free.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn first(nodes: &[MdNode]) -> &MdNode {
        nodes.first().expect("expected at least one node")
    }

    #[test]
    fn placeholder_sanitiser_strips_bidi_zero_width_and_truncates() {
        let hostile = format!("a\u{202E}b\u{200B}c{}", "x".repeat(400));
        let clean = sanitize_placeholder(&hostile);
        assert!(!clean.contains('\u{202E}'), "bidi override stripped");
        assert!(!clean.contains('\u{200B}'), "zero-width space stripped");
        assert!(clean.starts_with("abc"));
        assert_eq!(clean.chars().count(), MAX_PLACEHOLDER_CHARS, "capped");
    }

    #[test]
    fn image_node_sanitises_untrusted_url() {
        // A U+202E in the image URL would otherwise reorder the visible
        // `[image: …]` label to spoof a benign-looking path.
        let nodes = parse_with_limit("![](evil\u{202E}gnp.exe)").expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected a paragraph for a top-level image");
        };
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(text.starts_with("[image: "), "got {text:?}");
        assert!(
            !text.contains('\u{202E}'),
            "bidi override must be stripped: {text:?}"
        );
    }

    #[test]
    fn image_node_does_not_duplicate_alt_text() {
        let nodes = parse_with_limit("![alt text](cat.png)").expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected a paragraph for a top-level image");
        };
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "[image: cat.png]");
    }

    #[test]
    fn body_text_strips_bidi_and_zero_width() {
        // U-019: a U+202E (RLO) override or zero-width char embedded directly in
        // BODY text - not just the [image:] placeholder - must be stripped
        // before it reaches the renderer, so it can't visually reorder or hide
        // the paragraph the user is reading.
        let nodes = parse_with_limit("safe \u{202E}txet.exe\u{200B} end").expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected a paragraph");
        };
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(
            !text.contains('\u{202E}'),
            "RLO override stripped: {text:?}"
        );
        assert!(!text.contains('\u{200B}'), "ZWSP stripped: {text:?}");
        assert!(
            text.contains("txet.exe"),
            "visible text preserved: {text:?}"
        );
    }

    #[test]
    fn inline_code_strips_bidi_and_zero_width() {
        // U-019: inline code reaches the renderer via push_span (not push_text),
        // so it must be stripped too - otherwise `` `\u{202E}txet.exe` `` renders
        // visually reversed and the "every rendered span" guarantee leaks.
        let nodes = parse_with_limit("run `\u{202E}txet.exe\u{200B}` now").expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected a paragraph");
        };
        let code: String = spans
            .iter()
            .filter(|s| s.style.code)
            .map(|s| s.text.as_str())
            .collect();
        assert!(!code.is_empty(), "expected an inline code span: {spans:?}");
        assert!(
            !code.contains('\u{202E}') && !code.contains('\u{200B}'),
            "bidi/zero-width must be stripped from inline code: {code:?}"
        );
        assert!(code.contains("txet.exe"), "code text preserved: {code:?}");
    }

    #[test]
    fn parses_h1_through_h6() {
        let src = "# H1\n\n## H2\n\n### H3\n\n#### H4\n\n##### H5\n\n###### H6\n";
        let nodes = parse_with_limit(src).expect("parse");
        let levels: Vec<_> = nodes
            .iter()
            .filter_map(|n| {
                if let MdNode::Heading { level, .. } = n {
                    Some(*level)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            levels,
            vec![
                HeadingLevel::H1,
                HeadingLevel::H2,
                HeadingLevel::H3,
                HeadingLevel::H4,
                HeadingLevel::H5,
                HeadingLevel::H6
            ]
        );
        // Confirm the H1's inline text is captured.
        if let MdNode::Heading { spans, .. } = &nodes[0] {
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].text, "H1");
        } else {
            panic!("expected Heading");
        }
    }

    #[test]
    fn parses_indented_code_block() {
        let src = "    let x = 1;\n    let y = 2;\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::CodeBlock { lang, text } => {
                assert!(lang.is_none());
                assert!(text.contains("let x"));
                assert!(text.contains("let y"));
            }
            other => panic!("expected indented CodeBlock, got {:?}", other),
        }
    }

    #[test]
    fn parses_fenced_code_block_with_lang() {
        let src = "```rust\nfn main() {}\n```\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::CodeBlock { lang, text } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert!(text.contains("fn main"));
            }
            other => panic!("expected fenced CodeBlock, got {:?}", other),
        }
    }

    #[test]
    fn parses_ordered_list_with_start() {
        let src = "3. first\n4. second\n5. third\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::List {
                ordered_start,
                items,
            } => {
                assert_eq!(*ordered_start, Some(3));
                assert_eq!(items.len(), 3);
            }
            other => panic!("expected ordered List, got {:?}", other),
        }
    }

    #[test]
    fn parses_nested_unordered_list() {
        let src = "- top1\n  - nested-a\n  - nested-b\n- top2\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::List {
                ordered_start,
                items,
            } => {
                assert!(ordered_start.is_none());
                assert_eq!(items.len(), 2);
                // First item must contain a nested List node.
                let first_item = &items[0];
                let has_nested = first_item.iter().any(|n| matches!(n, MdNode::List { .. }));
                assert!(has_nested, "expected nested list in first item");
            }
            other => panic!("expected outer List, got {:?}", other),
        }
    }

    #[test]
    fn parses_link_with_url() {
        let src = "see [docs](https://example.com/x).\n";
        let nodes = parse_with_limit(src).expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected Paragraph");
        };
        let link_span = spans
            .iter()
            .find(|s| s.link_url.is_some())
            .expect("a link span");
        assert_eq!(link_span.text, "docs");
        assert_eq!(link_span.link_url.as_deref(), Some("https://example.com/x"));
    }

    #[test]
    fn parses_blockquote() {
        let src = "> quoted line one\n> quoted line two\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::BlockQuote { children } => {
                assert!(
                    children
                        .iter()
                        .any(|c| matches!(c, MdNode::Paragraph { .. })),
                    "blockquote must contain at least one paragraph"
                );
            }
            other => panic!("expected BlockQuote, got {:?}", other),
        }
    }

    #[test]
    fn parses_table_with_header_and_rows() {
        let src = "| col1 | col2 |\n|------|------|\n| a    | b    |\n| c    | d    |\n";
        let nodes = parse_with_limit(src).expect("parse");
        match first(&nodes) {
            MdNode::Table {
                header,
                rows,
                alignments,
            } => {
                assert_eq!(alignments.len(), 2);
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 2);
                let header_text: Vec<&str> = header
                    .iter()
                    .filter_map(|cell| cell.first().map(|s| s.text.as_str()))
                    .collect();
                assert_eq!(header_text, vec!["col1", "col2"]);
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn parses_horizontal_rule() {
        let src = "before\n\n---\n\nafter\n";
        let nodes = parse_with_limit(src).expect("parse");
        assert!(
            nodes.iter().any(|n| matches!(n, MdNode::Rule)),
            "expected a Rule node in {:?}",
            nodes
        );
    }

    #[test]
    fn parses_strikethrough_extension() {
        let src = "this is ~~struck~~ text\n";
        let nodes = parse_with_limit(src).expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected Paragraph");
        };
        let struck = spans
            .iter()
            .find(|s| s.style.strikethrough)
            .expect("strikethrough span");
        assert_eq!(struck.text, "struck");
    }

    #[test]
    fn parses_strong_emphasis_inline_code() {
        let src = "**bold** _em_ `code`\n";
        let nodes = parse_with_limit(src).expect("parse");
        let MdNode::Paragraph { spans } = first(&nodes) else {
            panic!("expected Paragraph");
        };
        assert!(spans.iter().any(|s| s.style.strong && s.text == "bold"));
        assert!(spans.iter().any(|s| s.style.emphasis && s.text == "em"));
        assert!(spans.iter().any(|s| s.style.code && s.text == "code"));
    }

    #[test]
    fn parses_footnote_definition() {
        let src = "see [^1]\n\n[^1]: footnote body\n";
        let nodes = parse_with_limit(src).expect("parse");
        assert!(
            nodes.iter().any(|n| matches!(n, MdNode::Footnote { .. })),
            "expected Footnote node"
        );
    }

    #[test]
    fn rejects_input_above_limit() {
        // 10 MB + 1 byte
        let big = "a".repeat(MAX_INPUT_BYTES + 1);
        let err = parse_with_limit(&big).expect_err("must reject");
        match err {
            ParseError::TooLarge { bytes, limit } => {
                assert_eq!(bytes, MAX_INPUT_BYTES + 1);
                assert_eq!(limit, MAX_INPUT_BYTES);
            }
        }
    }

    #[test]
    fn parses_100kb_under_budget() {
        // AC: 100 KB rendered (parse + emit) < 200 ms. We measure parse
        // alone; render-tree emission is negligible relative to parse
        // walking. Debug-build budget is 60 ms, release 10 ms - both are
        // generous vs the AC's 200 ms total budget.
        let mut src = String::new();
        while src.len() < 100 * 1024 {
            src.push_str(
                "## A heading\n\nSome **bold** _emphasis_ and `code` plus a [link](https://x.io).\n\n- bullet one\n- bullet two\n\n```rust\nfn x() {}\n```\n\n",
            );
        }
        let started = Instant::now();
        let nodes = parse_with_limit(&src).expect("parse");
        let elapsed = started.elapsed();
        assert!(!nodes.is_empty());
        let budget_ms: u128 = if cfg!(debug_assertions) { 60 } else { 10 };
        assert!(
            elapsed.as_millis() < budget_ms,
            "100 KB parse took {:?}, exceeds {} ms budget",
            elapsed,
            budget_ms
        );
    }

    #[test]
    fn empty_input_returns_empty_ast() {
        let nodes = parse_with_limit("").expect("parse");
        assert!(nodes.is_empty());
    }

    #[test]
    fn ast_node_cap_truncates_pathological_input() {
        // Build a markdown source that, naively parsed, would produce more
        // than `MAX_AST_NODES` nodes. A 200 K-line bullet list weighs ~600 KB
        // (well under the 10 MB byte cap) but would produce one MdNode per
        // bullet plus a paragraph child each - well over 100 K nodes.
        let mut src = String::with_capacity(200_000 * 4);
        for _ in 0..200_000 {
            src.push_str("- x\n");
        }
        let nodes = parse_with_limit(&src).expect("parse");
        // Count nodes recursively to verify the cap was respected.
        fn count(nodes: &[MdNode]) -> usize {
            let mut total = 0;
            for n in nodes {
                total += 1;
                match n {
                    MdNode::BlockQuote { children } | MdNode::Footnote { children, .. } => {
                        total += count(children);
                    }
                    MdNode::List { items, .. } => {
                        for item in items {
                            total += 1;
                            total += count(item);
                        }
                    }
                    _ => {}
                }
            }
            total
        }
        let total = count(&nodes);
        assert!(
            total <= MAX_AST_NODES + 2,
            "expected ≤ {} nodes, got {}",
            MAX_AST_NODES + 2,
            total
        );
        // The walker appends a "[markdown viewer: document truncated …]"
        // notice paragraph as the last node when it hits the cap.
        let last_text = match nodes.last() {
            Some(MdNode::Paragraph { spans }) => {
                spans.iter().map(|s| s.text.as_str()).collect::<String>()
            }
            _ => String::new(),
        };
        assert!(
            last_text.contains("truncated"),
            "expected truncation notice as last node, got: {:?}",
            nodes.last()
        );
    }
}
