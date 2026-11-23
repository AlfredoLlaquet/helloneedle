// SPDX-License-Identifier: GPL-3.0-or-later
//! The main story in Word 97–2003 binary documents, following [MS-DOC] §§2.3.1, 2.4.1, 2.4.6.2.
//! Only current piece-table addresses are read. Fast-save remnants and other stories stay outside the text.

use super::super::{paragraph_location, Output};
use crate::{MAX_FILE_BYTES, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, ensure, Result};
use cfb::CompoundFile;
use std::{fs::File, io::Read};

mod properties;
#[cfg(test)]
mod tests;
use properties::{character_runs, grpprl, piece_properties, Visibility};

// Bound index allocations and the combined work of repeated references independently of file length.
const MAX_RUNS: usize = 500_000;

fn bytes(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    start
        .checked_add(len)
        .and_then(|end| data.get(start..end))
        .ok_or_else(|| anyhow!("corrupt_document: binary Word offset outside stream"))
}
fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, at, 2)?.try_into()?))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, at, 4)?.try_into()?))
}

fn stream(cfb: &mut CompoundFile<File>, name: &str, available: u64) -> Result<Vec<u8>> {
    let stream = cfb
        .open_stream(name)
        .map_err(|_| anyhow!("corrupt_document: missing or invalid {name} stream"))?;
    let len = stream.len();
    ensure!(
        len <= available,
        "resource_limit: compound document streams"
    );
    let mut data = Vec::new();
    stream
        .take(len + 1)
        .read_to_end(&mut data)
        .map_err(|_| anyhow!("corrupt_document: invalid {name} stream"))?;
    ensure!(
        data.len() as u64 == len,
        "corrupt_document: incomplete {name} stream"
    );
    Ok(data)
}

/// A false result selects the older Word decoder. Recognized modern files keep their precise errors.
pub(super) fn read(file: File, out: &mut Output) -> Result<bool> {
    let mut cfb = CompoundFile::open(file)
        .map_err(|_| anyhow!("corrupt_document: invalid compound document"))?;
    if cfb.is_stream("/EncryptionInfo") && cfb.is_stream("/EncryptedPackage") {
        bail!("encrypted_document: encrypted Office package");
    }
    ensure!(
        cfb.is_stream("/WordDocument"),
        "unsupported_document: compound file lacks a Word document"
    );
    let word = stream(&mut cfb, "/WordDocument", MAX_FILE_BYTES)?;
    let ident = u16_at(&word, 0)?;
    let version = u16_at(&word, 2)?;
    if matches!(ident, 0xa5db | 0xa5dc) || (ident == 0xa5ec && version < 0x00c1) {
        return Ok(false);
    }
    ensure!(
        ident == 0xa5ec && matches!(version, 0x00c1 | 0x00d9 | 0x0101 | 0x010c | 0x0112),
        "unsupported_document: unrecognized binary Word version"
    );
    let flags = u16_at(&word, 10)?;
    ensure!(flags & 0x8100 == 0, "encrypted_document: binary Word");
    let table_name = if flags & 0x0200 != 0 {
        "/1Table"
    } else {
        "/0Table"
    };
    let table = stream(&mut cfb, table_name, MAX_FILE_BYTES - word.len() as u64)?;
    project(&word, &table, out)?;
    Ok(true)
}

// FibRgFcLcb97 lists a stream offset and byte count for each indexed structure.
fn table_part<'a>(pairs: &[u8], index: usize, table: &'a [u8]) -> Result<&'a [u8]> {
    let entry = bytes(pairs, index * 8, 8)?;
    let size = u32_at(entry, 4)? as usize;
    if size == 0 {
        return Ok(&[]);
    }
    bytes(table, u32_at(entry, 0)? as usize, size)
}

fn project(word: &[u8], table: &[u8], out: &mut Output) -> Result<()> {
    let lw = 34 + usize::from(u16_at(word, 32)?) * 2;
    let lw_size = usize::from(u16_at(word, lw)?) * 4;
    let longs = bytes(word, lw + 2, lw_size)?;
    let main_chars = u32_at(longs, 12)? as usize;
    ensure!(
        main_chars <= MAX_TEXT_BYTES,
        "resource_limit: binary Word character count"
    );
    let fc = lw + 2 + lw_size;
    let pairs = bytes(word, fc + 2, usize::from(u16_at(word, fc)?) * 8)?;
    let clx = table_part(pairs, 33, table)?;
    let chpx = table_part(pairs, 12, table)?;
    let runs = character_runs(chpx, word)?;
    let mut pos = 0;
    let mut groups = Vec::new();
    while clx.get(pos) == Some(&1) {
        ensure!(
            groups.len() < 32768,
            "resource_limit: binary Word property groups"
        );
        let size = usize::from(u16_at(clx, pos + 1)?);
        groups.push(grpprl(bytes(clx, pos + 3, size)?)?);
        pos += 3 + size;
    }
    ensure!(
        clx.get(pos) == Some(&2),
        "corrupt_document: missing binary Word piece table"
    );
    let piece_table = bytes(clx, pos + 5, u32_at(clx, pos + 1)? as usize)?;
    ensure!(
        piece_table.len() >= 4 && (piece_table.len() - 4) % 12 == 0,
        "corrupt_document: invalid binary Word piece table"
    );
    let count = (piece_table.len() - 4) / 12;
    ensure!(count <= MAX_RUNS, "resource_limit: binary Word piece count");
    ensure!(
        u32_at(piece_table, 0)? == 0 && u32_at(piece_table, count * 4)? as usize >= main_chars,
        "corrupt_document: binary Word piece table misses the main story"
    );
    let mut text = BodyText::new(out);
    let mut segments = 0;
    for index in 0..count {
        let cp = u32_at(piece_table, index * 4)? as usize;
        let next = u32_at(piece_table, (index + 1) * 4)? as usize;
        ensure!(
            cp <= next,
            "corrupt_document: out-of-order binary Word pieces"
        );
        if cp >= main_chars {
            continue;
        }
        let pcd = bytes(piece_table, (count + 1) * 4 + index * 8, 8)?;
        let fc = u32_at(pcd, 2)?;
        let compressed = fc & (1 << 30) != 0;
        let encoded = (fc & 0x3fff_ffff) as usize;
        ensure!(
            !compressed || encoded % 2 == 0,
            "corrupt_document: invalid compressed Word offset"
        );
        let width = if compressed { 1 } else { 2 };
        let mut start = if compressed { encoded / 2 } else { encoded };
        let end = start
            .checked_add((next.min(main_chars) - cp) * width)
            .ok_or_else(|| anyhow!("corrupt_document: binary Word piece range"))?;
        bytes(word, start, end - start)?;
        let piece = piece_properties(u16_at(pcd, 6)?, &groups)?;
        let mut run = runs.partition_point(|r| r.end <= start);
        while start < end {
            segments += 1;
            ensure!(
                segments <= MAX_RUNS * 2,
                "resource_limit: binary Word formatting intersections"
            );
            let (until, base) = match runs.get(run) {
                Some(r) if r.start <= start => {
                    run += 1;
                    (end.min(r.end), r.visibility)
                }
                Some(r) => (end.min(r.start), Visibility::default()),
                None => (end, Visibility::default()),
            };
            ensure!(
                (until - start) % width == 0,
                "corrupt_document: split binary Word character"
            );
            let state = piece.over(base);
            if !state.deleted() {
                for chunk in word[start..until].chunks_exact(width) {
                    let unit = if compressed {
                        uncompress(chunk[0])
                    } else {
                        u16::from_le_bytes([chunk[0], chunk[1]])
                    };
                    text.unit(unit, state.hidden())?;
                }
            }
            start = until;
        }
    }
    text.finish()
}

// The file format specifies a subset of the Windows-1252 substitutions. In particular
// 0x80, 0x8e, and 0x9e retain their byte values, unlike general Windows-1252 decoding.
fn uncompress(b: u8) -> u16 {
    const EXCEPTIONS: [u16; 32] = [
        0x80, 0x81, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
        0x0152, 0x8d, 0x8e, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
        0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x9d, 0x9e, 0x0178,
    ];
    if (0x80..0xa0).contains(&b) {
        EXCEPTIONS[(b - 0x80) as usize]
    } else {
        u16::from(b)
    }
}

struct BodyText<'a> {
    out: &'a mut Output,
    paragraph: String,
    number: usize,
    high: Option<u16>,
    fields: Vec<bool>,
    instructions: usize,
}
impl<'a> BodyText<'a> {
    fn new(out: &'a mut Output) -> Self {
        Self {
            out,
            paragraph: String::new(),
            number: 0,
            high: None,
            fields: Vec::new(),
            instructions: 0,
        }
    }
    fn end_paragraph(&mut self) -> Result<()> {
        self.number += 1;
        if !self.paragraph.is_empty() {
            let (location, label) = paragraph_location(self.number, None, None);
            self.out
                .push(std::mem::take(&mut self.paragraph), location, label, None)?;
        }
        Ok(())
    }
    fn unit(&mut self, u: u16, hidden: bool) -> Result<()> {
        match u {
            0x13 => {
                ensure!(
                    self.fields.len() < 128,
                    "resource_limit: binary Word field nesting"
                );
                self.fields.push(false);
                self.instructions += 1;
                return Ok(());
            }
            0x14 => {
                if let Some(result) = self.fields.last_mut() {
                    if !*result {
                        *result = true;
                        self.instructions -= 1;
                    }
                }
                return Ok(());
            }
            0x15 => {
                if self.fields.pop() == Some(false) {
                    self.instructions -= 1;
                }
                return Ok(());
            }
            _ => {}
        }
        if hidden || self.instructions > 0 {
            return Ok(());
        }
        if let Some(high) = self.high.take() {
            ensure!(
                (0xdc00..=0xdfff).contains(&u),
                "corrupt_document: incomplete binary Word Unicode character"
            );
            let ch = 0x10000 + (u32::from(high - 0xd800) << 10) + u32::from(u - 0xdc00);
            self.paragraph.push(char::from_u32(ch).unwrap());
        } else {
            match u {
                0x07 | 0x0c | 0x0d => self.end_paragraph()?,
                0x09 => self.paragraph.push('\t'),
                0x0b | 0x0e => self.paragraph.push('\n'),
                0x1e => self.paragraph.push('\u{2011}'),
                0x1f => self.paragraph.push('\u{ad}'),
                0x00..=0x1f | 0x7f..=0x9f => {}
                0xd800..=0xdbff => self.high = Some(u),
                _ => self
                    .paragraph
                    .push(char::from_u32(u32::from(u)).ok_or_else(|| {
                        anyhow!("corrupt_document: invalid binary Word Unicode character")
                    })?),
            }
        }
        ensure!(
            self.paragraph.len() <= MAX_TEXT_BYTES - self.out.bytes,
            "resource_limit: extracted binary Word text"
        );
        Ok(())
    }
    fn finish(mut self) -> Result<()> {
        ensure!(
            self.high.is_none() && self.fields.is_empty(),
            "corrupt_document: incomplete binary Word text or field"
        );
        self.end_paragraph()
    }
}
