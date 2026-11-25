// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Line, Mode, Page, WordBox, PAGE_MEMORY};
use anyhow::{anyhow, bail, ensure, Context, Result};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::{
    io::{BufRead, Read},
    mem::size_of,
};

fn number(tag: &BytesStart<'_>, name: &[u8]) -> Result<f64> {
    for attr in tag.attributes() {
        let attr = attr?;
        if attr.key.as_ref() == name {
            let n: f64 = std::str::from_utf8(&attr.value)?.parse()?;
            ensure!(n.is_finite(), "invalid_document: nonfinite PDF coordinate");
            return Ok(n);
        }
    }
    bail!("invalid_document: missing PDF coordinate");
}

pub(super) fn pages(
    input: &mut dyn BufRead,
    first: usize,
    geometry_line: Option<usize>,
    mut visit: impl FnMut(Page) -> Result<bool>,
) -> Result<bool> {
    let mut reader = Reader::from_reader(input.take(PAGE_MEMORY as u64));
    reader.config_mut().expand_empty_elements = true;
    let mut tagbuf = Vec::new();
    let mut page = None::<Page>;
    let mut next = first;
    let mut line = None::<Line>;
    let mut utf16 = 0;
    let mut word = None::<String>;
    let mut bounds = None;
    loop {
        // A single token or one working page may be pathologically large.
        // Reset the token allowance on each event, independently of file size.
        reader.get_mut().set_limit(PAGE_MEMORY as u64);
        let ev = reader
            .read_event_into(&mut tagbuf)
            .context("invalid_document: malformed PDF layout output")?;
        ensure!(
            reader.get_ref().limit() != 0,
            "resource_limit: PDF layout token working memory"
        );
        match ev {
            Event::Start(tag) => match tag.name().as_ref() {
                b"page" => {
                    ensure!(page.is_none(), "invalid_document: nested PDF pages");
                    let (w, h) = (number(&tag, b"width")?, number(&tag, b"height")?);
                    ensure!(
                        w > 0.0 && h > 0.0,
                        "invalid_document: invalid PDF page size"
                    );
                    page = Some(Page::new(next, w, h, Mode::Layout));
                }
                b"line" if page.is_some() => {
                    line = Some(Line::default());
                    utf16 = 0;
                }
                b"word" if line.is_some() => {
                    word = Some(String::new());
                    if page
                        .as_ref()
                        .is_some_and(|p| geometry_line == Some(p.lines.len() + 1))
                    {
                        let x = number(&tag, b"xMin")?;
                        let y = number(&tag, b"yMin")?;
                        bounds = Some((
                            x,
                            y,
                            (number(&tag, b"xMax")? - x).max(0.0),
                            (number(&tag, b"yMax")? - y).max(0.0),
                        ));
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let (Some(word), Some(p)) = (&mut word, &mut page) {
                    p.charge(text.len())?;
                    word.push_str(std::str::from_utf8(&text)?);
                }
            }
            Event::GeneralRef(reference) => {
                if let (Some(word), Some(p)) = (&mut word, &mut page) {
                    let entity = format!("&{};", std::str::from_utf8(&reference)?);
                    let decoded = quick_xml::escape::unescape(&entity)?;
                    p.charge(decoded.len())?;
                    word.push_str(&decoded);
                }
            }
            Event::End(tag) => match tag.name().as_ref() {
                b"word" => {
                    let rectangle = bounds.take();
                    if let (Some(text), Some(line), Some(p)) = (word.take(), &mut line, &mut page) {
                        if !text.is_empty() {
                            if !line.text.is_empty() {
                                p.charge(1)?;
                                line.text.push(' ');
                                utf16 += 1;
                            }
                            if let Some((x, y, w, h)) = rectangle {
                                let length = text.encode_utf16().count();
                                p.charge(size_of::<WordBox>())?;
                                line.words.push(WordBox {
                                    start: utf16,
                                    length,
                                    x,
                                    y,
                                    w,
                                    h,
                                });
                                utf16 += length;
                            }
                            line.text.push_str(&text);
                        }
                    }
                }
                b"line" => {
                    if let (Some(p), Some(line)) = (&mut page, line.take()) {
                        p.push_line(line)?;
                    }
                }
                b"page" => {
                    let finished = page
                        .take()
                        .ok_or_else(|| anyhow!("invalid_document: unmatched PDF page"))?;
                    if !visit(finished)? {
                        return Ok(false);
                    }
                    next += 1;
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        tagbuf.clear();
    }
    ensure!(
        next > first && page.is_none(),
        "invalid_document: PDF has no complete pages"
    );
    Ok(true)
}
