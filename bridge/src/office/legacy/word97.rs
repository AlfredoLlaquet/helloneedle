// SPDX-License-Identifier: GPL-3.0-or-later
//! The main story in Word 97–2003 binary documents, following [MS-DOC] §§2.3.1, 2.4.1, 2.4.6.2.
//! Only current piece-table addresses are read. Fast-save remnants and other stories stay outside the text.

use super::super::{paragraph_location, Output};
use crate::MAX_TEXT_BYTES;
use anyhow::{anyhow, bail, ensure, Result};
use cfb::CompoundFile;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

mod compound;
mod properties;
mod styles;
#[cfg(test)]
mod tests;
use properties::{character_runs, grpprl, piece_properties, Properties, Visibility};
use styles::{paragraphs, Styles};

// Only live format indexes need a storage budget. Total text processed is
// independent of those indexes (one piece can cover millions of paragraphs).
fn index_room<T>(entries: &[T]) -> Result<()> {
    ensure!(
        entries.len() < MAX_TEXT_BYTES / std::mem::size_of::<T>(),
        "resource_limit: binary Word index working memory"
    );
    Ok(())
}

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

trait RandomRead: Read + Seek {}
impl<R: Read + Seek> RandomRead for R {}

/// Word stores its text, formatting pages and table parts at independent
/// offsets. Keep the compound streams seekable and request only active data.
struct Indexed<'a> {
    source: &'a mut dyn RandomRead,
    len: u64,
    position: u64,
}
impl<'a> Indexed<'a> {
    fn new(source: &'a mut dyn RandomRead) -> Result<Self> {
        let len = source.seek(SeekFrom::End(0))?;
        Ok(Self {
            source,
            len,
            position: len,
        })
    }
    fn fill(&mut self, offset: usize, target: &mut [u8]) -> Result<()> {
        ensure!(
            (offset as u64)
                .checked_add(target.len() as u64)
                .is_some_and(|end| end <= self.len),
            "corrupt_document: binary Word offset outside stream"
        );
        if self.position != offset as u64 {
            self.position = self.source.seek(SeekFrom::Start(offset as u64))?;
        }
        for chunk in target.chunks_mut(64 * 1024) {
            crate::search::checkpoint()?;
            self.source
                .read_exact(chunk)
                .map_err(|_| anyhow!("corrupt_document: incomplete binary Word stream"))?;
            self.position += chunk.len() as u64;
        }
        Ok(())
    }
    fn word(&mut self, at: usize) -> Result<u16> {
        let mut data = [0; 2];
        self.fill(at, &mut data)?;
        Ok(u16::from_le_bytes(data))
    }
    fn part(&mut self, at: usize, size: usize) -> Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        ensure!(
            size <= MAX_TEXT_BYTES,
            "resource_limit: binary Word index working memory"
        );
        ensure!(
            (at as u64)
                .checked_add(size as u64)
                .is_some_and(|end| end <= self.len),
            "corrupt_document: binary Word index outside stream"
        );
        let mut data = vec![0; size];
        self.fill(at, &mut data)?;
        Ok(data)
    }
}

/// A false result selects the older Word decoder. Recognized modern files keep their precise errors.
pub(super) fn read(mut file: File, out: &mut Output) -> Result<bool> {
    compound::preflight(&mut file)?;
    let mut cfb = CompoundFile::open(file)
        .map_err(|_| anyhow!("corrupt_document: invalid compound document"))?;
    if cfb.is_stream("/EncryptionInfo") && cfb.is_stream("/EncryptedPackage") {
        bail!("encrypted_document: encrypted Office package");
    }
    ensure!(
        cfb.is_stream("/WordDocument"),
        "unsupported_document: compound file lacks a Word document"
    );
    let mut main_stream = cfb
        .open_stream("/WordDocument")
        .map_err(|_| anyhow!("corrupt_document: invalid WordDocument stream"))?;
    let mut word = Indexed::new(&mut main_stream)?;
    let ident = word.word(0)?;
    let version = word.word(2)?;
    if matches!(ident, 0xa5db | 0xa5dc) || (ident == 0xa5ec && version < 0x00c1) {
        return Ok(false);
    }
    ensure!(
        ident == 0xa5ec && matches!(version, 0x00c1 | 0x00d9 | 0x0101 | 0x010c | 0x0112),
        "unsupported_document: unrecognized binary Word version"
    );
    let flags = word.word(10)?;
    ensure!(flags & 0x8100 == 0, "encrypted_document: binary Word");
    let table_name = if flags & 0x0200 != 0 {
        "/1Table"
    } else {
        "/0Table"
    };
    let mut table_stream = cfb
        .open_stream(table_name)
        .map_err(|_| anyhow!("corrupt_document: missing or invalid {table_name} stream"))?;
    project_streams(&mut word, &mut Indexed::new(&mut table_stream)?, out)?;
    Ok(true)
}

// FibRgFcLcb97 lists a stream offset and byte count for each indexed structure.
fn table_part(pairs: &[u8], index: usize, table: &mut Indexed) -> Result<Vec<u8>> {
    let entry = bytes(pairs, index * 8, 8)?;
    table.part(u32_at(entry, 0)? as usize, u32_at(entry, 4)? as usize)
}

#[cfg(test)]
fn project(word: &[u8], table: &[u8], out: &mut Output) -> Result<()> {
    project_streams(
        &mut Indexed::new(&mut std::io::Cursor::new(word))?,
        &mut Indexed::new(&mut std::io::Cursor::new(table))?,
        out,
    )
}

fn project_streams(word: &mut Indexed, table: &mut Indexed, out: &mut Output) -> Result<()> {
    let lw = 34 + usize::from(word.word(32)?) * 2;
    let lw_size = usize::from(word.word(lw)?) * 4;
    ensure!(
        lw_size >= 16,
        "corrupt_document: truncated Word long header"
    );
    let mut main_chars = [0; 4];
    word.fill(lw + 2 + 12, &mut main_chars)?;
    let main_chars = u32::from_le_bytes(main_chars) as usize;
    let fc = lw + 2 + lw_size;
    let pair_size = usize::from(word.word(fc)?) * 8;
    let pairs = word.part(fc + 2, pair_size)?;
    let clx = table_part(&pairs, 33, table)?;
    let runs = character_runs(&table_part(&pairs, 12, table)?, word)?;
    let mut pos = 0;
    let mut groups = Vec::new();
    while clx.get(pos) == Some(&1) {
        ensure!(
            groups.len() < 32768,
            "resource_limit: binary Word property groups"
        );
        let size = usize::from(u16_at(&clx, pos + 1)?);
        groups.push(grpprl(bytes(&clx, pos + 3, size)?)?);
        pos += 3 + size;
    }
    ensure!(
        clx.get(pos) == Some(&2),
        "corrupt_document: missing binary Word piece table"
    );
    let piece_table = bytes(&clx, pos + 5, u32_at(&clx, pos + 1)? as usize)?;
    ensure!(
        piece_table.len() >= 4 && (piece_table.len() - 4) % 12 == 0,
        "corrupt_document: invalid binary Word piece table"
    );
    let count = (piece_table.len() - 4) / 12;
    ensure!(
        u32_at(piece_table, 0)? == 0 && u32_at(piece_table, count * 4)? as usize >= main_chars,
        "corrupt_document: binary Word piece table misses the main story"
    );
    let pieces = pieces(piece_table, count, main_chars, word.len, &groups)?;
    drop(clx);
    let styles = Styles::read(&table_part(&pairs, 1, table)?)?;
    let paragraphs = paragraphs(&table_part(&pairs, 13, table)?, word, &pieces)?;
    let mut para_index = 0;
    let mut text = BodyText::new(out);
    let mut buffer = vec![0; 64 * 1024];
    let empty = Properties::default();
    for piece in &pieces {
        let mut start = piece.start;
        let mut run = runs.partition_point(|r| r.end <= start);
        while start < piece.end {
            crate::search::checkpoint()?;
            // Paragraph ends use logical character positions; character formatting
            // uses physical byte offsets. Both can divide this same piece.
            let cp = piece.cp + (start - piece.start) / piece.width;
            while paragraphs.get(para_index).is_some_and(|p| p.end_cp <= cp) {
                para_index += 1;
            }
            let (mut until, paragraph_id) = match paragraphs.get(para_index) {
                Some(p) => (
                    start + (p.end_cp - cp).min((piece.end - start) / piece.width) * piece.width,
                    p.style,
                ),
                None => (piece.end, 0),
            };
            while runs.get(run).is_some_and(|r| r.end <= start) {
                run += 1;
            }
            let direct = match runs.get(run) {
                Some(r) if r.start <= start => {
                    until = until.min(r.end);
                    &r.properties
                }
                Some(r) => {
                    until = until.min(r.start);
                    &empty
                }
                None => &empty,
            };
            ensure!(
                until > start && (until - start) % piece.width == 0,
                "corrupt_document: split binary Word character"
            );
            let paragraph = styles.flags(paragraph_id, 0)?;
            let mut state = Visibility::from_style(paragraph);
            direct.apply(&styles, paragraph, &mut state)?;
            piece.properties.apply(&styles, paragraph, &mut state)?;
            if !state.deleted() {
                let mut cursor = start;
                while cursor < until {
                    let n = buffer.len().min(until - cursor);
                    word.fill(cursor, &mut buffer[..n])?;
                    for chunk in buffer[..n].chunks_exact(piece.width) {
                        let unit = if piece.width == 1 {
                            uncompress(chunk[0])
                        } else {
                            u16::from_le_bytes([chunk[0], chunk[1]])
                        };
                        text.unit(unit, state.hidden())?;
                    }
                    cursor += n;
                }
            }
            start = until;
        }
    }
    text.finish()
}

struct Piece<'a> {
    cp: usize,
    start: usize,
    end: usize,
    width: usize,
    properties: std::borrow::Cow<'a, Properties>,
}

fn pieces<'a>(
    piece_table: &[u8],
    count: usize,
    main_chars: usize,
    word_len: u64,
    groups: &'a [Properties],
) -> Result<Vec<Piece<'a>>> {
    let mut pieces = Vec::new();
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
        let start = if compressed { encoded / 2 } else { encoded };
        let end = start
            .checked_add((next.min(main_chars) - cp) * width)
            .ok_or_else(|| anyhow!("corrupt_document: binary Word piece range"))?;
        ensure!(
            end as u64 <= word_len,
            "corrupt_document: binary Word piece outside stream"
        );
        let properties = piece_properties(u16_at(pcd, 6)?, groups)?;
        index_room(&pieces)?;
        pieces.push(Piece {
            cp,
            start,
            end,
            width,
            properties,
        });
    }
    Ok(pieces)
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

struct BodyText<'a, 'visit> {
    out: &'a mut Output<'visit>,
    paragraph: String,
    number: usize,
    high: Option<u16>,
    fields: Vec<bool>,
    instructions: usize,
}
impl<'a, 'visit> BodyText<'a, 'visit> {
    fn new(out: &'a mut Output<'visit>) -> Self {
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
            self.paragraph.len() <= MAX_TEXT_BYTES,
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
