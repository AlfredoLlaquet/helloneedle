// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict encoding selection for flat documents and materialized package XML.
use anyhow::{anyhow, ensure, Result};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use quick_xml::{events::Event, Reader};

/// The declaration is ASCII in all supported byte-oriented encodings. A
/// UTF-16 stream has already been identified and decoded before this step.
fn declaration(bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    if !bytes.starts_with(b"<?xml") || !bytes.get(5).is_some_and(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let malformed = || anyhow!("corrupt_document: invalid XML declaration");
    let mut reader = Reader::from_reader(bytes);
    let Event::Decl(decl) = reader.read_event().map_err(|_| malformed())? else {
        return Err(malformed());
    };
    let value = decl.encoding().transpose().map_err(|_| malformed())?;
    Ok(value.map(|v| v.into_owned()))
}

fn decode_as(bytes: &[u8], encoding: &'static Encoding) -> Result<String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| text.into_owned())
        .ok_or_else(|| anyhow!("corrupt_document: invalid {} XML", encoding.name()))
}

pub(super) fn decode(bytes: &[u8]) -> Result<String> {
    let (signature, skip) = Encoding::for_bom(bytes).map_or_else(
        || {
            let encoding = if bytes.starts_with(&[b'<', 0]) {
                Some(UTF_16LE)
            } else if bytes.starts_with(&[0, b'<']) {
                Some(UTF_16BE)
            } else {
                None
            };
            (encoding, 0)
        },
        |(encoding, skip)| (Some(encoding), skip),
    );
    let bytes = &bytes[skip..];
    if let Some(encoding) = signature.filter(|&e| e == UTF_16LE || e == UTF_16BE) {
        let text = decode_as(bytes, encoding)?;
        if let Some(label) = declaration(text.as_bytes())? {
            ensure!(
                label.eq_ignore_ascii_case(b"UTF-16")
                    || label.eq_ignore_ascii_case(encoding.name().as_bytes()),
                "corrupt_document: XML encoding declaration conflicts with UTF-16 bytes"
            );
        }
        return Ok(text);
    }
    let Some(mut label) = declaration(bytes)? else {
        return decode_as(bytes, UTF_8);
    };
    label.make_ascii_lowercase();
    // XML encoding names follow their declared character sets. The HTML
    // decoder's aliases for US-ASCII and ISO-8859-1 would instead use 1252.
    if matches!(label.as_slice(), b"us-ascii" | b"ascii") {
        ensure!(
            signature.is_none() && bytes.is_ascii(),
            "corrupt_document: invalid US-ASCII XML"
        );
        return decode_as(bytes, UTF_8);
    }
    if matches!(label.as_slice(), b"iso-8859-1" | b"iso_8859-1" | b"latin1") {
        ensure!(
            signature.is_none(),
            "corrupt_document: XML declaration conflicts with UTF-8 BOM"
        );
        return Ok(bytes.iter().copied().map(char::from).collect());
    }
    // Keep faithful aliases such as CP1252. Other web aliases can mean
    // something else in XML (for example, ISO-8859-9 versus Windows-1254).
    let encoding = Encoding::for_label_no_replacement(&label)
        .filter(|e| {
            let name = e.name().as_bytes();
            label.eq_ignore_ascii_case(name)
                || (&label == b"utf8" && *e == UTF_8)
                || label
                    .strip_prefix(b"cp")
                    .zip(name.strip_prefix(b"windows-"))
                    .is_some_and(|(source, canonical)| source == canonical)
        })
        .ok_or_else(|| anyhow!("unsupported_encoding: unsupported XML encoding declaration"))?;
    ensure!(
        signature.is_none_or(|e| e == encoding) && encoding != UTF_16BE && encoding != UTF_16LE,
        "corrupt_document: XML declaration conflicts with its byte encoding"
    );
    decode_as(bytes, encoding)
}
