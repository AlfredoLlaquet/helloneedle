// SPDX-License-Identifier: GPL-3.0-or-later
//! Readers whose natural locations are source lines, document blocks, or PDF pages.

mod encoding;
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
    use encoding_rs::{DecoderResult, UTF_8, WINDOWS_1252};
    use std::borrow::Cow;

    let (marked, skip) = encoding::marked(bytes, false);
    let bytes = &bytes[skip..];
    let encoding = marked.unwrap_or(UTF_8);
    if encoding == UTF_8 {
        if let Ok(text) = std::str::from_utf8(bytes) {
            ensure!(
                text.len() <= MAX_TEXT_BYTES,
                "too_large: decoded text exceeds limit"
            );
            return Ok(
                (!encoding::binary_controls(text.as_bytes(), false)).then_some(Cow::Borrowed(text))
            );
        }
        if marked.is_some() || encoding::windows_controls(bytes) {
            return Ok(None);
        }
    }
    // The limit counts decoded UTF-8 bytes. Check it as the output grows,
    // including when a single-byte source expands to three bytes per character.
    let mut decoder = marked
        .unwrap_or(WINDOWS_1252)
        .new_decoder_without_bom_handling();
    let mut input = bytes;
    let mut output = String::new();
    let mut buffer = [0; 8192];
    loop {
        let (status, consumed, produced) =
            decoder.decode_to_utf8_without_replacement(input, &mut buffer, true);
        if matches!(status, DecoderResult::Malformed(_, _)) {
            return Ok(None);
        }
        ensure!(
            output.len() + produced <= MAX_TEXT_BYTES,
            "too_large: decoded text exceeds limit"
        );
        output.push_str(std::str::from_utf8(&buffer[..produced])?);
        input = &input[consumed..];
        if status == DecoderResult::InputEmpty {
            break;
        }
    }
    let text = Cow::Owned::<str>(output);
    // ZIPs, binaries filled with NULs, and small binary files can be valid UTF-8.
    if encoding::binary_controls(text.as_bytes(), false) {
        return Ok(None);
    }
    Ok(Some(text))
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
    fn markdown_uses_readable_windows_1252_only_when_unmarked() {
        assert_eq!(
            decode_text(b"**\x93Caf\xe9\x94**").unwrap().as_deref(),
            Some("**“Café”**")
        );
        assert_eq!(
            decode_text("niñez".as_bytes()).unwrap().as_deref(),
            Some("niñez")
        );
        assert!(decode_text(b"\xef\xbb\xbfNi\xf1ez").unwrap().is_none());
        assert!(decode_text(b"word\0\xf1").unwrap().is_none());
    }

    #[test]
    fn recognizes_both_html_charset_declarations() {
        for declaration in [
            "<meta charset='iso-8859-1'>",
            "<meta content='text/html; charset=windows-1252' http-equiv=Content-Type>",
        ] {
            let mut bytes = declaration.as_bytes().to_vec();
            bytes.extend_from_slice(b"<p>ma\xf1ana</p>");
            let document = html::read(std::io::Cursor::new(bytes)).unwrap();
            assert_eq!(document.blocks[0].text, "mañana");
        }
    }
}
