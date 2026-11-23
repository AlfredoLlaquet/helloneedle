// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured Office and rich-text readers. All document relationships stay inside the archive.
use crate::{Block, Document, MAX_FILE_BYTES, MAX_TEXT_BYTES};
use anyhow::{bail, Result};
use roxmltree::Node;
use serde_json::{json, Value};
use std::{fs::File, io::Read, path::Path};

mod legacy;
mod odf;
mod ooxml;
mod package;
mod rtf;
#[cfg(test)]
mod tests;

// Bound allocations independently of a ZIP's reported sizes and ODF's repeat counts.
const MAX_BLOCKS: usize = 100_000;
const MAX_XML_NODES: u32 = 350_000;
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

pub fn extract(path: &Path, extension: &str) -> Result<Option<Document>> {
    let kind = extension.trim_start_matches('.').to_ascii_lowercase();
    if !matches!(
        kind.as_str(),
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "fodt" | "doc" | "rtf"
    ) {
        return Ok(None);
    }
    let file = File::open(path).map_err(|e| anyhow::anyhow!("unreadable_document: {e}"))?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        bail!("resource_limit: document size");
    }
    let mut out = Output::default();
    match kind.as_str() {
        "doc" => legacy::extract(path, file, &mut out)?,
        "rtf" => rtf::extract(&read_bounded(file)?, &mut out)?,
        "fodt" => odf::extract(
            &package::decode_xml(&read_bounded(file)?)?,
            None,
            &kind,
            &mut out,
        )?,
        _ => {
            let mut pkg = package::Package::open(file)?;
            if matches!(kind.as_str(), "docx" | "xlsx" | "pptx") {
                ooxml::extract(&mut pkg, &kind, &mut out)?;
            } else {
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
                let content = pkg.required("content.xml")?;
                let styles = pkg.optional("styles.xml")?;
                odf::extract(&content, styles.as_deref(), &kind, &mut out)?;
            }
        }
    }
    Ok(Some(Document {
        kind,
        blocks: out.blocks,
    }))
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    reader
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut b)
        .map_err(|e| anyhow::anyhow!("unreadable_document: {e}"))?;
    if b.len() as u64 > MAX_FILE_BYTES {
        bail!("resource_limit: document size");
    }
    Ok(b)
}

#[derive(Default)]
struct Output {
    blocks: Vec<Block>,
    bytes: usize,
}

impl Output {
    fn push(
        &mut self,
        text: String,
        mut location: Value,
        label: String,
        html: Option<String>,
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.bytes += text.len() + html.as_ref().map_or(0, String::len) + label.len();
        if self.bytes > MAX_TEXT_BYTES || self.blocks.len() >= MAX_BLOCKS {
            bail!("resource_limit: extracted document");
        }
        location["index"] = json!(self.blocks.len());
        let mut block = Block::new(text, location, label);
        if let Some(html) = html {
            block = block.with_html(html);
        }
        self.blocks.push(block);
        Ok(())
    }
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
    if !(1..=MAX_COLS).contains(&col) || !(1..=MAX_ROWS).contains(&row) {
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
