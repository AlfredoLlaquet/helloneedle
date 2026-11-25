// SPDX-License-Identifier: GPL-3.0-or-later
//! Encoding markers shared by direct-text, HTML and Markdown readers.
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

/// No marker leaves the choice between strict UTF-8 and readable Windows-1252
/// open. A BOM overrides an HTML declaration. HTML permits UTF-16 only by BOM.
pub(super) fn marked(head: &[u8], html: bool) -> (Option<&'static Encoding>, usize) {
    if let Some((encoding, bytes)) = Encoding::for_bom(head) {
        return (Some(encoding), bytes);
    }
    let declared = if html {
        super::markup::declared_encoding(head).map(|encoding| {
            if encoding == UTF_16BE || encoding == UTF_16LE {
                UTF_8
            } else {
                encoding
            }
        })
    } else {
        None
    };
    (declared, 0)
}

/// These bytes represent DEL or unassigned C1 controls in Windows-1252.
pub(super) fn windows_controls(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|b| matches!(b, 0x7f | 0x81 | 0x8d | 0x8f | 0x90 | 0x9d))
}

/// Word plain-text exports can contain one additional control. Its neighbors
/// must also be verified by the text reader before it is accepted as a hyphen.
pub(super) fn binary_controls(bytes: &[u8], word_export: bool) -> bool {
    bytes.iter().any(|&b| {
        // ESC and BEL are validated as complete ANSI sequences after decoding.
        b < 0x20
            && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b | 7)
            && !(word_export && b == 0x1e)
    })
}
