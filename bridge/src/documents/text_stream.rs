// SPDX-License-Identifier: GPL-3.0-or-later
//! Match physical source lines while retaining only a bounded part of a long literal line.

mod source;
#[cfg(test)]
mod tests;

use self::source::{byte_len, can_seek, utf16_left, Charset, NotText, BUFFER};
use crate::Block;
use anyhow::{anyhow, ensure, Result};
use encoding_rs::Encoding;
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::Path;

const CONTEXT_LINES: usize = 12;
const PREVIEW_UNITS: usize = 8_192;
const REGEX_LINE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
struct Position {
    byte: u64,
    line: usize,
    column: usize, // UTF-16 units from the beginning of this physical line.
}

fn units(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

fn left(text: &str, end: usize, chars: usize) -> usize {
    let at = end.saturating_sub(chars);
    if text.as_bytes()[at..end].is_ascii() {
        at
    } else {
        text[..end]
            .char_indices()
            .rev()
            .nth(chars - 1)
            .map_or(0, |(i, _)| i)
    }
}

fn right(text: &str, start: usize, chars: usize) -> usize {
    let at = (start + chars).min(text.len());
    if text.as_bytes()[start..at].is_ascii() {
        at
    } else {
        text[start..]
            .char_indices()
            .nth(chars)
            .map_or(text.len(), |(i, _)| start + i)
    }
}

/// A large line gets a bookmark near its match. For adjacent short lines the bookmark also
/// provides the preceding preview context, without storing an index of the entire file.
fn location(
    text: &str,
    start: usize,
    at: Position,
    preceding: Position,
    charset: Charset,
) -> Value {
    let mut loc = json!({"index":at.line,"line":at.line + 1});
    if can_seek(charset) {
        let anchor = if at.byte.saturating_sub(preceding.byte) <= 64 * 1024 {
            preceding
        } else {
            let prefix = &text[left(text, start, PREVIEW_UNITS)..start];
            let n = units(prefix);
            Position {
                byte: at.byte - byte_len(charset, prefix, n),
                line: at.line,
                column: at.column - n,
            }
        };
        loc["sourceByte"] = json!(anchor.byte);
        loc["sourceLine"] = json!(anchor.line);
        loc["sourceColumn"] = json!(anchor.column);
        if let Some(enc) = charset {
            loc["sourceEncoding"] = json!(enc.name());
        }
    }
    loc
}

struct Window<'a, F> {
    matcher: &'a RegexMatcher,
    hit: F,
}

impl<F> Window<'_, F>
where
    F: FnMut(&Value, &str, usize, usize, usize, usize) -> Result<bool>,
{
    /// Only starts below `safe_end` have enough lookahead for an entire literal match and its
    /// excerpt. Return the first unsearched position, preserving nonoverlapping matches.
    fn matches(
        &mut self,
        text: &str,
        origin: Position,
        charset: Charset,
        preceding: Position,
        from: usize,
        safe_end: Option<usize>,
    ) -> Result<(bool, usize)> {
        let mut cursor = origin;
        let mut previous = 0;
        let mut keep_going = true;
        let mut resume = safe_end.unwrap_or(text.len());
        self.matcher.try_find_iter_at(text.as_bytes(), from, |m| {
            let (start, end) = (m.start(), m.end());
            if safe_end.is_some_and(|bound| start >= bound) {
                return Ok(false);
            }
            // The regex syntax also supports explicit byte classes.
            if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                return Ok(true);
            }
            cursor = cursor.after(&text[previous..start], charset);
            previous = start;
            let begin = left(text, start, 80);
            let finish = right(text, start, 240);
            let loc = location(text, start, cursor, preceding, charset);
            keep_going = (self.hit)(
                &loc,
                &text[begin..finish],
                units(&text[begin..start]),
                units(&text[start..end.min(finish)]),
                cursor.column,
                units(&text[start..end]),
            )?;
            resume = resume.max(end);
            Ok::<_, anyhow::Error>(keep_going)
        })??;
        Ok((keep_going, resume))
    }
}

pub(crate) fn search(
    path: &Path,
    kind: &str,
    matcher: &RegexMatcher,
    query: &str,
    regex: bool,
    hit: impl FnMut(&Value, &str, usize, usize, usize, usize) -> Result<bool>,
) -> Result<bool> {
    let mut window = Window { matcher, hit };
    let mut recent = VecDeque::<Position>::with_capacity(CONTEXT_LINES + 1);
    let mut pending = String::new();
    let mut origin = Position::default();
    let mut from = 0;
    // Unicode simple case folding preserves scalar count but may change UTF-8 byte lengths.
    // Also reserve enough bytes on the right for the displayed excerpt.
    let overlap = 4 * query.len().max(240);
    let whole_line = regex
        && query.contains([
            '\\', '.', '^', '$', '|', '*', '+', '?', '(', ')', '[', ']', '{', '}',
        ]);
    let result = source::walk(
        path,
        kind == "html",
        None,
        |part, pos, charset, ends_line| {
            if recent.back().is_none_or(|last| last.line != pos.line) {
                if recent.len() == CONTEXT_LINES + 1 {
                    recent.pop_front();
                }
                recent.push_back(pos);
            }
            let preceding = recent[0];
            if pending.is_empty() {
                origin = pos;
                if ends_line {
                    return Ok(window.matches(part, pos, charset, preceding, 0, None)?.0);
                }
            }
            if whole_line {
                let needed = pending.len().saturating_add(part.len());
                ensure!(needed <= REGEX_LINE_BYTES, "resource_limit: regular expression needs more than 256 MiB for one source line");
                if needed > pending.capacity() {
                    let capacity = needed
                        .max(pending.capacity().saturating_mul(2))
                        .min(REGEX_LINE_BYTES);
                    pending
                        .try_reserve_exact(capacity - pending.len())
                        .map_err(|_| {
                            anyhow!(
                                "resource_limit: insufficient memory for regular-expression line"
                            )
                        })?;
                }
            }
            pending.push_str(part);
            if ends_line {
                let (more, _) = window.matches(&pending, origin, charset, preceding, from, None)?;
                pending.clear();
                from = 0;
                return Ok(more);
            }
            if !whole_line && pending.len() >= overlap + BUFFER.max(overlap) {
                let mut safe = pending.len() - overlap;
                while !pending.is_char_boundary(safe) {
                    safe -= 1;
                }
                let (more, resume) =
                    window.matches(&pending, origin, charset, preceding, from, Some(safe))?;
                if !more {
                    return Ok(false);
                }
                // Retain sufficient left context for a selected preview as well as a search excerpt.
                let cut = left(&pending, resume, PREVIEW_UNITS);
                origin = origin.after(&pending[..cut], charset);
                pending.drain(..cut);
                from = resume - cut;
            }
            Ok(true)
        },
    );
    match result {
        Err(e)
            if e.is::<NotText>()
                && kind == "text"
                && !path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("txt")) =>
        {
            Ok(false)
        }
        other => other.map(|_| true),
    }
}

fn usize_value(value: &Value) -> Option<usize> {
    usize::try_from(value.as_u64()?).ok()
}

fn bookmark(loc: &Value, target: usize) -> Result<Option<(Position, Charset)>> {
    let Some(byte) = loc.get("sourceByte") else {
        return Ok(None);
    };
    let bad = || anyhow!("location: invalid source bookmark");
    let byte = byte.as_u64().ok_or_else(bad)?;
    let line = usize_value(&loc["sourceLine"]).ok_or_else(bad)?;
    let column = usize_value(&loc["sourceColumn"]).ok_or_else(bad)?;
    ensure!(
        line <= target
            && line as u64 <= byte
            && column as u64 <= byte
            && loc["line"].as_u64() == (target as u64).checked_add(1),
        "location: inconsistent source line"
    );
    let encoding = match loc.get("sourceEncoding") {
        None => None,
        Some(label) => {
            Some(Encoding::for_label(label.as_str().ok_or_else(bad)?.as_bytes()).ok_or_else(bad)?)
        }
    };
    ensure!(
        can_seek(encoding),
        "location: source encoding is not seekable"
    );
    Ok(Some((Position { byte, line, column }, encoding)))
}

pub(crate) fn preview(
    path: &Path,
    kind: &str,
    location: &Value,
    block_start: usize,
) -> Result<(Vec<Block>, usize, Option<usize>)> {
    let target = match location.get("index") {
        None => 0,
        Some(i) => usize_value(i).ok_or_else(|| anyhow!("location: invalid source line"))?,
    };
    let seek = bookmark(location, target)?;
    let first = target.saturating_sub(CONTEXT_LINES);
    let last = target.saturating_add(CONTEXT_LINES);
    let mut blocks = Vec::new();
    let mut selected = None;
    let mut text = String::new();
    let mut offset = None;
    let mut known_lines = Some(0);
    source::walk(path, kind == "html", seek, |part, pos, _, ends_line| {
        if pos.line < first {
            return Ok(true);
        }
        if pos.line > last {
            known_lines = None;
            return Ok(false);
        }
        let focus = if pos.line == target { block_start } else { 0 };
        let lo = focus.saturating_sub(PREVIEW_UNITS);
        let hi = focus.saturating_add(2 * PREVIEW_UNITS);
        let n = units(part);
        let end_column = pos.column + n;
        if pos.column < hi && lo <= end_column {
            let a = utf16_left(part, lo.saturating_sub(pos.column));
            let b = utf16_left(part, hi.saturating_sub(pos.column));
            offset.get_or_insert_with(|| pos.column + units(&part[..a]));
            text.push_str(&part[a..b]);
        }
        // For a one-line export, a click near its beginning should not read through gigabytes
        // of trailing text just to find the next newline.
        let clipped = pos.line == target && end_column >= hi && !ends_line;
        if ends_line || clipped {
            let mut loc = json!({"index":pos.line, "line":pos.line + 1});
            if let Some(start) = offset.take().filter(|&v| v != 0) {
                loc["textOffset"] = json!(start);
            }
            if pos.line == target {
                selected = Some(blocks.len());
            }
            blocks.push(Block::new(
                std::mem::take(&mut text),
                loc,
                format!("Line {}", pos.line + 1),
            ));
            known_lines = if clipped { None } else { Some(pos.line + 1) };
        }
        Ok(!clipped)
    })?;
    let selected =
        selected.ok_or_else(|| anyhow!("location: source line is outside the document"))?;
    Ok((blocks, selected, known_lines))
}
