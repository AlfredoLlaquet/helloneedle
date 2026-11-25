// SPDX-License-Identifier: GPL-3.0-or-later
//! Readers whose natural locations are source lines, document blocks, or PDF pages.

mod html;
mod markup;
pub(crate) mod pdf;
pub(crate) mod text_stream;

use crate::{Document, MAX_FILE_BYTES, MAX_TEXT_BYTES};
use anyhow::{ensure, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

// Count as well as byte ceilings matter for files made up of empty lines.
const MAX_BLOCKS: usize = 100_000;

pub(crate) fn read_html(path: &Path) -> Result<Document> {
    html::read(File::open(path)?)
}

pub(crate) fn read_markdown(path: &Path) -> Result<Document> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE_BYTES,
        "too_large: Markdown exceeds input limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "too_large: Markdown exceeds input limit"
    );
    let text = decode_text(&bytes)?
        .ok_or_else(|| anyhow::anyhow!("unsupported_encoding: unreadable Markdown text"))?;
    Ok(Document {
        kind: "markdown".into(),
        blocks: markup::project(&text, true)?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let document = html::read(bytes.as_slice()).unwrap();
            assert_eq!(document.blocks[0].text, "mañana");
        }
    }
}
