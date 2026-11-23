// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct character properties in the piece table and formatted disk pages ([MS-DOC] §2.4.6.2).
use super::{bytes, u16_at, u32_at, MAX_RUNS};
use anyhow::{anyhow, bail, ensure, Result};

const DELETED: u8 = 1;
const HIDDEN: u8 = 2;
const FIELD_CODE: u8 = 4;
const SEPARATOR: u8 = 8;
const XML_MARK: u8 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Visibility {
    assigned: u8,
    value: u8,
}

impl Visibility {
    pub fn deleted(self) -> bool {
        self.value & DELETED != 0
    }
    pub fn hidden(self) -> bool {
        self.value & !DELETED != 0
    }
    pub fn over(self, base: Self) -> Self {
        Self {
            assigned: self.assigned | base.assigned,
            value: (base.value & !self.assigned) | self.value,
        }
    }
    fn set(&mut self, flag: u8, operand: u8) -> Result<()> {
        if flag == 0 {
            return Ok(());
        }
        // A style-relative toggle cannot be resolved by the direct-formatting reader.
        ensure!(
            operand <= 1,
            "unsupported_document: style-relative Word text visibility"
        );
        self.assigned |= flag;
        if operand == 0 {
            self.value &= !flag;
        } else {
            self.value |= flag;
        }
        Ok(())
    }
}

pub(super) fn piece_properties(prm: u16, groups: &[Visibility]) -> Result<Visibility> {
    if prm & 1 != 0 {
        return groups
            .get(usize::from(prm >> 1))
            .copied()
            .ok_or_else(|| anyhow!("corrupt_document: unknown Word property group"));
    }
    let flag = match (prm >> 1) & 0x7f {
        0x41 => DELETED,
        0x43 => FIELD_CODE,
        0x51 => SEPARATOR,
        0x5c => HIDDEN,
        0x7b => XML_MARK,
        _ => 0,
    };
    let mut v = Visibility::default();
    v.set(flag, (prm >> 8) as u8)?;
    Ok(v)
}

pub(super) fn grpprl(mut data: &[u8]) -> Result<Visibility> {
    let mut v = Visibility::default();
    while !data.is_empty() {
        let opcode = u16_at(data, 0)?;
        data = &data[2..];
        let width = match opcode >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => {
                let size =
                    usize::from(*data.first().ok_or_else(|| {
                        anyhow!("corrupt_document: missing Word property length")
                    })?);
                match opcode {
                    // Table definitions store a 16-bit length including the second length byte.
                    0xd606 | 0xd608 => 1 + usize::from(u16_at(data, 0)?),
                    0xc615 if size == 255 => {
                        let deleted = usize::from(bytes(data, 1, 1)?[0]);
                        let added = usize::from(bytes(data, 2 + deleted * 4, 1)?[0]);
                        3 + deleted * 4 + added * 3
                    }
                    _ => size + 1,
                }
            }
        };
        let operand = bytes(data, 0, width)?;
        let flag = match opcode {
            0x0800 => DELETED,
            0x0802 => FIELD_CODE,
            0x083c => HIDDEN,
            0x0818 => SEPARATOR,
            0x2a90 => XML_MARK,
            _ => 0,
        };
        v.set(flag, operand[0])?;
        data = &data[width..];
    }
    Ok(v)
}

pub(super) struct Run {
    pub start: usize,
    pub end: usize,
    pub visibility: Visibility,
}

pub(super) fn character_runs(bte: &[u8], word: &[u8]) -> Result<Vec<Run>> {
    let mut ranges: Vec<Run> = Vec::new();
    if bte.is_empty() {
        return Ok(ranges);
    }
    ensure!(
        bte.len() >= 4 && (bte.len() - 4) % 8 == 0,
        "corrupt_document: invalid Word character formatting index"
    );
    let pages = (bte.len() - 4) / 8;
    ensure!(
        pages <= MAX_RUNS,
        "resource_limit: binary Word formatting pages"
    );
    ensure!(
        pages <= word.len() / 512,
        "corrupt_document: impossible Word formatting page count"
    );
    let mut total_runs = 0;
    for i in 0..pages {
        let begin = u32_at(bte, 4 * i)? as usize;
        let limit = u32_at(bte, 4 * i + 4)? as usize;
        ensure!(
            begin < limit,
            "corrupt_document: unordered Word formatting pages"
        );
        let pn = (u32_at(bte, (pages + 1 + i) * 4)? & 0x003f_ffff) as usize;
        let page = bytes(word, pn * 512, 512)?;
        let count = usize::from(page[511]);
        ensure!(
            (1..=101).contains(&count),
            "corrupt_document: invalid Word character formatting page"
        );
        total_runs += count;
        ensure!(
            total_runs <= MAX_RUNS,
            "resource_limit: binary Word formatting runs"
        );
        for j in 0..count {
            let a = u32_at(page, j * 4)? as usize;
            let b = u32_at(page, (j + 1) * 4)? as usize;
            ensure!(
                a < b,
                "corrupt_document: unordered Word character formatting runs"
            );
            let loc = 2 * usize::from(page[4 * (count + 1) + j]);
            if loc == 0 || a >= limit || b <= begin {
                continue;
            }
            ensure!(
                loc >= 4 * (count + 1) + count && loc < 511,
                "corrupt_document: invalid Word character property offset"
            );
            let prop = grpprl(bytes(&page[..511], loc + 1, usize::from(page[loc]))?)?;
            if prop.assigned == 0 {
                continue;
            }
            let start = a.max(begin);
            let end = b.min(limit);
            if let Some(last) = ranges.last_mut() {
                if last.end > start {
                    bail!("corrupt_document: overlapping Word character formatting");
                }
                if last.end == start && last.visibility == prop {
                    last.end = end;
                    continue;
                }
            }
            ranges.push(Run {
                start,
                end,
                visibility: prop,
            });
        }
    }
    Ok(ranges)
}
