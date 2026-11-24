// SPDX-License-Identifier: GPL-3.0-or-later
//! Readers whose natural locations are source lines, document blocks, or PDF pages.

mod html;
mod markup;
mod pdf;

use crate::{Block, Document, MAX_FILE_BYTES, MAX_TEXT_BYTES};
use anyhow::{bail, ensure, Result};
use serde_json::json;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

// Count as well as byte ceilings matter for files made up of empty lines.
const MAX_BLOCKS: usize = 100_000;

pub(crate) fn is_html(extension: &str) -> bool {
    matches!(extension, "html" | "htm" | "xhtml")
}

pub fn extract(path: &Path, extension: &str, mode: &str) -> Result<Option<Document>> {
    if extension == "pdf" {
        return pdf::read(path).map(Some);
    }
    if is_html(extension) && mode == "document" {
        return html::read(File::open(path)?).map(Some);
    }
    let kind = match extension {
        ext if is_html(ext) => "html",
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" => "markdown",
        _ => "text",
    };
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE_BYTES,
        "too_large: file exceeds input limit"
    );
    let mut reader = BufReader::new(file);
    if kind == "text" && extension != "txt" {
        let first = reader.fill_buf()?;
        // Skip common binaries after one buffered read when walking a mixed folder.
        // A multibyte UTF-8 scalar may straddle the buffer's last byte.
        let utf16 = first.starts_with(&[0xff, 0xfe]) || first.starts_with(&[0xfe, 0xff]);
        if !utf16
            && (has_binary_controls(first)
                || std::str::from_utf8(first).is_err_and(|error| error.error_len().is_some()))
        {
            return Ok(None);
        }
    }
    let mut bytes = Vec::new();
    reader.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "too_large: file exceeds input limit"
    );
    let decoded = if kind == "html" {
        decode_html(&bytes)?
    } else {
        decode_text(&bytes)?
    };
    let Some(text) = decoded else {
        if kind != "text" || extension == "txt" {
            bail!(
                "unsupported_encoding: file does not contain readable text in a supported encoding"
            );
        }
        return Ok(None);
    };
    let blocks = if kind == "text" || mode == "source" {
        source_lines(&text)?
    } else {
        markup::project(&text, kind == "markdown")?
    };
    Ok(Some(Document {
        kind: kind.into(),
        blocks,
    }))
}

fn decode_html(bytes: &[u8]) -> Result<Option<std::borrow::Cow<'_, str>>> {
    use encoding_rs::{DecoderResult, Encoding, UTF_16BE, UTF_16LE, UTF_8};
    if Encoding::for_bom(bytes).is_some() || bytes.is_ascii() {
        return decode_text(bytes);
    }
    let Some(encoding) = markup::declared_encoding(bytes) else {
        return decode_text(bytes);
    };
    // A UTF-16 HTML label without a UTF-16 BOM denotes UTF-8 under HTML's encoding rules.
    if encoding == UTF_8 || encoding == UTF_16BE || encoding == UTF_16LE {
        return decode_text(bytes);
    }
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = decoder
        .max_utf8_buffer_length_without_replacement(bytes.len())
        .unwrap_or(MAX_TEXT_BYTES)
        .min(MAX_TEXT_BYTES);
    let mut output = String::with_capacity(capacity);
    match decoder
        .decode_to_string_without_replacement(bytes, &mut output, true)
        .0
    {
        DecoderResult::InputEmpty if !has_binary_controls(output.as_bytes()) => {
            Ok(Some(output.into()))
        }
        DecoderResult::OutputFull => bail!("too_large: decoded HTML exceeds limit"),
        _ => Ok(None),
    }
}

fn decode_text(bytes: &[u8]) -> Result<Option<std::borrow::Cow<'_, str>>> {
    use std::borrow::Cow;
    let little_endian = bytes.starts_with(&[0xff, 0xfe]);
    let text = if little_endian || bytes.starts_with(&[0xfe, 0xff]) {
        let bytes = &bytes[2..];
        if bytes.len() % 2 != 0 {
            return Ok(None);
        }
        let units = bytes.chunks_exact(2).map(|pair| {
            let raw = [pair[0], pair[1]];
            if little_endian {
                u16::from_le_bytes(raw)
            } else {
                u16::from_be_bytes(raw)
            }
        });
        let mut output = String::new();
        for scalar in char::decode_utf16(units) {
            let Ok(scalar) = scalar else {
                return Ok(None);
            };
            output.push(scalar);
            ensure!(
                output.len() <= MAX_TEXT_BYTES,
                "too_large: decoded text exceeds limit"
            );
        }
        Cow::Owned(output)
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let Ok(utf8) = std::str::from_utf8(bytes) else {
            return Ok(None);
        };
        ensure!(
            utf8.len() <= MAX_TEXT_BYTES,
            "too_large: decoded text exceeds limit"
        );
        Cow::Borrowed(utf8)
    };
    // ZIPs, binaries filled with NULs, and small binary files can be valid UTF-8.
    if has_binary_controls(text.as_bytes()) {
        return Ok(None);
    }
    Ok(Some(text))
}

fn has_binary_controls(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .copied()
        .any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c))
}

fn source_lines(text: &str) -> Result<Vec<Block>> {
    let mut blocks = Vec::new();
    // Keep leading spaces, trailing spaces, and tabs; recognize CRLF, LF, and CR.
    let mut rest = text;
    while !rest.is_empty() || blocks.is_empty() {
        ensure!(blocks.len() < MAX_BLOCKS, "too_large: too many text lines");
        let index = blocks.len();
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let line = &rest[..end];
        blocks.push(Block::new(
            line,
            json!({"index":index, "line":index+1}),
            format!("Line {}", index + 1),
        ));
        if end == rest.len() {
            break;
        }
        let terminator = if rest[end..].starts_with("\r\n") {
            2
        } else {
            1
        };
        rest = &rest[end + terminator..];
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_preserves_whitespace_and_physical_lines() {
        let blocks = source_lines("  one  \r\n\t二\r\rthree\n").unwrap();
        let text: Vec<_> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(text, ["  one  ", "\t二", "", "three"]);
        assert_eq!(blocks[2].location["line"], 3);
    }

    #[test]
    fn decodes_bom_and_rejects_incomplete_or_binary_content() {
        assert_eq!(
            decode_text(b"\xef\xbb\xbfabc").unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(
            decode_text(&[0xff, 0xfe, 61, 216, 0, 222])
                .unwrap()
                .as_deref(),
            Some("😀")
        );
        assert_eq!(
            decode_text(&[0xfe, 0xff, 0, 241]).unwrap().as_deref(),
            Some("ñ")
        );
        assert!(decode_text(&[0xff, 0xfe, 0]).unwrap().is_none());
        assert!(decode_text(b"abc\0binary").unwrap().is_none());
        assert!(decode_text(&[0xff, 0x7f]).unwrap().is_none());
    }

    #[test]
    fn recognizes_both_html_charset_declarations() {
        for declaration in [
            "<meta charset='iso-8859-1'>",
            "<meta content='text/html; charset=windows-1252' http-equiv=Content-Type>",
        ] {
            let mut bytes = declaration.as_bytes().to_vec();
            bytes.extend_from_slice(b"<p>ma\xf1ana</p>");
            let decoded = decode_html(&bytes).unwrap().unwrap();
            assert_eq!(markup::project(&decoded, false).unwrap()[0].text, "mañana");
        }
    }
}
