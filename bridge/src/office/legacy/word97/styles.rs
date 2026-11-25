// SPDX-License-Identifier: GPL-3.0-or-later
//! Word 97 style definitions and logical paragraph boundaries ([MS-DOC] 2.4.2, 2.4.6).
use super::properties::{grpprl, visit_fkp, Properties, Run};
use super::{bytes, u16_at, Piece, MAX_RUNS};
use anyhow::{anyhow, ensure, Result};

const NO_PARENT: u16 = 0x0fff;

/// Each visibility bit inherits, inverts, clears or sets its parent bit. Recording
/// the two endpoints lets a named character style be applied to any paragraph style.
#[derive(Clone, Copy)]
struct StyleFlags {
    when_clear: u8,
    when_set: u8,
    specified: u8,
}

impl StyleFlags {
    const INHERIT: Self = Self {
        when_clear: 0,
        when_set: u8::MAX,
        specified: 0,
    };

    fn apply(self, parent: u8) -> u8 {
        (self.when_clear & !parent) | (self.when_set & parent)
    }
}

pub(super) struct Styles(Vec<StyleFlags>);

impl Styles {
    pub fn read(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Ok(Self(Vec::new()));
        }
        let header_len = usize::from(u16_at(data, 0)?);
        let header = bytes(data, 2, header_len)?;
        let count = usize::from(u16_at(header, 0)?);
        let base_size = usize::from(u16_at(header, 2)?);
        ensure!(
            count <= 0xffe && base_size >= 10,
            "corrupt_document: invalid Word stylesheet"
        );
        let mut pos = 2 + header_len;
        let mut definitions = Vec::with_capacity(count);
        for _ in 0..count {
            let length = usize::from(u16_at(data, pos)?);
            let record = bytes(data, pos + 2, length)?;
            definitions.push(style_definition(record, base_size)?);
            pos += 2 + length;
        }
        // Resolve each parent chain once. A malformed cycle must not recurse indefinitely.
        let mut flags = vec![StyleFlags::INHERIT; count];
        let mut visited = vec![false; count];
        let mut done = vec![false; count];
        let mut chain = Vec::new();
        for id in 0..count {
            let mut parent = id;
            let mut inherited = loop {
                if parent == usize::from(NO_PARENT) {
                    break StyleFlags::INHERIT;
                }
                ensure!(
                    parent < count,
                    "corrupt_document: invalid Word parent style"
                );
                if done[parent] {
                    break flags[parent];
                }
                ensure!(
                    !visited[parent],
                    "corrupt_document: circular Word style inheritance"
                );
                visited[parent] = true;
                chain.push(parent);
                parent = usize::from(definitions[parent].0);
            };
            while let Some(index) = chain.pop() {
                let properties = &definitions[index].1;
                inherited = StyleFlags {
                    when_clear: properties.style_flags(inherited.when_clear),
                    when_set: properties.style_flags(inherited.when_set),
                    specified: inherited.specified | properties.style_mask(),
                };
                flags[index] = inherited;
                done[index] = true;
            }
        }
        Ok(Self(flags))
    }

    pub fn mask(&self, id: u16) -> u8 {
        self.0
            .get(usize::from(id))
            .map_or(0, |style| style.specified)
    }

    pub fn flags(&self, id: u16, base: u8) -> Result<u8> {
        if self.0.is_empty() || id == NO_PARENT {
            return Ok(base);
        }
        let flags = self
            .0
            .get(usize::from(id))
            .ok_or_else(|| anyhow!("corrupt_document: unknown Word style"))?;
        Ok(flags.apply(base))
    }
}

fn style_definition(record: &[u8], base_size: usize) -> Result<(u16, Properties)> {
    if record.is_empty() {
        return Ok((NO_PARENT, Properties::default()));
    }
    let kind_and_parent = u16_at(record, 2)?;
    let kind = kind_and_parent & 0xf;
    let parent = kind_and_parent >> 4;
    // Paragraph styles have a paragraph UPX followed by their character UPX. A
    // character style starts with the latter. Skip the UTF-16 name, including its NUL.
    if kind != 1 && kind != 2 {
        return Ok((NO_PARENT, Properties::default()));
    }
    let paragraph = kind == 1;
    let upx_count = u16_at(record, 4)? & 0xf;
    if upx_count <= u16::from(paragraph) {
        return Ok((parent, Properties::default()));
    }
    let name_chars = usize::from(u16_at(record, base_size)?);
    let mut pos = base_size + 2 + 2 * (name_chars + 1);
    if paragraph {
        let papx_size = usize::from(u16_at(record, pos)?);
        pos += 2 + papx_size + papx_size % 2;
    }
    let chpx_size = usize::from(u16_at(record, pos)?);
    let chpx = bytes(record, pos + 2, chpx_size)?;
    Ok((parent, grpprl(chpx)?))
}

pub(super) struct Paragraph {
    pub end_cp: usize,
    pub style: u16,
}

pub(super) fn paragraphs(bte: &[u8], word: &[u8], pieces: &[Piece<'_>]) -> Result<Vec<Paragraph>> {
    let mut physical = Vec::new();
    visit_fkp(bte, word, 13, |start, end, record| {
        let style = match record.split_first() {
            None => 0,
            Some((&cb, rest)) => {
                let (offset, size) = if cb == 0 {
                    (
                        1,
                        2 * usize::from(*rest.first().ok_or_else(|| {
                            anyhow!("corrupt_document: missing Word paragraph properties")
                        })?),
                    )
                } else {
                    (0, 2 * usize::from(cb) - 1)
                };
                let data = bytes(rest, offset, size)?;
                let style = u16_at(data, 0)?;
                grpprl(&data[2..])?.paragraph_style(style)
            }
        };
        physical.push(Run {
            start,
            end,
            properties: style,
        });
        Ok(())
    })?;
    let mut logical = Vec::new();
    // For a paragraph spanning several (possibly reordered) pieces, Word chooses
    // the style at the mark in its *last* piece, even for text in earlier pieces.
    for piece in pieces {
        let first = physical.partition_point(|r| r.end <= piece.start);
        for run in physical[first..].iter().take_while(|r| r.end <= piece.end) {
            ensure!(
                (run.end - piece.start) % piece.width == 0,
                "corrupt_document: misaligned Word paragraph boundary"
            );
            ensure!(
                logical.len() < MAX_RUNS,
                "resource_limit: binary Word paragraph formatting"
            );
            logical.push(Paragraph {
                end_cp: piece.cp + (run.end - piece.start) / piece.width,
                style: piece.properties.paragraph_style(run.properties),
            });
        }
    }
    Ok(logical)
}
