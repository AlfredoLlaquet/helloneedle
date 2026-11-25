// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured Office and rich-text readers. All document relationships stay inside the archive.
use crate::{Block, Document, MAX_TEXT_BYTES};
use anyhow::{bail, Result};
use roxmltree::Node;
use serde_json::{json, Value};
use std::{collections::VecDeque, fmt, fs::File, path::Path};

mod docx;
pub(crate) use docx::{preview_docx, visit_docx};
mod legacy;
mod odf;
mod ooxml;
mod package;
mod rtf;
mod stream_xml;
#[cfg(test)]
mod tests;
mod xml_encoding;

// DOM parsing is reserved for bounded metadata or one logical unit; XLSX has a fixed grid.
const MAX_XML_NODES: u32 = 350_000;
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

/// Compatibility for callers that explicitly ask to collect a complete document in memory.
pub fn extract(path: &Path, extension: &str) -> Result<Option<Document>> {
    let kind = extension.trim_start_matches('.').to_ascii_lowercase();
    if kind == "docx" {
        return docx::collect(path).map(Some);
    }
    if !matches!(
        kind.as_str(),
        "xlsx" | "pptx" | "odt" | "ods" | "odp" | "fodt" | "doc" | "rtf"
    ) {
        return Ok(None);
    }
    let mut output = Output::default();
    read(path, &kind, &mut output)?;
    Ok(Some(Document {
        kind,
        blocks: output.blocks,
    }))
}

#[cfg(test)]
pub(crate) fn visit(
    path: &Path,
    kind: &str,
    visitor: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    visit_window(path, kind, 0, visitor)
}

pub(crate) fn visit_matching(
    path: &Path,
    kind: &str,
    matcher: &grep_regex::RegexMatcher,
    visitor: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    visit_selected(path, kind, Some(matcher), 0, visitor)
}

/// Readers with compact repetitions can advance directly to the first required
/// logical block. Other readers may supply earlier blocks to the caller.
pub(crate) fn visit_window(
    path: &Path,
    kind: &str,
    first: usize,
    visitor: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    visit_selected(path, kind, None, first, visitor)
}

fn visit_selected(
    path: &Path,
    kind: &str,
    repeat_matcher: Option<&grep_regex::RegexMatcher>,
    window_start: usize,
    mut visitor: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let mut output = Output {
        visitor: Some(&mut visitor),
        repeat_matcher,
        window_start,
        ..Output::default()
    };
    match read(path, kind, &mut output) {
        Err(error) if error.is::<EndVisit>() => Ok(()),
        result => result,
    }
}

fn read(path: &Path, kind: &str, out: &mut Output) -> Result<()> {
    let file = File::open(path).map_err(|e| anyhow::anyhow!("unreadable_document: {e}"))?;
    match kind {
        "doc" => legacy::extract(path, file, out),
        "rtf" => rtf::extract(file, out),
        "fodt" => odf::extract(file, None, kind, out),
        _ => {
            let mut pkg = package::Package::open(file)?;
            if matches!(kind, "xlsx" | "pptx") {
                return ooxml::extract(&mut pkg, kind, out);
            }
            if let Some(manifest) = pkg.optional("META-INF/manifest.xml")? {
                let meta = package::parse(&manifest)?;
                if meta.descendants().any(|n| {
                    n.has_tag_name((
                        "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0",
                        "encryption-data",
                    ))
                }) {
                    bail!("encrypted_document: OpenDocument");
                }
            }
            let styles = pkg.optional("styles.xml")?;
            let content = pkg.stream("content.xml")?;
            odf::extract(content, styles.as_deref(), kind, out)
        }
    }
}

#[derive(Debug)]
struct EndVisit;

impl fmt::Display for EndVisit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("selected Office context complete")
    }
}

impl std::error::Error for EndVisit {}

#[derive(Default)]
struct Output<'visit> {
    visitor: Option<&'visit mut dyn FnMut(Block) -> Result<bool>>,
    repeat_matcher: Option<&'visit grep_regex::RegexMatcher>,
    window_start: usize,
    // Only populated by explicit, memory-bounded collection callers.
    blocks: Vec<Block>,
    collected_bytes: usize,
    index: usize,
}

impl Output<'_> {
    fn advance(&mut self, count: usize) -> Result<()> {
        self.index = self.index.checked_add(count).ok_or_else(|| {
            anyhow::anyhow!("resource_limit: logical Office block index overflow")
        })?;
        Ok(())
    }
    fn push(
        &mut self,
        text: String,
        mut location: Value,
        label: String,
        html: Option<String>,
    ) -> Result<()> {
        crate::search::checkpoint()?;
        if text.trim().is_empty() {
            return Ok(());
        }
        location["index"] = json!(self.index);
        self.advance(1)?;
        let block = Block {
            text,
            location,
            label,
            html,
        };
        if let Some(visit) = self.visitor.as_mut() {
            if !visit(block)? {
                return Err(EndVisit.into());
            }
        } else {
            self.collected_bytes += block_size(&block);
            if self.collected_bytes > MAX_TEXT_BYTES {
                bail!("resource_limit: materialized Office document");
            }
            self.blocks.push(block);
        }
        Ok(())
    }
}

fn block_size(block: &Block) -> usize {
    std::mem::size_of::<Block>()
        + block.label.len()
        + block.text.len()
        + block.html.as_ref().map_or(0, String::len)
}

fn same_group(first: &Value, other: &Value) -> bool {
    first.get("slide").is_some() && first["slide"] == other["slide"]
        || first.get("sheet").is_some()
            && first["sheet"] == other["sheet"]
            && first["row"] == other["row"]
}

/// Read as far as the requested window; memory usage is independent of its index.
pub(crate) fn preview(
    path: &Path,
    kind: &str,
    index: usize,
    focus: usize,
    length: usize,
) -> Result<(Vec<Block>, usize, Option<usize>)> {
    const BUDGET: usize = 8 * 1024 * 1024;
    let mut context = VecDeque::<Block>::new();
    let mut used = 0usize;
    let mut seen = 0;
    let mut selected = None;
    let mut local = 0;
    let mut complete = true;
    visit_window(path, kind, index.saturating_sub(64), |mut block| {
        let current = block.location["index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| anyhow::anyhow!("location: missing Office block index"))?;
        seen = current + 1;
        if current == index {
            if block_size(&block) > BUDGET {
                crate::protocol::clip_paragraph(&mut block, focus, length);
            }
            let mut first = context.len().saturating_sub(12);
            while first > 0 && same_group(&block.location, &context[first - 1].location) {
                first -= 1;
            }
            for _ in 0..first {
                used -= block_size(&context.pop_front().unwrap());
            }
            selected = Some(block.location.clone());
        } else if current > index {
            let distance = current - index;
            if distance > 64
                || distance > 12 && !same_group(selected.as_ref().unwrap(), &block.location)
            {
                complete = false;
                return Ok(false);
            }
        }
        let size = block_size(&block);
        if current <= index {
            while !context.is_empty()
                && (context.len() >= 64 && current < index || used + size > BUDGET)
            {
                used -= block_size(&context.pop_front().unwrap());
            }
            if current == index {
                local = context.len();
            }
        } else if used + size > BUDGET {
            complete = false;
            return Ok(false);
        }
        if size <= BUDGET || current == index {
            context.push_back(block);
            used += size;
        }
        Ok(true)
    })?;
    anyhow::ensure!(
        selected.is_some() || (seen == 0 && index == 0),
        "location: Block index is outside the document"
    );
    Ok((context.into(), local, complete.then_some(seen)))
}

#[derive(Default, Clone, Copy)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

// Only these tags and program-generated escape sequences reach the rich-text widget.
#[derive(Default)]
struct RichText {
    text: String,
    html: String,
    last: Option<u8>,
}

impl RichText {
    fn append(&mut self, value: &str, style: Style) -> Result<()> {
        if value.is_empty() {
            return Ok(());
        }
        let mask = style.bold as u8
            | (style.italic as u8) << 1
            | (style.underline as u8) << 2
            | (style.strike as u8) << 3;
        if self.last != Some(mask) {
            self.close();
            for (bit, tag) in [(1, "<b>"), (2, "<i>"), (4, "<u>"), (8, "<s>")] {
                if mask & bit != 0 {
                    self.html.push_str(tag);
                }
            }
            self.last = Some(mask);
        }
        self.text.push_str(value);
        for c in value.chars() {
            match c {
                '&' => self.html.push_str("&amp;"),
                '<' => self.html.push_str("&lt;"),
                '>' => self.html.push_str("&gt;"),
                '\n' => self.html.push_str("<br/>"),
                _ => self.html.push(c),
            }
        }
        if self.text.len() + self.html.len() > MAX_TEXT_BYTES {
            bail!("resource_limit: paragraph size");
        }
        Ok(())
    }
    fn close(&mut self) {
        if let Some(mask) = self.last.take() {
            for (bit, tag) in [(8, "</s>"), (4, "</u>"), (2, "</i>"), (1, "</b>")] {
                if mask & bit != 0 {
                    self.html.push_str(tag);
                }
            }
        }
    }
    fn finish(mut self, heading: Option<usize>) -> (String, String) {
        self.close();
        let tag = heading
            .map(|n| format!("h{}", n.clamp(1, 6)))
            .unwrap_or_else(|| "p".into());
        (
            self.text,
            format!(
                "<{tag} style=\"white-space:pre-wrap\">{}</{tag}>",
                self.html
            ),
        )
    }
}

fn paragraph_location(
    number: usize,
    heading: Option<usize>,
    table: Option<(usize, usize, usize)>,
) -> (Value, String) {
    let mut loc = json!({"paragraph":number});
    let mut label = format!("Paragraph {number}");
    if let Some(l) = heading {
        loc["heading_level"] = json!(l);
    }
    if let Some((t, r, c)) = table {
        loc["table"] = json!(t);
        loc["row"] = json!(r);
        loc["column"] = json!(c);
        label = format!("Table {t}, row {r}, column {c} · {label}");
    }
    (loc, label)
}

fn cell_name(mut col: usize, row: usize) -> Result<String> {
    if col == 0 || row == 0 {
        bail!("resource_limit: spreadsheet coordinates");
    }
    let mut letters = Vec::new();
    while col > 0 {
        col -= 1;
        letters.push((b'A' + (col % 26) as u8) as char);
        col /= 26;
    }
    Ok(format!(
        "{}{row}",
        letters.into_iter().rev().collect::<String>()
    ))
}

fn push_cell(out: &mut Output, sheet: &str, row: usize, col: usize, text: String) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let cell = cell_name(col, row)?;
    out.push(
        text,
        json!({"sheet":sheet,"cell":cell,"row":row,"column":col}),
        format!("{sheet}!{cell}"),
        None,
    )
}

fn name<'a>(n: Node<'_, 'a>) -> &'a str {
    n.tag_name().name()
}
