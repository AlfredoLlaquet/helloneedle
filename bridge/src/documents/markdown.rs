// SPDX-License-Identifier: GPL-3.0-or-later
//! Decode source incrementally. Independent paragraph runs can be parsed in batches.
//! Where CommonMark requires document-wide definitions or block state, use one parser
//! and render balanced groups of its resolved events. Raw HTML can span Markdown blocks.
use super::{html, text_stream::markdown_source};
use crate::Block;
use anyhow::{anyhow, ensure, Result};
use pulldown_cmark::{html::write_html_fmt, Event, Options, Parser};
use serde_json::json;
use std::{collections::VecDeque, fmt, path::Path};

const BATCH: usize = 256 * 1024;
const PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const WORKING_MEMORY: usize = html::DOCUMENT_MEMORY;
const RESOURCE_ERROR: &str = "resource_limit: Markdown parser exceeds its working memory budget";

/// Account separately for decoded storage, parser structures and emitted HTML. Short
/// lines and punctuation can create many parser nodes from comparatively few bytes.
fn parser_cost(text: &str) -> usize {
    let structural = text
        .bytes()
        .filter(|c| {
            matches!(
                c,
                b'\n'
                    | b'['
                    | b']'
                    | b'!'
                    | b'*'
                    | b'_'
                    | b'~'
                    | b'`'
                    | b'<'
                    | b'>'
                    | b'|'
                    | b'\\'
                    | b'&'
                    | b'{'
                    | b'#'
            )
        })
        .count();
    text.len()
        .saturating_mul(3)
        .saturating_add(structural.saturating_mul(128))
}

fn append(buffer: &mut String, text: &str, limit: usize) -> Result<()> {
    let needed = buffer
        .len()
        .checked_add(text.len())
        .ok_or_else(|| anyhow!(RESOURCE_ERROR))?;
    ensure!(needed <= limit, RESOURCE_ERROR);
    if needed > buffer.capacity() {
        let capacity = needed.max(buffer.capacity().saturating_mul(2)).min(limit);
        buffer
            .try_reserve_exact(capacity - buffer.len())
            .map_err(|_| anyhow!(RESOURCE_ERROR))?;
    }
    buffer.push_str(text);
    Ok(())
}

struct Rendered {
    text: String,
    limit: usize,
}

impl fmt::Write for Rendered {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        append(&mut self.text, text, self.limit).map_err(|_| fmt::Error)
    }
}

/// Use the real parser for reference lookups, tight/loose lists, fences, tables
/// and inline content. No byte slice is interpreted as a new Markdown document here.
pub(super) fn visit_source(
    source: &str,
    visitor: &mut dyn FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let remaining = WORKING_MEMORY
        .checked_sub(parser_cost(source))
        .ok_or_else(|| anyhow!(RESOURCE_ERROR))?;
    let raw_html = source.contains('<');
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events = Parser::new_ext(source, options)
        .into_offset_iter()
        .peekable();
    let mut keep_going = true;
    while keep_going && events.peek().is_some() {
        crate::search::checkpoint()?;
        let start = events.peek().unwrap().1.start;
        let mut consumed = start;
        let mut depth = 0usize;
        let mut since_checkpoint = 0usize;
        let mut interrupted = None;
        let batch = std::iter::from_fn(|| {
            if !raw_html && depth == 0 && consumed.saturating_sub(start) >= BATCH {
                return None;
            }
            since_checkpoint += 1;
            if since_checkpoint == 1024 {
                since_checkpoint = 0;
                if let Err(error) = crate::search::checkpoint() {
                    interrupted = Some(error);
                    return None;
                }
            }
            let (event, range) = events.next()?;
            match &event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth -= 1,
                _ => {}
            }
            consumed = consumed.max(range.end);
            Some(event)
        });
        let mut output = Rendered {
            text: String::new(),
            limit: remaining,
        };
        write_html_fmt(&mut output, batch).map_err(|_| anyhow!(RESOURCE_ERROR))?;
        if let Some(error) = interrupted {
            return Err(error.into());
        }
        let tree_budget = remaining
            .checked_sub(output.text.capacity())
            .ok_or_else(|| anyhow!(RESOURCE_ERROR))?;
        html::rendered(output.text.as_bytes(), tree_budget, &mut |block| {
            keep_going = visitor(block)?;
            Ok(keep_going)
        })?;
    }
    Ok(())
}

/// A blank physical line ends an ordinary paragraph. Anything which could carry
/// state across that line (definitions, list continuations, raw HTML, fences or
/// indentation) selects the full document parser. Inspection retains no source.
fn independent_paragraphs(path: &Path) -> Result<bool> {
    let mut independent = true;
    let mut beginning = true;
    markdown_source(path, |part, ends_line| {
        if part.contains(['<', '['])
            || (beginning
                && part.as_bytes().first().is_some_and(|c| {
                    matches!(
                        c,
                        b' ' | b'\t' | b'>' | b'-' | b'+' | b'*' | b'~' | b'`' | b'0'..=b'9'
                    )
                }))
        {
            independent = false;
            return Ok(false);
        }
        beginning = ends_line || (beginning && part.is_empty());
        Ok(true)
    })?;
    Ok(independent)
}

pub(crate) fn visit_markdown(
    path: &Path,
    mut visit: impl FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let independent = independent_paragraphs(path)?;
    let mut index = 0usize;
    let mut running = true;
    let mut source = String::new();
    let mut cost = 0usize;
    let mut line_content = false;
    let mut project = |source: &str| -> Result<bool> {
        let mut more = true;
        visit_source(source, &mut |mut block| {
            block.location["index"] = json!(index);
            index += 1;
            block.label = format!("Block {index}");
            more = visit(block)?;
            Ok(more)
        })?;
        Ok(more)
    };
    markdown_source(path, |part, end| {
        cost = cost.saturating_add(parser_cost(part));
        if end {
            cost = cost.saturating_add(parser_cost("\n"));
        }
        ensure!(cost < WORKING_MEMORY, RESOURCE_ERROR);
        append(&mut source, part, WORKING_MEMORY / 3)?;
        line_content |= !part.is_empty();
        if end {
            append(&mut source, "\n", WORKING_MEMORY / 3)?;
            if independent && !line_content && source.len() >= BATCH {
                running = project(&source)?;
                source.clear();
                cost = 0;
            }
            line_content = false;
        }
        Ok(running)
    })?;
    if running && !source.is_empty() {
        project(&source)?;
    }
    Ok(())
}

pub(crate) fn preview_markdown(
    path: &Path,
    selected: usize,
    focus: usize,
    length: usize,
) -> Result<(Vec<Block>, usize, Option<usize>)> {
    let mut blocks = VecDeque::new();
    let mut bytes = 0usize;
    let mut seen = 0usize;
    let mut selected_index = 0usize;
    let mut finished = true;
    let size = |block: &Block| block.text.len() + block.html.as_ref().map_or(0, String::len);
    visit_markdown(path, |mut block| {
        let index = seen;
        seen += 1;
        if index < selected.saturating_sub(12) {
            return Ok(true);
        }
        if index == selected && size(&block) > PREVIEW_BYTES {
            crate::protocol::clip_paragraph(&mut block, focus, length);
        }
        let n = size(&block);
        if index <= selected {
            while !blocks.is_empty() && bytes + n > PREVIEW_BYTES {
                bytes -= size(&blocks.pop_front().unwrap());
            }
            if index == selected {
                selected_index = blocks.len();
            }
        } else if index - selected > 12 || bytes + n > PREVIEW_BYTES {
            finished = false;
            return Ok(false);
        }
        blocks.push_back(block);
        bytes += n;
        Ok(true)
    })?;
    ensure!(
        selected < seen || (selected == 0 && seen == 0),
        "location: Markdown block is outside the document"
    );
    Ok((blocks.into(), selected_index, finished.then_some(seen)))
}
