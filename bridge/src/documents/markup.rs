// SPDX-License-Identifier: GPL-3.0-or-later
//! CommonMark and HTML share one searchable projection and one preview sanitizer.

use super::html::Budget;
use crate::Block;
use anyhow::Result;
use ego_tree::iter::Edge;
use scraper::{node::Node, ElementRef, Html, Selector};
use serde_json::json;

pub(super) fn declared_encoding(bytes: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    // HTML places encoding declarations within its first 1024 bytes. The declaration itself is ASCII.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    let document = Html::parse_document(&head);
    let meta = Selector::parse("meta").ok()?;
    for tag in document.select(&meta) {
        let value = tag.value();
        let label = value.attr("charset").or_else(|| {
            if !value
                .attr("http-equiv")?
                .eq_ignore_ascii_case("content-type")
            {
                return None;
            }
            value.attr("content")?.split(';').find_map(|part| {
                let (name, label) = part.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("charset")
                    .then(|| label.trim().trim_matches(['\'', '"']))
            })
        });
        if let Some(encoding) = label.and_then(|s| encoding_rs::Encoding::for_label(s.as_bytes())) {
            return Some(encoding);
        }
    }
    None
}

#[cfg(test)]
pub(super) fn project(source: &str, markdown: bool) -> Result<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut collect = |block| {
        blocks.push(block);
        Ok(true)
    };
    if markdown {
        super::markdown::visit_source(source, &mut collect)?;
    } else {
        // Independent reference for the filtering HTML tokenizer tests.
        let doc = Html::parse_document(source);
        project_tree(&doc, None, false, &mut collect)?;
    }
    Ok(blocks)
}

pub(super) fn project_tree(
    document: &Html,
    budget: Option<&Budget>,
    stream_blocks: bool,
    visitor: &mut dyn FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let mut projection = Projection {
        budget,
        stream_blocks,
        visitor: Some(visitor),
        ..Projection::default()
    };
    projection.walk(document.root_element())?;
    projection.finish()
}

#[derive(Clone, Copy, Default)]
enum Role {
    #[default]
    Paragraph,
    Heading(u8),
    Quote,
    Item,
    Pre,
    Cell,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::Heading(_) => "heading",
            Self::Quote => "quote",
            Self::Item => "listItem",
            Self::Pre => "code",
            Self::Cell => "cell",
        }
    }
}

#[derive(Default)]
struct Projection<'a> {
    budget: Option<&'a Budget>,
    stream_blocks: bool,
    output_reservation: usize,
    visitor: Option<&'a mut dyn FnMut(Block) -> Result<bool>>,
    index: usize,
    stopped: bool,
    role: Role,
    text: String,
    fragment: String,
    // Only these fixed tags are ever copied to the preview. No input attributes survive.
    styles: Vec<&'static str>,
    emitted_styles: Vec<&'static str>,
    pending_space: bool,
    // None denotes an unordered list; Some stores the next ordinal for an ordered list.
    lists: Vec<Option<i64>>,
    marker: Option<String>,
}

struct Frame {
    previous_role: Role,
    previous_marker: Option<String>,
    styled: bool,
    boundary: bool,
}

impl Projection<'_> {
    fn reserve(&mut self, bytes: usize) -> Result<()> {
        if let Some(budget) = self.budget {
            budget.reserve(bytes)?;
            if self.stream_blocks {
                self.output_reservation += bytes;
            }
        }
        Ok(())
    }

    fn walk(&mut self, root: ElementRef<'_>) -> Result<()> {
        // The input may have more levels than the native call stack can hold.
        let mut frames = Vec::new();
        let mut ignored_depth = 0usize;
        for edge in root.traverse() {
            if self.stopped {
                break;
            }
            match edge {
                Edge::Open(node) if ignored_depth == 0 => {
                    if let Some(element) = ElementRef::wrap(node) {
                        if let Some(frame) = self.enter(element)? {
                            frames.push(frame);
                        } else {
                            ignored_depth = 1;
                        }
                    } else if let Node::Text(text) = node.value() {
                        self.append(text)?;
                    } else {
                        ignored_depth = 1;
                    }
                }
                Edge::Open(_) => ignored_depth += 1,
                Edge::Close(_) if ignored_depth != 0 => ignored_depth -= 1,
                Edge::Close(node) => {
                    if let Some(element) = ElementRef::wrap(node) {
                        let frame = frames.pop().expect("paired HTML traversal edges");
                        self.leave(element.value().name(), frame)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn enter(&mut self, element: ElementRef<'_>) -> Result<Option<Frame>> {
        let value = element.value();
        let name = value.name();
        if is_hidden(&element)
            || matches!(
                name,
                "head"
                    | "script"
                    | "style"
                    | "template"
                    | "noscript"
                    | "iframe"
                    | "frame"
                    | "object"
                    | "embed"
                    | "canvas"
                    | "svg"
                    | "math"
                    | "audio"
                    | "video"
                    | "input"
                    | "select"
                    | "textarea"
                    | "button"
            )
        {
            return Ok(None);
        }
        match name {
            "br" => {
                self.pending_space = false;
                self.reserve(12)?;
                self.sync_styles()?;
                self.text.push('\n');
                self.fragment.push_str("<br/>");
                return Ok(None);
            }
            "hr" => {
                self.finish()?;
                return Ok(None);
            }
            // Only the human-readable alternate text belongs in this offline preview.
            "img" => {
                if let Some(alt) = value.attr("alt") {
                    self.append(alt)?;
                }
                return Ok(None);
            }
            _ => {}
        }

        let role = match name {
            "h1" => Some(Role::Heading(1)),
            "h2" => Some(Role::Heading(2)),
            "h3" => Some(Role::Heading(3)),
            "h4" => Some(Role::Heading(4)),
            "h5" => Some(Role::Heading(5)),
            "h6" => Some(Role::Heading(6)),
            "blockquote" => Some(Role::Quote),
            "pre" => Some(Role::Pre),
            "li" => Some(Role::Item),
            "td" | "th" => Some(Role::Cell),
            _ => None,
        };
        let boundary = role.is_some()
            || matches!(
                name,
                "p" | "div"
                    | "section"
                    | "article"
                    | "aside"
                    | "header"
                    | "footer"
                    | "main"
                    | "nav"
                    | "address"
                    | "figure"
                    | "figcaption"
                    | "table"
                    | "tr"
                    | "caption"
                    | "ol"
                    | "ul"
                    | "dl"
                    | "dt"
                    | "dd"
                    | "summary"
                    | "details"
            );
        if boundary {
            self.finish()?;
        }
        let old_role = self.role;
        if let Some(role) = role {
            self.role = role;
        }
        let mut old_marker = None;
        if name == "ol" {
            self.lists.push(Some(
                value
                    .attr("start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1),
            ));
        } else if name == "ul" {
            self.lists.push(None);
        } else if name == "li" {
            let marker = if let Some(Some(next)) = self.lists.last_mut() {
                let ordinal = value
                    .attr("value")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(*next);
                *next = ordinal.saturating_add(1);
                format!("{ordinal}. ")
            } else {
                "• ".into()
            };
            old_marker = self.marker.replace(marker);
        }
        let style = match name {
            "b" | "strong" | "th" => Some("strong"),
            "em" | "i" => Some("em"),
            "a" | "u" => Some("u"),
            "s" | "del" | "strike" => Some("s"),
            "sup" => Some("sup"),
            "sub" => Some("sub"),
            "code" | "kbd" | "samp" | "tt" => Some("code"),
            _ => None,
        };
        if let Some(style) = style {
            self.styles.push(style);
        }
        Ok(Some(Frame {
            previous_role: old_role,
            previous_marker: old_marker,
            styled: style.is_some(),
            boundary,
        }))
    }

    fn leave(&mut self, name: &str, frame: Frame) -> Result<()> {
        if frame.styled {
            self.styles.pop();
        }
        if frame.boundary {
            self.finish()?;
        }
        if name == "li" {
            self.marker = frame.previous_marker;
        }
        if matches!(name, "ul" | "ol") {
            self.lists.pop();
        }
        self.role = frame.previous_role;
        Ok(())
    }

    fn append(&mut self, input: &str) -> Result<()> {
        if input.is_empty() {
            return Ok(());
        }
        if self.budget.is_some() {
            // Two output strings and their growth capacity; account for escaping before allocating.
            let escapes: usize = input
                .bytes()
                .map(|b| match b {
                    b'&' => 4,
                    b'<' | b'>' => 3,
                    _ => 0,
                })
                .sum();
            self.reserve(
                input
                    .len()
                    .saturating_mul(4)
                    .saturating_add(escapes.saturating_mul(2)),
            )?;
        }
        let normalized;
        let text = if matches!(self.role, Role::Pre) {
            input
        } else {
            let mut output = String::with_capacity(input.len());
            for ch in input.chars() {
                if matches!(ch, ' ' | '\t' | '\r' | '\n' | '\u{c}' | '\u{a0}') {
                    self.pending_space = true;
                } else {
                    if self.pending_space {
                        let previous = if output.is_empty() {
                            &self.text
                        } else {
                            &output
                        };
                        if !previous.is_empty() && !previous.ends_with('\n') {
                            output.push(' ');
                        }
                    }
                    self.pending_space = false;
                    output.push(ch);
                }
            }
            normalized = output;
            &normalized
        };
        if text.is_empty() {
            return Ok(());
        }
        self.sync_styles()?;
        self.text.push_str(text);
        escape_into(&mut self.fragment, text);
        Ok(())
    }

    fn sync_styles(&mut self) -> Result<()> {
        let common = self
            .emitted_styles
            .iter()
            .zip(&self.styles)
            .take_while(|(a, b)| a == b)
            .count();
        self.reserve(
            (self.emitted_styles.len() - common + self.styles.len() - common).saturating_mul(20),
        )?;
        for tag in self.emitted_styles.drain(common..).rev() {
            self.fragment.push_str("</");
            self.fragment.push_str(tag);
            self.fragment.push('>');
        }
        for &tag in &self.styles[common..] {
            self.fragment.push('<');
            self.fragment.push_str(tag);
            self.fragment.push('>');
            self.emitted_styles.push(tag);
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.pending_space = false;
        if self.text.is_empty() || self.stopped {
            return Ok(());
        }
        self.reserve(1024 + 20 * self.emitted_styles.len())?;
        for tag in self.emitted_styles.drain(..).rev() {
            self.fragment.push_str("</");
            self.fragment.push_str(tag);
            self.fragment.push('>');
        }
        let index = self.index;
        self.index += 1;
        let mut location = json!({"index":index, "role":self.role.name()});
        let mut text = std::mem::take(&mut self.text);
        let mut fragment = std::mem::take(&mut self.fragment);
        let html = match self.role {
            Role::Heading(level) => {
                location["level"] = json!(level);
                format!("<h{level}>{fragment}</h{level}>")
            }
            Role::Item => {
                if let Some(marker) = self.marker.take() {
                    text.insert_str(0, &marker);
                    fragment.insert_str(0, &marker);
                }
                format!("<p style=\"margin-left: 18px\">{fragment}</p>")
            }
            Role::Quote => format!("<blockquote><p>{fragment}</p></blockquote>"),
            Role::Pre => format!("<pre>{fragment}</pre>"),
            _ => format!("<p>{fragment}</p>"),
        };
        let block = Block::new(text, location, format!("Block {}", index + 1)).with_html(html);
        self.stopped = !self.visitor.as_mut().expect("projection has a sink")(block)?;
        // The HTML tree stays live until its projection finishes. A streaming
        // Markdown search can release the separate output allocation after each block.
        if self.stream_blocks {
            if let Some(budget) = self.budget {
                budget.release(std::mem::take(&mut self.output_reservation));
            }
        }
        Ok(())
    }
}

fn is_hidden(element: &ElementRef<'_>) -> bool {
    let value = element.value();
    if value.attr("hidden").is_some()
        || value
            .attr("aria-hidden")
            .is_some_and(|s| s.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    value
        .attr("style")
        .is_some_and(|style| style.split(';').any(hides))
}

pub(super) fn hides(declaration: &str) -> bool {
    let Some((property, value)) = declaration.split_once(':') else {
        return false;
    };
    let value = value.split('!').next().unwrap_or("").trim();
    property.trim().eq_ignore_ascii_case("display") && value.eq_ignore_ascii_case("none")
        || property.trim().eq_ignore_ascii_case("visibility")
            && value.eq_ignore_ascii_case("hidden")
}

fn escape_into(output: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_nested_text_and_discards_active_resources() {
        let html = r#"<html><head><title>private title</title></head><body><h2>The <em>Latest</em>&nbsp;Batch</h2>
            <p>Welcome <a href="file:///secret">reader</a>, <img src="https://invalid/" alt="a cat">.
            <span hidden>bad</span><b style="display: none !important">bad</b></p>
            <script>alert(1)</script><iframe src="bad">bad</iframe><p>&lt;script&gt;</p></body></html>"#;
        let blocks = project(html, false).unwrap();
        let text: Vec<_> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            text,
            ["The Latest Batch", "Welcome reader, a cat.", "<script>"]
        );
        assert_eq!(blocks[0].location["level"], 2);
        let html: String = blocks.iter().filter_map(|b| b.html.as_deref()).collect();
        assert!(html.contains("<em>"));
        assert!(!html.contains("href") && !html.contains("<script") && !html.contains("<img"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn preserves_markdown_blocks_and_preformatted_code() {
        let blocks = project("# The **Latest** Batch\n\n> quoted *word*\n\n3. [x](https://invalid/)\n4. y\n\n```\n  a < b\n   c\n```\n", true).unwrap();
        let text: Vec<_> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            text,
            [
                "The Latest Batch",
                "quoted word",
                "3. x",
                "4. y",
                "  a < b\n   c\n"
            ]
        );
        assert_eq!(blocks[1].location["role"], "quote");
        assert!(blocks[4].html.as_ref().unwrap().contains("&lt;"));
    }

    #[test]
    fn breaks_stay_inside_their_block() {
        let blocks = project("<p>one<br>   two</p><p>three</p>", false).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "one\ntwo");
    }
}
