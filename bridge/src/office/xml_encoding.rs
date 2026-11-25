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

#[derive(Clone, Copy)]
enum Charset {
    Text(&'static Encoding),
    Ascii,
    Latin1,
}

fn charset(label: Option<Vec<u8>>, signature: Option<&'static Encoding>) -> Result<Charset> {
    let Some(mut label) = label else {
        return Ok(Charset::Text(signature.unwrap_or(UTF_8)));
    };
    label.make_ascii_lowercase();
    if let Some(utf16) = signature.filter(|&e| e == UTF_16LE || e == UTF_16BE) {
        ensure!(
            label == b"utf-16" || label.eq_ignore_ascii_case(utf16.name().as_bytes()),
            "corrupt_document: XML encoding declaration conflicts with UTF-16 bytes"
        );
        return Ok(Charset::Text(utf16));
    }
    if matches!(
        label.as_slice(),
        b"us-ascii" | b"ascii" | b"iso-8859-1" | b"iso_8859-1" | b"latin1"
    ) {
        ensure!(
            signature.is_none(),
            "corrupt_document: XML encoding conflicts with BOM"
        );
        return Ok(if matches!(label.as_slice(), b"ascii" | b"us-ascii") {
            Charset::Ascii
        } else {
            Charset::Latin1
        });
    }
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
    Ok(Charset::Text(encoding))
}

use encoding_rs::{Decoder, DecoderResult};
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::ops::Range;

const BUFFER: usize = 64 * 1024;

/// All supported readers see strict UTF-8 after this adapter. Only the leading XML
/// declaration, the decoder's input/output buffers and an incomplete code point are retained.
pub(super) struct Input<R: Read> {
    source: BufReader<std::io::Chain<Cursor<Vec<u8>>, BufReader<R>>>,
    decoder: Option<Decoder>,
    charset: Charset,
    decoded: Box<[u8]>,
    available: Range<usize>,
    finished: bool,
}

impl<R: Read> Input<R> {
    pub fn new(source: R) -> Result<Self> {
        let mut source = BufReader::new(source);
        let mut head = Vec::new();
        source.by_ref().take(16).read_to_end(&mut head)?;
        let (signature, skip) = Encoding::for_bom(&head).map_or_else(
            || {
                let guessed = if head.starts_with(&[b'<', 0]) {
                    Some(UTF_16LE)
                } else if head.starts_with(&[0, b'<']) {
                    Some(UTF_16BE)
                } else {
                    None
                };
                (guessed, 0)
            },
            |(e, n)| (Some(e), n),
        );
        let utf16 = signature.filter(|&e| e == UTF_16LE || e == UTF_16BE);
        let ascii_unit = |data: &[u8], index: usize| {
            if let Some(encoding) = utf16 {
                let pair = data.get(index * 2..index * 2 + 2)?;
                let c = if encoding == UTF_16LE {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                };
                u8::try_from(c).ok()
            } else {
                data.get(index).copied()
            }
        };
        let mut label = None;
        let start = &head[skip..];
        if b"<?xml"
            .iter()
            .enumerate()
            .all(|(i, ch)| ascii_unit(start, i) == Some(*ch))
            && ascii_unit(start, 5).is_some_and(|c| c.is_ascii_whitespace())
        {
            let unit = if utf16.is_some() { 2 } else { 1 };
            let mut i = 5;
            loop {
                if skip + (i + 2) * unit > head.len() {
                    ensure!(
                        head.len() < super::MAX_TEXT_BYTES,
                        "resource_limit: XML declaration"
                    );
                    let pending = source.fill_buf()?;
                    ensure!(
                        !pending.is_empty(),
                        "corrupt_document: unfinished XML declaration"
                    );
                    let n = pending.len().min(8192);
                    head.extend_from_slice(&pending[..n]);
                    source.consume(n);
                } else if ascii_unit(&head[skip..], i) == Some(b'?')
                    && ascii_unit(&head[skip..], i + 1) == Some(b'>')
                {
                    let data = &head[skip..skip + (i + 2) * unit];
                    if let Some(encoding) = utf16 {
                        let decoded = encoding
                            .decode_without_bom_handling_and_without_replacement(data)
                            .ok_or_else(|| anyhow!("corrupt_document: invalid XML declaration"))?;
                        label = declaration(decoded.as_bytes())?;
                    } else {
                        label = declaration(data)?;
                    }
                    break;
                } else {
                    i += 1;
                }
            }
        }
        let charset = charset(label, signature)?;
        let decoder = match charset {
            Charset::Text(e) => Some(e.new_decoder_without_bom_handling()),
            _ => None,
        };
        let mut prefix = Cursor::new(head);
        prefix.set_position(skip as u64);
        Ok(Self {
            source: BufReader::with_capacity(BUFFER, prefix.chain(source)),
            decoder,
            charset,
            decoded: vec![0; BUFFER].into_boxed_slice(),
            available: 0..0,
            finished: false,
        })
    }
}

impl<R: Read> BufRead for Input<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.available.is_empty() && !self.finished {
            crate::search::checkpoint()?;
            let raw = self.source.fill_buf()?;
            let last = raw.is_empty();
            let (consumed, produced) = match self.charset {
                Charset::Text(_) => {
                    let (status, used, produced) = self
                        .decoder
                        .as_mut()
                        .unwrap()
                        .decode_to_utf8_without_replacement(raw, &mut self.decoded, last);
                    if matches!(status, DecoderResult::Malformed(_, _)) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "corrupt_document: invalid XML encoding",
                        ));
                    }
                    self.finished = last && status == DecoderResult::InputEmpty;
                    (used, produced)
                }
                Charset::Ascii => {
                    let n = raw.len().min(self.decoded.len());
                    if !raw[..n].is_ascii() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "corrupt_document: invalid US-ASCII XML",
                        ));
                    }
                    self.decoded[..n].copy_from_slice(&raw[..n]);
                    self.finished = last;
                    (n, n)
                }
                Charset::Latin1 => {
                    let n = raw.len().min(self.decoded.len() / 2);
                    let mut wrote = 0;
                    for &b in &raw[..n] {
                        wrote += char::from(b).encode_utf8(&mut self.decoded[wrote..]).len();
                    }
                    self.finished = last;
                    (n, wrote)
                }
            };
            self.source.consume(consumed);
            self.available = 0..produced;
        }
        Ok(&self.decoded[self.available.clone()])
    }
    fn consume(&mut self, count: usize) {
        self.available.start += count;
    }
}

impl<R: Read> Read for Input<R> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let data = self.fill_buf()?;
        let n = target.len().min(data.len());
        target[..n].copy_from_slice(&data[..n]);
        self.consume(n);
        Ok(n)
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<String> {
    let mut result = String::new();
    Input::new(bytes)?.read_to_string(&mut result)?;
    Ok(result)
}
