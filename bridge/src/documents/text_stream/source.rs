// SPDX-License-Identifier: GPL-3.0-or-later
//! Buffered decoding shared by search and seekable source previews.

use super::{units, Position};
use anyhow::{ensure, Context, Result};
use encoding_rs::{DecoderResult, Encoding, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(super) const BUFFER: usize = 256 * 1024;
pub(super) type Charset = Option<&'static Encoding>;

#[derive(Debug)]
pub(super) struct NotText;

impl fmt::Display for NotText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported_encoding: file does not contain readable text in a supported encoding"
        )
    }
}

impl std::error::Error for NotText {}

pub(super) fn can_seek(encoding: Charset) -> bool {
    encoding.is_none_or(|e| e == UTF_8 || e == UTF_16LE || e == UTF_16BE || e == WINDOWS_1252)
}

pub(super) fn byte_len(encoding: Charset, text: &str, utf16: usize) -> u64 {
    if encoding == Some(UTF_16LE) || encoding == Some(UTF_16BE) {
        utf16 as u64 * 2
    } else if encoding == Some(WINDOWS_1252) && !text.is_ascii() {
        text.chars().count() as u64
    } else {
        // Non-seekable legacy HTML encodings never export these offsets.
        text.len() as u64
    }
}

fn forbidden(bytes: &[u8], html: bool) -> bool {
    if html {
        bytes.contains(&0)
    } else {
        super::super::has_binary_controls(bytes)
    }
}

fn windows_controls(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|b| matches!(b, 0x7f | 0x81 | 0x8d | 0x8f | 0x90 | 0x9d))
}

/// Until the first non-ASCII byte, UTF-8 and Windows-1252 yield identical searchable text,
/// offsets and source positions. Resolve the ambiguity once, without retaining the file or
/// exposing matches under a tentative decoding. Pure ASCII files use one sequential pass.
fn unmarked_charset(first: &[u8], file: &mut File, html: bool) -> Result<&'static Encoding> {
    let resume = file.stream_position()?;
    let mut remainder = Vec::with_capacity(BUFFER + 3);
    remainder.extend_from_slice(first);
    let encoding = loop {
        if forbidden(&remainder, html) {
            return Err(NotText.into());
        }
        let tail = match std::str::from_utf8(&remainder) {
            Ok(_) => 0,
            Err(e) if e.error_len().is_none() => remainder.len() - e.valid_up_to(),
            Err(_) => break WINDOWS_1252,
        };
        // A byte which could only be an unwanted control in Windows-1252 resolves the
        // ambiguity immediately. The UTF-8 decoder continues to validate later input.
        if !html && windows_controls(&remainder) {
            break UTF_8;
        }
        let end = remainder.len();
        remainder.copy_within(end - tail..end, 0);
        remainder.resize(BUFFER + tail, 0);
        let n = file.read(&mut remainder[tail..])?;
        if n == 0 {
            break if tail == 0 { UTF_8 } else { WINDOWS_1252 };
        }
        remainder.truncate(tail + n);
    };
    file.seek(SeekFrom::Start(resume))?;
    Ok(encoding)
}

fn initial_charset(file: &mut File, html: bool) -> Result<(Charset, u64)> {
    let mut head = Vec::with_capacity(1024);
    file.by_ref().take(1024).read_to_end(&mut head)?;
    if let Some((encoding, n)) = Encoding::for_bom(&head) {
        return Ok((Some(encoding), n as u64));
    }
    let encoding = if html {
        super::super::markup::declared_encoding(&head).map(|encoding| {
            if encoding == UTF_16BE || encoding == UTF_16LE {
                UTF_8
            } else {
                encoding
            }
        })
    } else {
        None
    };
    Ok((encoding, 0))
}

/// CR, LF and CRLF each finish one physical line, including between two decode buffers.
struct Lines {
    position: Position,
    skip_lf: bool,
    emitted: bool,
}

impl Lines {
    fn chunk(
        &mut self,
        mut text: &str,
        encoding: Charset,
        visit: &mut impl FnMut(&str, Position, Charset, bool) -> Result<bool>,
    ) -> Result<bool> {
        while !text.is_empty() {
            if self.skip_lf {
                self.skip_lf = false;
                if let Some(rest) = text.strip_prefix('\n') {
                    self.position.byte += byte_len(encoding, "\n", 1);
                    text = rest;
                    continue;
                }
            }
            let end = text.find(['\r', '\n']).unwrap_or(text.len());
            let part = &text[..end];
            let finished = end < text.len();
            if !visit(part, self.position, encoding, finished)? {
                return Ok(false);
            }
            self.emitted = true;
            self.position = self.position.after(part, encoding);
            if !finished {
                return Ok(true);
            }
            self.skip_lf = text.as_bytes()[end] == b'\r';
            self.position.byte += byte_len(encoding, "\n", 1);
            self.position.column = 0;
            self.position.line += 1;
            text = &text[end + 1..];
        }
        Ok(true)
    }
}

/// A seek bookmark always points to a complete decoded scalar. Stateful HTML encodings
/// re-enter the decoder at the beginning, preserving its original shift state.
pub(super) fn walk(
    path: &Path,
    html: bool,
    seek: Option<(Position, Charset)>,
    mut visit: impl FnMut(&str, Position, Charset, bool) -> Result<bool>,
) -> Result<()> {
    let mut file = File::open(path).context("read: could not open text source")?;
    let size = file.metadata()?.len();
    let (mut encoding, mut position) = match seek {
        Some((position, encoding)) => (encoding, position),
        None => {
            let (encoding, byte) = initial_charset(&mut file, html)?;
            (
                encoding,
                Position {
                    byte,
                    ..Default::default()
                },
            )
        }
    };
    ensure!(
        position.byte <= size,
        "location: position is outside the source file"
    );
    file.seek(SeekFrom::Start(position.byte))?;
    let mut lines = Lines {
        position,
        skip_lf: false,
        emitted: false,
    };
    let capacity = size.saturating_sub(position.byte).clamp(1, BUFFER as u64) as usize;
    let mut raw = vec![0; capacity];
    let mut decoded = String::new();
    let mut decoder = encoding.map(|e| e.new_decoder_without_bom_handling());
    loop {
        let n = file
            .read(&mut raw)
            .context("read: could not read text source")?;
        let mut input = &raw[..n];
        if encoding.is_none() {
            if forbidden(input, html) {
                return Err(NotText.into());
            }
            if input.is_ascii() {
                if n == 0 {
                    break;
                }
                if !lines.chunk(std::str::from_utf8(input)?, None, &mut visit)? {
                    return Ok(());
                }
                continue;
            }
            let selected = unmarked_charset(input, &mut file, html)?;
            encoding = Some(selected);
            decoder = Some(selected.new_decoder_without_bom_handling());
        }
        // These Windows-1252 bytes map to control characters, not printable letters. They
        // frequently occur in binary files that begin with a superficially textual header.
        if encoding == Some(WINDOWS_1252) && !html && windows_controls(input) {
            return Err(NotText.into());
        }
        let decoder = decoder.as_mut().expect("explicit encoding has a decoder");
        if decoded.capacity() == 0 {
            decoded.reserve(3 * capacity + 16);
        }
        loop {
            decoded.clear();
            let (status, consumed) =
                decoder.decode_to_string_without_replacement(input, &mut decoded, n == 0);
            if matches!(status, DecoderResult::Malformed(_, _))
                || forbidden(decoded.as_bytes(), html)
            {
                return Err(NotText.into());
            }
            if !lines.chunk(&decoded, encoding, &mut visit)? {
                return Ok(());
            }
            input = &input[consumed..];
            if status == DecoderResult::InputEmpty {
                break;
            }
        }
        if n == 0 {
            break;
        }
    }
    position = lines.position;
    if position.column > 0 || !lines.emitted {
        visit("", position, encoding, true)?;
    }
    Ok(())
}

pub(super) fn utf16_left(text: &str, count: usize) -> usize {
    if text.is_ascii() {
        return count.min(text.len());
    }
    let mut taken = 0;
    for (byte, ch) in text.char_indices() {
        taken += ch.len_utf16();
        if taken > count {
            return byte;
        }
    }
    text.len()
}

impl Position {
    pub(super) fn after(self, text: &str, encoding: Charset) -> Self {
        let count = units(text);
        Self {
            byte: self.byte + byte_len(encoding, text, count),
            column: self.column + count,
            ..self
        }
    }
}
