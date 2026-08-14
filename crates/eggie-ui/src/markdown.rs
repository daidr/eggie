//! A small Markdown renderer for GPUI.
//!
//! Zed's `markdown` crate is a full-featured (near 10k LOC) implementation that
//! depends on a dozen of Zed's internal crates, so it can't be reused here.
//! Instead this module parses a common subset of Markdown with `pulldown-cmark`
//! into a small block model, then renders it with GPUI's native rich-text
//! primitives (`StyledText` / `HighlightStyle` / `InteractiveText`).
//!
//! Supported: headings, paragraphs, bold / italic / strikethrough, inline code,
//! fenced code blocks, ordered / unordered lists (nested), links, block quotes,
//! and thematic breaks. Not supported: tables, images, footnotes, task lists.

use gpui::{
    AnyElement, App, FontStyle, FontWeight, HighlightStyle, InteractiveText, StyledText,
    StrikethroughStyle, TextStyle, UnderlineStyle, div, prelude::*, px, rgb,
};
use std::ops::Range;

use crate::settings::UiColors;

/// Monospace family used for inline code and code blocks.
const CODE_FONT: &str = "Menlo";

// ---------------------------------------------------------------------------
// Parse layer (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Inline styling flags carried by a run of text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    /// Destination URL if this span is a link.
    pub link: Option<String>,
}

/// A run of text with uniform inline styling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MdSpan {
    pub text: String,
    pub style: SpanStyle,
}

/// A block-level Markdown element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MdBlock {
    Heading { level: u8, spans: Vec<MdSpan> },
    Paragraph(Vec<MdSpan>),
    CodeBlock { text: String },
    List { ordered: bool, items: Vec<Vec<MdBlock>> },
    Quote(Vec<MdBlock>),
    Rule,
}

/// Parse Markdown `source` into a flat list of block elements.
pub(crate) fn parse_markdown(source: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(source, options);

    // Container stack: blocks nest (lists contain items which contain blocks,
    // quotes contain blocks). Each frame collects the blocks emitted while it
    // is open; closing a frame folds its blocks into its parent.
    enum Frame {
        Root(Vec<MdBlock>),
        Paragraph(Vec<MdSpan>),
        Heading(u8, Vec<MdSpan>),
        CodeBlock(String),
        Quote(Vec<MdBlock>),
        List { ordered: bool, items: Vec<Vec<MdBlock>> },
        Item(Vec<MdBlock>),
    }

    let mut stack: Vec<Frame> = vec![Frame::Root(Vec::new())];
    // Inline styling state, applied to Text events.
    let mut style = SpanStyle::default();
    // Link destination currently open (Markdown links don't nest in practice).
    let mut link_stack: Vec<String> = Vec::new();

    // Push a finished block into the nearest block container.
    fn push_block(stack: &mut Vec<Frame>, block: MdBlock) {
        for frame in stack.iter_mut().rev() {
            match frame {
                Frame::Root(blocks)
                | Frame::Quote(blocks)
                | Frame::Item(blocks) => {
                    blocks.push(block);
                    return;
                }
                _ => {}
            }
        }
    }

    // Push text into the nearest inline container. Paragraph / Heading frames
    // collect spans directly. In "tight" lists pulldown-cmark emits list-item
    // text as bare Text events (no Paragraph), so when the nearest block
    // container is a list Item we fold the text into an implicit trailing
    // Paragraph block inside that item.
    fn push_span(stack: &mut Vec<Frame>, text: &str, style: &SpanStyle) {
        if text.is_empty() {
            return;
        }
        for frame in stack.iter_mut().rev() {
            let spans = match frame {
                Frame::Paragraph(spans) | Frame::Heading(_, spans) => spans,
                Frame::Item(blocks) => {
                    // Reuse a trailing implicit paragraph, or start one.
                    if !matches!(blocks.last(), Some(MdBlock::Paragraph(_))) {
                        blocks.push(MdBlock::Paragraph(Vec::new()));
                    }
                    let Some(MdBlock::Paragraph(spans)) = blocks.last_mut() else {
                        return;
                    };
                    spans
                }
                Frame::Root(_) | Frame::Quote(_) => return,
                _ => continue,
            };
            // Merge into the previous span if styling matches.
            if let Some(last) = spans.last_mut()
                && &last.style == style
            {
                last.text.push_str(text);
            } else {
                spans.push(MdSpan {
                    text: text.to_string(),
                    style: style.clone(),
                });
            }
            return;
        }
    }

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => stack.push(Frame::Paragraph(Vec::new())),
                Tag::Heading { level, .. } => {
                    stack.push(Frame::Heading(level as u8, Vec::new()))
                }
                Tag::CodeBlock(_) => stack.push(Frame::CodeBlock(String::new())),
                Tag::BlockQuote(_) => stack.push(Frame::Quote(Vec::new())),
                Tag::List(start) => stack.push(Frame::List {
                    ordered: start.is_some(),
                    items: Vec::new(),
                }),
                Tag::Item => stack.push(Frame::Item(Vec::new())),
                Tag::Emphasis => style.italic = true,
                Tag::Strong => style.bold = true,
                Tag::Strikethrough => style.strikethrough = true,
                Tag::Link { dest_url, .. } => {
                    link_stack.push(dest_url.to_string());
                    style.link = Some(dest_url.to_string());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if let Some(Frame::Paragraph(spans)) = stack.pop() {
                        push_block(&mut stack, MdBlock::Paragraph(spans));
                    }
                }
                TagEnd::Heading(_) => {
                    if let Some(Frame::Heading(level, spans)) = stack.pop() {
                        push_block(&mut stack, MdBlock::Heading { level, spans });
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some(Frame::CodeBlock(mut text)) = stack.pop() {
                        // Drop the single trailing newline pulldown emits.
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        push_block(&mut stack, MdBlock::CodeBlock { text });
                    }
                }
                TagEnd::BlockQuote(_) => {
                    if let Some(Frame::Quote(blocks)) = stack.pop() {
                        push_block(&mut stack, MdBlock::Quote(blocks));
                    }
                }
                TagEnd::List(_) => {
                    if let Some(Frame::List { ordered, items }) = stack.pop() {
                        push_block(&mut stack, MdBlock::List { ordered, items });
                    }
                }
                TagEnd::Item => {
                    if let Some(Frame::Item(blocks)) = stack.pop() {
                        // Fold this item's blocks into the enclosing list.
                        if let Some(Frame::List { items, .. }) = stack.last_mut() {
                            items.push(blocks);
                        }
                    }
                }
                TagEnd::Emphasis => style.italic = false,
                TagEnd::Strong => style.bold = false,
                TagEnd::Strikethrough => style.strikethrough = false,
                TagEnd::Link => {
                    link_stack.pop();
                    style.link = link_stack.last().cloned();
                }
                _ => {}
            },
            Event::Text(text) => {
                // Inside a code block, text accumulates verbatim.
                if let Some(Frame::CodeBlock(buf)) = stack.last_mut() {
                    buf.push_str(&text);
                } else {
                    push_span(&mut stack, &text, &style);
                }
            }
            Event::Code(text) => {
                let mut code_style = style.clone();
                code_style.code = true;
                push_span(&mut stack, &text, &code_style);
            }
            Event::SoftBreak | Event::HardBreak => {
                push_span(&mut stack, " ", &style);
            }
            Event::Rule => push_block(&mut stack, MdBlock::Rule),
            _ => {}
        }
    }

    match stack.into_iter().next() {
        Some(Frame::Root(blocks)) => blocks,
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Render layer
// ---------------------------------------------------------------------------

/// Styling inputs for rendering Markdown into GPUI elements.
#[derive(Clone, Copy)]
pub(crate) struct MarkdownStyle {
    pub colors: UiColors,
    pub base_size: f32,
}

/// Render Markdown `source` into a scrollable column of GPUI elements.
///
/// Links open in the system browser via `cx.open_url`. `id_seed` disambiguates
/// the interactive-text element ids when several markdown blocks coexist.
pub(crate) fn markdown_element(source: &str, style: MarkdownStyle) -> AnyElement {
    let blocks = parse_markdown(source);
    let mut column = div().flex().flex_col().gap_2();
    for (index, block) in blocks.iter().enumerate() {
        column = column.child(render_block(block, style, index));
    }
    column.into_any_element()
}

fn render_block(block: &MdBlock, style: MarkdownStyle, id: usize) -> AnyElement {
    let colors = style.colors;
    match block {
        MdBlock::Heading { level, spans } => {
            let size = match level {
                1 => style.base_size + 7.,
                2 => style.base_size + 4.,
                3 => style.base_size + 2.,
                _ => style.base_size + 1.,
            };
            div()
                .pt_1()
                .child(inline_text(spans, style, size, FontWeight::SEMIBOLD, id))
                .into_any_element()
        }
        MdBlock::Paragraph(spans) => {
            inline_text(spans, style, style.base_size, FontWeight::NORMAL, id)
        }
        MdBlock::CodeBlock { text } => div()
            .id(("md-code-block", id))
            .overflow_x_scroll()
            .p_2()
            .rounded_md()
            .bg(rgb(colors.panel_alt))
            .border_1()
            .border_color(rgb(colors.border))
            .font_family(CODE_FONT)
            .text_size(px(style.base_size - 1.))
            .text_color(rgb(colors.text))
            .child(text.clone())
            .into_any_element(),
        MdBlock::List { ordered, items } => {
            let mut list = div().flex().flex_col().gap_1();
            for (item_index, item_blocks) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}.", item_index + 1)
                } else {
                    "•".to_string()
                };
                let mut content = div().flex().flex_col().gap_1();
                for (block_index, block) in item_blocks.iter().enumerate() {
                    content = content.child(render_block(
                        block,
                        style,
                        id * 100 + item_index * 10 + block_index,
                    ));
                }
                list = list.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_start()
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(16.))
                                .text_size(px(style.base_size))
                                .text_color(rgb(colors.muted))
                                .child(marker),
                        )
                        .child(div().flex_1().min_w_0().child(content)),
                );
            }
            list.pl_2().into_any_element()
        }
        MdBlock::Quote(blocks) => {
            let mut inner = div().flex().flex_col().gap_2();
            for (block_index, block) in blocks.iter().enumerate() {
                inner = inner.child(render_block(block, style, id * 100 + block_index));
            }
            div()
                .flex()
                .pl_3()
                .border_l_2()
                .border_color(rgb(colors.accent))
                .text_color(rgb(colors.muted))
                .child(inner)
                .into_any_element()
        }
        MdBlock::Rule => div()
            .my_1()
            .h(px(1.))
            .bg(rgb(colors.border))
            .into_any_element(),
    }
}

/// Render a run of inline spans into a single (interactive) styled text element.
fn inline_text(
    spans: &[MdSpan],
    style: MarkdownStyle,
    size: f32,
    weight: FontWeight,
    id: usize,
) -> AnyElement {
    let colors = style.colors;
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    // (byte range, url) pairs for click handling.
    let mut links: Vec<(Range<usize>, String)> = Vec::new();

    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let range = start..text.len();

        let mut highlight = HighlightStyle::default();
        if span.style.bold {
            highlight.font_weight = Some(FontWeight::BOLD);
        }
        if span.style.italic {
            highlight.font_style = Some(FontStyle::Italic);
        }
        if span.style.strikethrough {
            highlight.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(rgb(colors.muted).into()),
            });
        }
        if span.style.code {
            highlight.background_color = Some(rgb(colors.panel_alt).into());
            highlight.color = Some(rgb(colors.text).into());
        }
        if let Some(url) = &span.style.link {
            highlight.color = Some(rgb(colors.accent).into());
            highlight.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(rgb(colors.accent).into()),
                wavy: false,
            });
            links.push((range.clone(), url.clone()));
        }
        if highlight != HighlightStyle::default() {
            highlights.push((range, highlight));
        }
    }

    let text_style = TextStyle {
        color: rgb(colors.text).into(),
        font_family: ".SystemUIFont".into(),
        font_size: px(size).into(),
        font_weight: weight,
        line_height: px(size * 1.5).into(),
        ..Default::default()
    };
    let styled = StyledText::new(text).with_default_highlights(&text_style, highlights);

    if links.is_empty() {
        return styled.into_any_element();
    }

    let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, url)| url).collect();
    InteractiveText::new(("md-inline", id), styled)
        .on_click(ranges, move |index, _window, cx: &mut App| {
            if let Some(url) = urls.get(index) {
                cx.open_url(url);
            }
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> MdSpan {
        MdSpan {
            text: text.to_string(),
            style: SpanStyle::default(),
        }
    }

    #[test]
    fn parses_heading_levels() {
        let blocks = parse_markdown("# One\n\n## Two");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Heading {
                    level: 1,
                    spans: vec![plain("One")]
                },
                MdBlock::Heading {
                    level: 2,
                    spans: vec![plain("Two")]
                },
            ]
        );
    }

    #[test]
    fn parses_inline_styles() {
        let blocks = parse_markdown("normal **bold** *italic* ~~strike~~ `code`");
        let MdBlock::Paragraph(spans) = &blocks[0] else {
            panic!("expected paragraph, got {blocks:?}");
        };
        assert!(spans.iter().any(|s| s.style.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.style.italic && s.text == "italic"));
        assert!(spans.iter().any(|s| s.style.strikethrough && s.text == "strike"));
        assert!(spans.iter().any(|s| s.style.code && s.text == "code"));
    }

    #[test]
    fn parses_link_with_url() {
        let blocks = parse_markdown("see [docs](https://example.com/x)");
        let MdBlock::Paragraph(spans) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let link = spans
            .iter()
            .find(|s| s.text == "docs")
            .expect("link span");
        assert_eq!(link.style.link.as_deref(), Some("https://example.com/x"));
    }

    #[test]
    fn parses_unordered_list() {
        let blocks = parse_markdown("- a\n- b\n- c");
        let MdBlock::List { ordered, items } = &blocks[0] else {
            panic!("expected list, got {blocks:?}");
        };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0],
            vec![MdBlock::Paragraph(vec![plain("a")])]
        );
    }

    #[test]
    fn parses_ordered_list() {
        let blocks = parse_markdown("1. first\n2. second");
        let MdBlock::List { ordered, items } = &blocks[0] else {
            panic!("expected list");
        };
        assert!(ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parses_nested_list() {
        let blocks = parse_markdown("- outer\n  - inner");
        let MdBlock::List { items, .. } = &blocks[0] else {
            panic!("expected list");
        };
        // The first item holds a paragraph plus a nested list.
        assert!(items[0].iter().any(|b| matches!(b, MdBlock::List { .. })));
    }

    #[test]
    fn parses_code_block() {
        let blocks = parse_markdown("```\nlet x = 1;\nlet y = 2;\n```");
        assert_eq!(
            blocks,
            vec![MdBlock::CodeBlock {
                text: "let x = 1;\nlet y = 2;".to_string()
            }]
        );
    }

    #[test]
    fn parses_blockquote() {
        let blocks = parse_markdown("> quoted text");
        let MdBlock::Quote(inner) = &blocks[0] else {
            panic!("expected quote, got {blocks:?}");
        };
        assert_eq!(inner, &vec![MdBlock::Paragraph(vec![plain("quoted text")])]);
    }

    #[test]
    fn parses_thematic_break() {
        let blocks = parse_markdown("above\n\n---\n\nbelow");
        assert!(blocks.contains(&MdBlock::Rule));
    }

    #[test]
    fn preserves_block_order() {
        let blocks = parse_markdown("# Title\n\npara\n\n- item");
        assert!(matches!(blocks[0], MdBlock::Heading { .. }));
        assert!(matches!(blocks[1], MdBlock::Paragraph(_)));
        assert!(matches!(blocks[2], MdBlock::List { .. }));
    }

    #[test]
    fn empty_source_is_empty() {
        assert!(parse_markdown("").is_empty());
    }
}
