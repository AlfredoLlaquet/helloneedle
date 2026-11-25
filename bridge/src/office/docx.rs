// SPDX-License-Identifier: GPL-3.0-or-later
//! Visit the Word main story in order, keeping only the open elements and current paragraph.

use super::xml_encoding::Input as XmlInput;
use super::{
    ooxml::{main_part, named_heading, word_headings},
    package::Package,
    paragraph_location, RichText, Style,
};
use crate::{Block, Document, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, ensure, Result};
use quick_xml::{
    events::{BytesStart, Event},
    name::ResolveResult,
    NsReader,
};
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufRead, Read},
    ops::Range,
    path::Path,
};

type Cell = (usize, usize, usize);

// The limits apply to a single token, active ancestors, and live paragraph/preview data.
const XML_WINDOW: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 512;
const PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const NEIGHBORS: usize = 12;

pub(crate) fn visit_docx(path: &Path, visit: impl FnMut(Block) -> Result<bool>) -> Result<()> {
    visit_file(path, 0..0, visit)
}

fn visit_file(
    path: &Path,
    rich_blocks: Range<usize>,
    visit: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let file = File::open(path).map_err(|e| anyhow!("unreadable_document: {e}"))?;
    let mut package = Package::open(file)?;
    let main = main_part(&mut package, "word/document.xml")?;
    let headings = word_headings(&mut package, &main)?;
    let body = package.stream(&main)?;
    Word::new(headings, rich_blocks).read(body, visit)
}

// Compatibility for callers of office::extract that explicitly want a materialized document.
pub(super) fn collect(path: &Path) -> Result<Document> {
    let mut blocks = Vec::new();
    let mut used = 0;
    visit_file(path, 0..usize::MAX, |block| {
        used += block_bytes(&block) + std::mem::size_of::<Block>();
        ensure!(used <= MAX_TEXT_BYTES, "resource_limit: extracted document");
        blocks.push(block);
        Ok(true)
    })?;
    Ok(Document {
        kind: "docx".into(),
        blocks,
    })
}

fn block_bytes(block: &Block) -> usize {
    block.text.len() + block.html.as_ref().map_or(0, String::len) + block.label.len()
}

/// Per-block IDs refer to nonempty paragraphs. Keep the actual paragraph numbers, including
/// omitted empty ones, in the location. Return an exact count only after reaching EOF.
pub(crate) fn preview_docx(
    path: &Path,
    selected: usize,
    focus: usize,
    length: usize,
) -> Result<(Vec<Block>, usize, Option<usize>)> {
    let range = selected.saturating_sub(NEIGHBORS)..selected.saturating_add(NEIGHBORS + 1);
    let mut context = VecDeque::new();
    let mut used = 0;
    let mut total = 0;
    let mut selected_in_context = 0;
    let mut stopped = false;
    visit_file(path, range.clone(), |mut block| {
        let index = total;
        total += 1;
        if !range.contains(&index) {
            return Ok(true);
        }
        if index == selected && block_bytes(&block) > PREVIEW_BYTES {
            crate::protocol::clip_paragraph(&mut block, focus, length);
        }
        let size = block_bytes(&block);
        if index <= selected {
            while !context.is_empty() && used + size > PREVIEW_BYTES {
                used -= block_bytes(&context.pop_front().unwrap());
            }
            if index == selected {
                selected_in_context = context.len();
            }
        } else if used + size > PREVIEW_BYTES {
            stopped = true;
            return Ok(false);
        }
        if size <= PREVIEW_BYTES {
            used += size;
            context.push_back(block);
        }
        if index >= selected && index - selected == NEIGHBORS {
            stopped = true;
        }
        Ok(!stopped)
    })?;
    ensure!(
        selected < total || (selected == 0 && total == 0),
        "location: Block index is outside the document"
    );
    Ok((
        context.into(),
        selected_in_context,
        (!stopped).then_some(total),
    ))
}

enum ParagraphText {
    Plain(String),
    Rich(RichText),
}

impl ParagraphText {
    fn append(&mut self, value: &str, style: Style) -> Result<()> {
        if let Self::Rich(rich) = self {
            // Large paragraphs still preview as text. Reserve for XML escaping and style
            // changes before appending so formatting alone cannot make a search hit unreadable.
            let remaining = PREVIEW_BYTES.saturating_sub(rich.html.len() + rich.text.len() + 64);
            if value.len() > remaining / 6 {
                *self = Self::Plain(std::mem::take(&mut rich.text));
            }
        }
        match self {
            Self::Plain(text) => {
                ensure!(
                    value.len() <= MAX_TEXT_BYTES - text.len(),
                    "resource_limit: paragraph size"
                );
                text.push_str(value);
                Ok(())
            }
            Self::Rich(text) => text.append(value, style),
        }
    }
}

struct Paragraph {
    content: ParagraphText,
    outline: Option<usize>,
    style_heading: Option<usize>,
}

#[derive(Clone, Copy)]
enum Kind {
    Root,
    Document,
    Body,
    Flow,
    Table {
        id: usize,
        rows: usize,
    },
    Row {
        table: usize,
        row: usize,
        next_column: usize,
    },
    RowProps,
    Cell,
    CellProps,
    Paragraph,
    ParagraphProps,
    Inline,
    Run,
    RunProps,
    Text,
    Ignore,
}

#[derive(Clone, Copy)]
struct Frame {
    kind: Kind,
    cell: Option<Cell>,
    style: Style,
    tag_bytes: usize,
}

struct Word {
    headings: HashMap<String, usize>,
    rich_blocks: Range<usize>,
    frames: Vec<Frame>,
    open_bytes: usize,
    body_seen: bool,
    root_seen: bool,
    tables: usize,
    paragraphs: usize,
    blocks: usize,
    paragraph: Option<Paragraph>,
}

impl Word {
    fn new(headings: HashMap<String, usize>, rich_blocks: Range<usize>) -> Self {
        Self {
            headings,
            rich_blocks,
            frames: vec![Frame {
                kind: Kind::Root,
                cell: None,
                style: Style::default(),
                tag_bytes: 0,
            }],
            open_bytes: 0,
            body_seen: false,
            root_seen: false,
            tables: 0,
            paragraphs: 0,
            blocks: 0,
            paragraph: None,
        }
    }

    fn read(
        mut self,
        input: impl Read,
        mut visit: impl FnMut(Block) -> Result<bool>,
    ) -> Result<()> {
        // Re-arm Take at each event. An unterminated tag or a very large text token cannot
        // grow quick-xml's event buffer without bound, even in unused formatting.
        let mut xml = NsReader::from_reader(XmlInput::new(input)?.take(XML_WINDOW as u64));
        xml.config_mut().expand_empty_elements = true;
        let mut buffer = Vec::new();
        loop {
            xml.get_mut().set_limit(XML_WINDOW as u64);
            buffer.clear();
            let event = xml.read_event_into(&mut buffer);
            ensure!(
                xml.get_ref().limit() != 0,
                "resource_limit: Word XML token size"
            );
            match event.map_err(xml_error)? {
                Event::Start(tag) => self.start(&xml, &tag)?,
                Event::End(_) => {
                    let frame = self
                        .frames
                        .pop()
                        .ok_or_else(|| anyhow!("corrupt_document: unexpected Word XML end"))?;
                    self.open_bytes -= frame.tag_bytes;
                    if matches!(frame.kind, Kind::Paragraph) {
                        if let Some(block) = self.finish_paragraph(frame.cell) {
                            if !visit(block)? {
                                return Ok(());
                            }
                        }
                    }
                }
                Event::Text(value) => self.text(&value.xml10_content().map_err(xml_error)?)?,
                Event::CData(value) => self.text(&value.xml10_content().map_err(xml_error)?)?,
                Event::GeneralRef(reference) => {
                    if let Some(ch) = reference.resolve_char_ref().map_err(xml_error)? {
                        self.text(ch.encode_utf8(&mut [0; 4]))?;
                    } else {
                        let entity = reference.decode().map_err(xml_error)?;
                        let value = quick_xml::escape::resolve_xml_entity(&entity)
                            .ok_or_else(|| anyhow!("corrupt_document: unknown XML entity"))?;
                        self.text(value)?;
                    }
                }
                Event::DocType(_) => bail!("corrupt_document: DTD in Word XML"),
                Event::Eof => break,
                _ => {}
            }
        }
        ensure!(
            self.frames.len() == 1 && self.body_seen,
            "corrupt_document: missing or incomplete Word body"
        );
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<()> {
        let frame = self.frames.last().unwrap();
        if matches!(frame.kind, Kind::Text) {
            self.paragraph
                .as_mut()
                .unwrap()
                .content
                .append(text, frame.style)?;
        } else if matches!(frame.kind, Kind::Root) && !text.trim().is_empty() {
            bail!("corrupt_document: text outside Word document");
        }
        Ok(())
    }

    fn finish_paragraph(&mut self, cell: Option<Cell>) -> Option<Block> {
        let p = self.paragraph.take().unwrap();
        let heading = p.outline.or(p.style_heading);
        let (text, html) = match p.content {
            ParagraphText::Plain(text) => (text, None),
            ParagraphText::Rich(rich) => {
                let (text, html) = rich.finish(heading);
                (text, Some(html))
            }
        };
        if text.trim().is_empty() {
            return None;
        }
        let (mut location, label) = paragraph_location(self.paragraphs, heading, cell);
        location["index"] = json!(self.blocks);
        self.blocks += 1;
        Some(Block {
            text,
            location,
            label,
            html,
        })
    }

    fn start<B: BufRead>(&mut self, xml: &NsReader<B>, tag: &BytesStart<'_>) -> Result<()> {
        ensure!(
            self.frames.len() <= MAX_DEPTH,
            "resource_limit: Word XML nesting"
        );
        self.open_bytes += tag.len();
        ensure!(
            self.open_bytes <= XML_WINDOW,
            "resource_limit: active Word XML markup"
        );
        let (ns, local) = xml.resolve_element(tag.name());
        let is_word = word_namespace(ns)?;
        let name = if is_word { local.as_ref() } else { b"" };
        let parent = *self.frames.last().unwrap();
        let mut cell = parent.cell;
        let style = parent.style;
        let kind = match (parent.kind, name) {
            (Kind::Root, b"document") if !self.root_seen => {
                self.root_seen = true;
                Kind::Document
            }
            (Kind::Root, _) => bail!("corrupt_document: expected a Word document"),
            (Kind::Document, b"body") if !self.body_seen => {
                self.body_seen = true;
                Kind::Body
            }
            (
                Kind::Body | Kind::Cell | Kind::Flow,
                b"sdt" | b"sdtContent" | b"customXml" | b"ins" | b"moveTo",
            ) => Kind::Flow,
            (Kind::Body | Kind::Cell | Kind::Flow, b"tbl") => {
                self.tables += 1;
                Kind::Table {
                    id: self.tables,
                    rows: 0,
                }
            }
            (Kind::Table { id, rows }, b"tr") => {
                self.frames.last_mut().unwrap().kind = Kind::Table { id, rows: rows + 1 };
                Kind::Row {
                    table: id,
                    row: rows + 1,
                    next_column: 1,
                }
            }
            (Kind::Row { .. }, b"trPr") => Kind::RowProps,
            (Kind::RowProps, b"gridBefore") => {
                if let Some(skip) = val(xml, tag)? {
                    self.set_next_column(grid_column(1, &skip)?);
                }
                Kind::Ignore
            }
            (
                Kind::Row {
                    table,
                    row,
                    next_column,
                },
                b"tc",
            ) => {
                cell = Some((table, row, next_column));
                self.set_next_column(
                    next_column
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("corrupt_document: Word table column overflow"))?,
                );
                Kind::Cell
            }
            (Kind::Cell, b"tcPr") => Kind::CellProps,
            (Kind::CellProps, b"gridSpan") => {
                if let Some(span) = val(xml, tag)? {
                    let column = cell.unwrap().2;
                    let next = grid_column(column, &span)?;
                    ensure!(next > column, "corrupt_document: zero Word table cell span");
                    self.set_next_column(next);
                }
                Kind::Ignore
            }
            (Kind::Body | Kind::Cell | Kind::Flow, b"p") => {
                self.paragraphs += 1;
                let content = if self.rich_blocks.contains(&self.blocks) {
                    ParagraphText::Rich(RichText::default())
                } else {
                    ParagraphText::Plain(String::new())
                };
                self.paragraph = Some(Paragraph {
                    content,
                    outline: None,
                    style_heading: None,
                });
                Kind::Paragraph
            }
            (Kind::Paragraph, b"pPr") => Kind::ParagraphProps,
            (Kind::ParagraphProps, b"outlineLvl") => {
                self.paragraph.as_mut().unwrap().outline = val(xml, tag)?
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| *n < 9)
                    .map(|n| n + 1);
                Kind::Ignore
            }
            (Kind::ParagraphProps, b"pStyle") => {
                self.paragraph.as_mut().unwrap().style_heading = val(xml, tag)?
                    .and_then(|s| self.headings.get(&s).copied().or_else(|| named_heading(&s)));
                Kind::Ignore
            }
            (Kind::Paragraph | Kind::Inline | Kind::Run, b"r") => Kind::Run,
            (
                Kind::Paragraph | Kind::Inline | Kind::Run,
                b"hyperlink" | b"sdt" | b"sdtContent" | b"smartTag" | b"ins" | b"moveTo"
                | b"fldSimple" | b"customXml",
            ) => Kind::Inline,
            (Kind::Run, b"rPr") => Kind::RunProps,
            (Kind::RunProps, b"b" | b"i" | b"u" | b"strike") => {
                let on = !matches!(
                    val(xml, tag)?.as_deref(),
                    Some("0" | "off" | "false" | "none")
                );
                let run = self.frames.len() - 2;
                let style = &mut self.frames[run].style;
                match name {
                    b"b" => style.bold = on,
                    b"i" => style.italic = on,
                    b"u" => style.underline = on,
                    _ => style.strike = on,
                }
                Kind::Ignore
            }
            (Kind::Paragraph | Kind::Inline | Kind::Run, b"t") => Kind::Text,
            (Kind::Paragraph | Kind::Inline | Kind::Run, name) => {
                let value = match name {
                    b"tab" | b"ptab" => "\t",
                    b"br" | b"cr" => "\n",
                    b"noBreakHyphen" => "\u{2011}",
                    b"softHyphen" => "\u{ad}",
                    _ => "",
                };
                self.paragraph
                    .as_mut()
                    .unwrap()
                    .content
                    .append(value, style)?;
                Kind::Ignore
            }
            _ => Kind::Ignore,
        };
        self.frames.push(Frame {
            kind,
            cell,
            style,
            tag_bytes: tag.len(),
        });
        Ok(())
    }

    fn set_next_column(&mut self, next: usize) {
        for frame in self.frames.iter_mut().rev() {
            if let Kind::Row { next_column, .. } = &mut frame.kind {
                *next_column = next;
                break;
            }
        }
    }
}

fn word_namespace(ns: ResolveResult<'_>) -> Result<bool> {
    match ns {
        ResolveResult::Bound(ns) => Ok(matches!(
            ns.as_ref(),
            b"http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                | b"http://purl.oclc.org/ooxml/wordprocessingml/main"
        )),
        ResolveResult::Unbound => Ok(false),
        ResolveResult::Unknown(_) => bail!("corrupt_document: undeclared XML prefix"),
    }
}

fn val<B: BufRead>(xml: &NsReader<B>, tag: &BytesStart<'_>) -> Result<Option<String>> {
    for attribute in tag.attributes() {
        let attr =
            attribute.map_err(|e| anyhow!("corrupt_document: invalid Word attribute: {e}"))?;
        let (ns, local) = xml.resolve_attribute(attr.key);
        if local.as_ref() == b"val" && word_namespace(ns)? {
            return Ok(Some(
                attr.decode_and_unescape_value(xml.decoder())
                    .map_err(xml_error)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn xml_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("corrupt_document: invalid Word XML: {error}")
}

fn grid_column(base: usize, delta: &str) -> Result<usize> {
    let delta: usize = delta
        .trim()
        .parse()
        .map_err(|_| anyhow!("corrupt_document: invalid Word table grid coordinate"))?;
    base.checked_add(delta)
        .ok_or_else(|| anyhow!("corrupt_document: Word table column overflow"))
}
