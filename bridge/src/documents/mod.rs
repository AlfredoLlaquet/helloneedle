// SPDX-License-Identifier: GPL-3.0-or-later
//! Readers whose natural locations are source lines, document blocks, or PDF pages.

mod ansi;
mod encoding;
mod html;
mod markdown;
mod markup;
pub(crate) use markdown::{preview_markdown, visit_markdown};
pub(crate) mod pdf;
pub(crate) mod text_stream;

use crate::Document;
use anyhow::Result;
use std::fs::File;
use std::path::Path;

pub(crate) fn read_html(path: &Path) -> Result<Document> {
    html::read(File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
