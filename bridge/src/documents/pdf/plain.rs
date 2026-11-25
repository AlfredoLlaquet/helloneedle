// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Line, Mode, Page};
use anyhow::{Context, Result};
use std::io::BufRead;

fn finish_line(page: &mut Page, pending: &mut Vec<u8>) -> Result<()> {
    while pending.last() == Some(&b'\r') {
        pending.pop();
    }
    let bytes = std::mem::take(pending);
    let text =
        String::from_utf8(bytes).context("invalid_document: pdftotext produced invalid UTF-8")?;
    page.push_line(Line {
        text,
        words: Vec::new(),
    })
}

/// Plain pdftotext output uses a form feed for every page, including blank
/// pages and the last page. A line or UTF-8 character may cross pipe reads.
pub(super) fn pages(
    input: &mut dyn BufRead,
    first: usize,
    skip: usize,
    mut visit: impl FnMut(Page) -> Result<bool>,
) -> Result<bool> {
    let mut page = Page::new(first, 0.0, 0.0, Mode::Plain);
    let mut text = Vec::new();
    let mut started = false;
    let mut saw_page = false;
    loop {
        let buf = input.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        let mut start = 0;
        for (i, &b) in buf
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b, b'\n' | 0x0c))
        {
            if page.number > skip {
                page.charge(i - start)?;
                text.extend_from_slice(&buf[start..i]);
                finish_line(&mut page, &mut text)?;
            }
            start = i + 1;
            started = true;
            if b == 0x0c {
                let next = page.number + 1;
                if page.number > skip && !visit(page)? {
                    return Ok(false);
                }
                page = Page::new(next, 0.0, 0.0, Mode::Plain);
                started = false;
                saw_page = true;
            }
        }
        if start < buf.len() {
            started = true;
            if page.number > skip {
                page.charge(buf.len() - start)?;
                text.extend_from_slice(&buf[start..]);
            }
        }
        let n = buf.len();
        input.consume(n);
    }
    if (started || !saw_page) && page.number > skip {
        finish_line(&mut page, &mut text)?;
        return visit(page);
    }
    Ok(true)
}
