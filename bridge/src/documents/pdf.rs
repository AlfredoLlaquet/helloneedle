// SPDX-License-Identifier: GPL-3.0-or-later
//! Poppler emits pages in reading order. Commit a page only at its closing
//! tag so a failed layout conversion can resume in plain text without duplicates.

mod layout;
mod plain;
mod process;

use crate::Block;
use anyhow::{anyhow, bail, ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{io::Read, mem::size_of, path::Path};

const PAGE_MEMORY: usize = 128 * 1024 * 1024;

#[derive(Debug, Default)]
struct Line {
    text: String,
    words: Vec<WordBox>,
}

#[derive(Debug, Serialize)]
struct WordBox {
    start: usize,
    length: usize,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Layout,
    Plain,
}

struct Page {
    number: usize,
    width: f64,
    height: f64,
    mode: Mode,
    lines: Vec<Line>,
    working: usize,
}

impl Page {
    fn new(number: usize, width: f64, height: f64, mode: Mode) -> Self {
        Self {
            number,
            width,
            height,
            mode,
            lines: Vec::new(),
            working: 0,
        }
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.working = self.working.saturating_add(bytes);
        ensure!(
            self.working <= PAGE_MEMORY,
            "resource_limit: PDF page working memory"
        );
        Ok(())
    }

    fn push_line(&mut self, line: Line) -> Result<()> {
        if !line.text.trim().is_empty() {
            self.charge(size_of::<Line>())?;
            self.lines.push(line);
        }
        Ok(())
    }

    fn search(
        mut self,
        index: &mut usize,
        visit: &mut impl FnMut(Block) -> Result<bool>,
    ) -> Result<bool> {
        let lines = std::mem::take(&mut self.lines);
        if lines.is_empty() {
            // Image-only pages reserve a location for navigation.
            *index += 1;
        }
        for (ordinal, line) in lines.into_iter().enumerate() {
            let block = self.block(line, ordinal + 1, *index, None);
            *index += 1;
            if !visit(block)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn block(
        &self,
        line: Line,
        ordinal: usize,
        index: usize,
        selection: Option<(usize, usize)>,
    ) -> Block {
        let location = json!({"index":index, "page":self.number, "pageLine":ordinal});
        let mut block = Block::new(line.text, location, format!("Page {}", self.number));
        if self.mode == Mode::Plain {
            block.location["pdfPlain"] = json!(true);
        }
        if let Some((focus, length)) = selection {
            if block.text.len() > 256 * 1024 {
                block.location["lineTextLength"] = json!(block.text.encode_utf16().count());
                crate::protocol::clip_paragraph(&mut block, focus, length);
            }
            if self.mode == Mode::Layout {
                block.location["width"] = json!(self.width);
                block.location["height"] = json!(self.height);
                let mut words = line.words;
                if let Some(offset) = block.location["textOffset"].as_u64() {
                    // The page renderer uses original, unclipped UTF-16 positions.
                    let left = offset as usize;
                    let right = left + block.text.encode_utf16().count();
                    words.retain(|w| w.start < right && w.start + w.length > left);
                }
                block.location["words"] = json!(words);
            }
        }
        block
    }

    fn passage(
        &mut self,
        line: usize,
        index: usize,
        focus: usize,
        length: usize,
        pages: usize,
        complete_match: Option<(usize, usize)>,
    ) -> Result<(Vec<Block>, usize)> {
        if self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        ensure!(
            line <= self.lines.len(),
            "location: PDF line is outside the page"
        );
        let selected = line - 1;
        let total = self.lines.len();
        let content = std::mem::take(&mut self.lines[selected]);
        // A long match can continue beyond this text window. Its entire extent
        // on the single printed line needs just one rectangle.
        let mut bounds: Option<[f64; 4]> = None;
        if let Some((start, end)) = complete_match {
            for word in &content.words {
                if word.start < end && word.start + word.length > start {
                    let edge = [word.x, word.y, word.x + word.w, word.y + word.h];
                    bounds = Some(bounds.map_or(edge, |b| {
                        [
                            b[0].min(edge[0]),
                            b[1].min(edge[1]),
                            b[2].max(edge[2]),
                            b[3].max(edge[3]),
                        ]
                    }));
                }
            }
        }
        let mut selected_block = self.block(content, line, index, Some((focus, length)));
        if let Some([x, y, right, bottom]) = bounds {
            selected_block.location["fullMatchBox"] =
                json!({"x":x, "y":y, "w":right-x, "h":bottom-y});
        }
        // Most pages fit in one text view. For unusually dense pages keep the
        // selected line and disclose which lines the adjacent windows can open.
        let mut remaining = (1024 * 1024usize).saturating_sub(selected_block.text.len());
        let mut first = selected;
        let mut end = line;
        loop {
            let before = first;
            let after = end;
            if first > 0 {
                let cost = self.lines[first - 1].text.len() + 128;
                if cost <= remaining {
                    remaining -= cost;
                    first -= 1;
                }
            }
            if end < total {
                let cost = self.lines[end].text.len() + 128;
                if cost <= remaining {
                    remaining -= cost;
                    end += 1;
                }
            }
            if first == before && end == after {
                break;
            }
        }
        selected_block.location["pageCount"] = json!(pages);
        selected_block.location["firstPageLine"] = json!(first + 1);
        selected_block.location["lastPageLine"] = json!(end);
        selected_block.location["pageLines"] = json!(total);
        let mut selected_block = Some(selected_block);
        let mut blocks = Vec::with_capacity(end - first);
        for item in first..end {
            if item == selected {
                blocks.push(selected_block.take().unwrap());
            } else {
                let other = std::mem::take(&mut self.lines[item]);
                let global = if item < selected {
                    index.saturating_sub(selected - item)
                } else {
                    index.saturating_add(item - selected)
                };
                blocks.push(self.block(other, item + 1, global, None));
            }
        }
        Ok((blocks, selected - first))
    }
}

fn convert(
    path: &Path,
    mode: Mode,
    page: Option<usize>,
    skip: usize,
    geometry_line: Option<usize>,
    visit: impl FnMut(Page) -> Result<bool>,
) -> Result<()> {
    let mut command = crate::converters::command("pdftotext")
        .ok_or_else(|| anyhow!("tool_missing: pdftotext (Poppler utilities)"))?;
    if mode == Mode::Layout {
        command.arg("-bbox-layout");
    }
    if let Some(page) = page {
        let page = page.to_string();
        command.args(["-f", &page, "-l", &page]);
    }
    command
        .args(["-enc", "UTF-8"])
        .arg(std::path::absolute(path)?)
        .arg("-")
        .env("LC_ALL", "C");
    let first = page.unwrap_or(1);
    process::run(&mut command, |stream| match mode {
        Mode::Layout => layout::pages(stream, first, geometry_line, visit),
        Mode::Plain => plain::pages(stream, first, skip, visit),
    })
}

fn crashed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<process::DecoderFailure>()
        .is_some_and(|f| f.crashed())
}

fn both_failed(layout: anyhow::Error, plain: anyhow::Error) -> anyhow::Error {
    anyhow!("invalid_document: layout extraction: {layout:#}; plain-text extraction: {plain:#}")
}

pub(crate) fn scan(path: &Path, mut visit: impl FnMut(Block) -> Result<bool>) -> Result<()> {
    let mut index = 0;
    let mut completed = 0;
    let result = convert(path, Mode::Layout, None, 0, None, |page| {
        completed = page.number;
        page.search(&mut index, &mut visit)
    });
    match result {
        Err(layout) if crashed(&layout) => {
            // Plain and layout output can split a page's lines differently.
            // Count form feeds from the start to resume after complete pages.
            // This also handles a late crash after the last page, for which
            // passing -f (last+1) would produce a spurious error.
            convert(path, Mode::Plain, None, completed, None, |page| {
                page.search(&mut index, &mut visit)
            })
            .map_err(|plain| both_failed(layout, plain))
        }
        other => other,
    }
}

fn page_count(path: &Path) -> Result<usize> {
    let path = std::path::absolute(path)?;
    // FreeBSD and Linux install pdfinfo with Poppler. Older self-contained
    // installers have only pdftotext, whose plain output ends each page in FF.
    if let Some(mut info) = crate::converters::command("pdfinfo") {
        info.arg(&path).env("LC_ALL", "C");
        let mut bytes = Vec::new();
        let read = process::run(&mut info, |r| {
            r.take(512 * 1024).read_to_end(&mut bytes)?;
            Ok(bytes.len() < 512 * 1024)
        });
        if read.is_ok() {
            if let Some(count) = String::from_utf8_lossy(&bytes)
                .lines()
                .find_map(|l| {
                    l.strip_prefix("Pages:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .filter(|n| *n > 0)
            {
                return Ok(count);
            }
        }
    }
    let mut text = crate::converters::command("pdftotext")
        .ok_or_else(|| anyhow!("tool_missing: pdftotext (Poppler utilities)"))?;
    text.args(["-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .env("LC_ALL", "C");
    let mut count = 0;
    let mut trailing = false;
    process::run(&mut text, |r| {
        loop {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            count += buf.iter().filter(|&&b| b == 0x0c).count();
            trailing = !buf.ends_with(&[0x0c]);
            let len = buf.len();
            r.consume(len);
        }
        Ok(true)
    })?;
    Ok((count + usize::from(trailing)).max(1))
}

/// Extract only the selected page and collect geometry solely for its selected
/// line. Search locations preserve the source page even in text-only mode.
pub(crate) fn preview(
    path: &Path,
    location: &Value,
    focus: usize,
    length: usize,
) -> Result<(Vec<Block>, usize)> {
    let index = location
        .get("index")
        .map(|n| {
            n.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| anyhow!("location: invalid PDF block index"))
        })
        .transpose()?
        .unwrap_or(0);
    let page = location
        .get("page")
        .map(|n| {
            n.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| anyhow!("location: invalid PDF page"))
        })
        .transpose()?
        .unwrap_or(1);
    let line = location
        .get("pageLine")
        .map(|n| {
            n.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| anyhow!("location: invalid PDF line"))
        })
        .transpose()?;
    if index > 0 && line.is_none() {
        // Older clients sent a document-wide index without a page-local line.
        let mut found = None;
        scan(path, |block| {
            if block.location["index"] == index
                && (location.get("page").is_none() || block.location["page"] == page)
            {
                found = Some(block.location);
                return Ok(false);
            }
            Ok(true)
        })?;
        if let Some(known) = found {
            return preview(path, &known, focus, length);
        }
        if location.get("page").is_none() {
            bail!("location: PDF line is outside the document");
        }
    }
    let line = line.unwrap_or(1);
    let count = page_count(path)?;
    ensure!(page <= count, "location: PDF page is outside the document");
    let plain_requested = location["pdfPlain"] == true;
    let complete_match = location["highlightStart"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .zip(
            location["highlightLength"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()),
        )
        .and_then(|(start, length)| start.checked_add(length).map(|end| (start, end)));
    let mut selected = None;
    let mut take = |mut p: Page| -> Result<bool> {
        selected = Some(p.passage(line, index, focus, length, count, complete_match)?);
        // Observe the exit status even after a complete preview page.
        Ok(true)
    };
    if plain_requested {
        convert(path, Mode::Plain, Some(page), 0, None, &mut take)?;
    } else if let Err(layout) = convert(path, Mode::Layout, Some(page), 0, Some(line), &mut take) {
        if !crashed(&layout) {
            return Err(layout);
        }
        convert(path, Mode::Plain, Some(page), 0, None, &mut take)
            .map_err(|plain| both_failed(layout, plain))?;
    }
    selected.ok_or_else(|| anyhow!("location: PDF page is unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn text_view_preserves_an_entire_page_and_pages_through_dense_neighbors() {
        let mut page = Page::new(7, 600.0, 800.0, Mode::Layout);
        for text in ["before", "chosen", "after"] {
            page.lines.push(Line {
                text: text.into(),
                words: vec![],
            });
        }
        let (blocks, selected) = page.passage(2, 200, 0, 6, 12, None).unwrap();
        assert_eq!(selected, 1);
        assert_eq!(
            blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            ["before", "chosen", "after"]
        );
        assert_eq!(blocks[selected].location["index"], 200);
        assert_eq!(blocks[0].location["index"], 199);
        assert_eq!(blocks[selected].location["firstPageLine"], 1);
        assert_eq!(blocks[selected].location["lastPageLine"], 3);

        let mut dense = Page::new(7, 600.0, 800.0, Mode::Plain);
        for _ in 0..4 {
            dense.lines.push(Line {
                text: "y".repeat(520 * 1024),
                words: vec![],
            });
        }
        let (blocks, selected) = dense.passage(2, 200, 10_000, 30_000, 12, None).unwrap();
        assert!(blocks[selected].text.len() >= 30_000);
        assert_eq!(blocks[selected].location["pageLines"], 4);
        assert!(blocks[selected].location["lastPageLine"].as_u64().unwrap() < 4);
    }

    #[test]
    fn layout_coordinates_and_unicode_offsets_are_page_local() {
        let xml = r#"<doc><page width="100" height="200"/><page width="500" height="400"><flow><block><line>
          <word xMin="0" yMin="10" xMax="20" yMax="31">🪡&amp;</word>
          <word xMin="40" yMin="10" xMax="60" yMax="31">needle</word>
          <word xMin="80" yMin="10" xMax="100" yMax="31">needle</word>
          </line></block></flow></page></doc>"#;
        let mut pages = Vec::new();
        layout::pages(&mut Cursor::new(xml), 1, Some(1), |p| {
            pages.push(p);
            Ok(true)
        })
        .unwrap();
        assert!(pages[0].lines.is_empty());
        let mut page = pages.pop().unwrap();
        let line = page.lines.pop().unwrap();
        let block = page.block(line, 1, 1, Some((0, 0)));
        assert_eq!(block.text, "🪡& needle needle");
        assert_eq!(block.location["page"], 2);
        assert_eq!(block.location["pageLine"], 1);
        let boxes = &block.location["words"];
        assert_eq!(boxes[1]["start"], 4);
        assert_eq!(boxes[2]["start"], 11);
        assert_eq!(boxes[2]["x"], 80.0);
        assert_eq!(boxes[2]["h"], 21.0);
        assert!(layout::pages(
            &mut Cursor::new(br#"<page width="NaN" height="10"/>"#),
            1,
            None,
            |_| Ok(true)
        )
        .is_err());
    }

    #[test]
    fn form_feeds_preserve_empty_pages_and_skip_committed_layout_pages() {
        let input = "first\n\u{000c}\u{000c}dos\r\nneedle🧵\u{000c}";
        let mut stream = BufReader::with_capacity(2, Cursor::new(input.as_bytes()));
        let mut pages = Vec::new();
        plain::pages(&mut stream, 1, 1, |p| {
            pages.push(p);
            Ok(true)
        })
        .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].number, 2);
        assert!(pages[0].lines.is_empty());
        assert_eq!(pages[1].number, 3);
        assert_eq!(pages[1].lines[1].text, "needle🧵");
    }
}
