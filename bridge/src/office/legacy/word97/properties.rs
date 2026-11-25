// SPDX-License-Identifier: GPL-3.0-or-later
//! Character and paragraph properties from Word's FKP pages and fast-save pieces.
use super::styles::Styles;
use super::{bytes, u16_at, u32_at, MAX_RUNS};
use anyhow::{anyhow, ensure, Result};

const DELETED: u8 = 1;
const HIDDEN: u8 = 2;
const FIELD_CODE: u8 = 4;
const SEPARATOR: u8 = 8;
const XML_MARK: u8 = 16;

/// Actual property values at a position, and the style against which a ToggleOperand is evaluated.
#[derive(Clone, Copy, Default)]
pub(super) struct Visibility {
    style_id: u16,
    style: u8,
    value: u8,
    direct: u8,
}

impl Visibility {
    pub fn from_style(style: u8) -> Self {
        Self {
            style_id: 10,
            style,
            value: style,
            direct: 0,
        }
    }
    pub fn deleted(self) -> bool {
        self.value & DELETED != 0
    }
    pub fn hidden(self) -> bool {
        self.value & !DELETED != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Toggles {
    assigned: u8,
    relative: u8,
    on_or_inverted: u8,
}

impl Toggles {
    fn set(&mut self, flag: u8, operand: u8) -> Result<()> {
        ensure!(
            matches!(operand, 0 | 1 | 0x80 | 0x81) && (flag != XML_MARK || operand < 2),
            "corrupt_document: invalid Word visibility operand"
        );
        self.assigned |= flag;
        self.relative = (self.relative & !flag) | if operand & 0x80 != 0 { flag } else { 0 };
        self.on_or_inverted =
            (self.on_or_inverted & !flag) | if operand & 1 != 0 { flag } else { 0 };
        Ok(())
    }

    fn apply(self, current: u8, style: u8) -> u8 {
        (current & !self.assigned) | ((style & self.relative) ^ self.on_or_inverted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Edit {
    Toggle(Toggles),
    CharacterStyle(u16),
    Plain,
    ParagraphStyle(u16),
    Remap {
        paragraph: bool,
        first: u16,
        styles: Vec<u16>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Properties(Vec<Edit>);

impl Properties {
    fn toggle(&mut self, flag: u8, operand: u8) -> Result<()> {
        if let Some(Edit::Toggle(last)) = self.0.last_mut() {
            return last.set(flag, operand);
        }
        let mut value = Toggles::default();
        value.set(flag, operand)?;
        self.0.push(Edit::Toggle(value));
        Ok(())
    }
    pub fn paragraph_style(&self, mut id: u16) -> u16 {
        for edit in &self.0 {
            match edit {
                Edit::ParagraphStyle(next) => id = *next,
                Edit::Remap {
                    paragraph: true,
                    first,
                    styles,
                } => id = remapped(id, *first, styles),
                _ => {}
            }
        }
        id
    }
    /// Used while resolving a stylesheet's character UPX relative to its parent.
    pub fn style_flags(&self, inherited: u8) -> u8 {
        self.0.iter().fold(inherited, |state, edit| match edit {
            Edit::Toggle(t) => t.apply(state, inherited),
            _ => state,
        })
    }
    pub fn style_mask(&self) -> u8 {
        self.0.iter().fold(0, |mask, edit| match edit {
            Edit::Toggle(t) => mask | t.assigned,
            _ => mask,
        })
    }
    pub fn apply(&self, styles: &Styles, paragraph: u8, state: &mut Visibility) -> Result<()> {
        for edit in &self.0 {
            let mut reapply = 0;
            let (new_id, preserve) = match edit {
                Edit::Toggle(t) => {
                    state.value = t.apply(state.value, state.style);
                    state.direct |= t.assigned;
                    continue;
                }
                Edit::CharacterStyle(id) => (*id, DELETED | FIELD_CODE | XML_MARK),
                Edit::Plain => (10, DELETED | FIELD_CODE),
                Edit::Remap {
                    paragraph: false,
                    first,
                    styles: remapping,
                } => {
                    let id = remapped(state.style_id, *first, remapping);
                    if id == state.style_id {
                        continue;
                    }
                    reapply = state.direct & !styles.mask(state.style_id);
                    (id, DELETED | FIELD_CODE | XML_MARK)
                }
                _ => continue,
            };
            // MS-DOC 2.6.1: a character style starts from the paragraph style, preserving
            // revision, field, and certain other flags explicitly excluded from the reset.
            state.style_id = new_id;
            let selected = if matches!(edit, Edit::Plain) {
                paragraph
            } else {
                styles.flags(new_id, paragraph)?
            };
            state.style = (state.style & preserve) | (selected & !preserve);
            let keep_value = preserve | reapply;
            state.value = (state.value & keep_value) | (selected & !keep_value);
            state.direct &= keep_value;
        }
        Ok(())
    }
}

fn remapped(style: u16, first: u16, map: &[u16]) -> u16 {
    style
        .checked_sub(first)
        .and_then(|i| map.get(usize::from(i)))
        .copied()
        .unwrap_or(style)
}

pub(super) fn piece_properties(
    prm: u16,
    groups: &[Properties],
) -> Result<std::borrow::Cow<'_, Properties>> {
    use std::borrow::Cow;
    if prm & 1 != 0 {
        return groups
            .get(usize::from(prm >> 1))
            .map(Cow::Borrowed)
            .ok_or_else(|| anyhow!("corrupt_document: unknown Word property group"));
    }
    let mut properties = Properties::default();
    let flag = match (prm >> 1) & 0x7f {
        0x41 => DELETED,
        0x43 => FIELD_CODE,
        0x51 => SEPARATOR,
        0x53 => {
            properties.0.push(Edit::Plain);
            0
        }
        0x5c => HIDDEN,
        0x7b => XML_MARK,
        _ => 0,
    };
    if flag != 0 {
        properties.toggle(flag, (prm >> 8) as u8)?;
    }
    Ok(Cow::Owned(properties))
}

pub(super) fn grpprl(mut data: &[u8]) -> Result<Properties> {
    let mut properties = Properties::default();
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
        match opcode {
            0x0800 => properties.toggle(DELETED, operand[0])?,
            0x0802 => properties.toggle(FIELD_CODE, operand[0])?,
            0x083c => properties.toggle(HIDDEN, operand[0])?,
            0x0818 => properties.toggle(SEPARATOR, operand[0])?,
            0x2a90 => properties.toggle(XML_MARK, operand[0])?,
            0x4a30 => properties.0.push(Edit::CharacterStyle(u16_at(operand, 0)?)),
            0x2a33 => properties.0.push(Edit::Plain),
            0x4600 => properties.0.push(Edit::ParagraphStyle(u16_at(operand, 0)?)),
            0xca31 | 0xc601 => {
                let first = u16_at(operand, 2)?;
                let last = u16_at(operand, 4)?;
                ensure!(
                    last >= first,
                    "corrupt_document: invalid Word style remapping"
                );
                let mapped = bytes(operand, 6, 2 * (usize::from(last - first) + 1))?;
                properties.0.push(Edit::Remap {
                    paragraph: opcode == 0xc601,
                    first,
                    styles: mapped
                        .chunks_exact(2)
                        .map(|v| u16::from_le_bytes([v[0], v[1]]))
                        .collect(),
                });
            }
            _ => {}
        }
        data = &data[width..];
    }
    Ok(properties)
}

pub(super) struct Run<T> {
    pub start: usize,
    pub end: usize,
    pub properties: T,
}

pub(super) fn character_runs(bte: &[u8], word: &[u8]) -> Result<Vec<Run<Properties>>> {
    let mut ranges: Vec<Run<Properties>> = Vec::new();
    visit_fkp(bte, word, 1, |start, end, record| {
        let Some((&len, data)) = record.split_first() else {
            return Ok(());
        };
        let properties = grpprl(bytes(data, 0, usize::from(len))?)?;
        if properties.0.is_empty() {
            return Ok(());
        }
        if let Some(last) = ranges.last_mut() {
            if last.end == start && last.properties == properties {
                last.end = end;
                return Ok(());
            }
        }
        ranges.push(Run {
            start,
            end,
            properties,
        });
        Ok(())
    })?;
    Ok(ranges)
}

/// A BxPap is 13 bytes and a CHPX offset is one byte (MS-DOC 2.9.23, 2.9.312).
/// The two indexes share the layout of their PLC, FKP range array and half-word offsets.
pub(super) fn visit_fkp(
    bte: &[u8],
    word: &[u8],
    descriptor: usize,
    mut visit: impl FnMut(usize, usize, &[u8]) -> Result<()>,
) -> Result<()> {
    if bte.is_empty() {
        return Ok(());
    }
    ensure!(
        bte.len() >= 4 && (bte.len() - 4) % 8 == 0,
        "corrupt_document: invalid Word formatting index"
    );
    let pages = (bte.len() - 4) / 8;
    ensure!(
        pages <= MAX_RUNS && pages <= word.len() / 512,
        "corrupt_document: impossible Word formatting page count"
    );
    let mut total = 0;
    for i in 0..pages {
        let begin = u32_at(bte, 4 * i)? as usize;
        let limit = u32_at(bte, 4 * (i + 1))? as usize;
        ensure!(
            begin < limit,
            "corrupt_document: unordered Word formatting pages"
        );
        let pn = (u32_at(bte, 4 * (pages + 1 + i))? & 0x003f_ffff) as usize;
        let page = bytes(word, pn * 512, 512)?;
        let count = usize::from(page[511]);
        let offsets = 4 * (count + 1);
        let first_record = offsets + count * descriptor;
        ensure!(
            count > 0 && first_record <= 511,
            "corrupt_document: invalid Word formatting page"
        );
        total += count;
        ensure!(
            total <= MAX_RUNS,
            "resource_limit: binary Word formatting runs"
        );
        for j in 0..count {
            let a = u32_at(page, j * 4)? as usize;
            let b = u32_at(page, 4 * (j + 1))? as usize;
            ensure!(a < b, "corrupt_document: unordered Word formatting runs");
            if a >= limit || b <= begin {
                continue;
            }
            let loc = 2 * usize::from(page[offsets + j * descriptor]);
            let record = if loc == 0 {
                &[]
            } else {
                ensure!(
                    loc >= first_record && loc < 511,
                    "corrupt_document: invalid Word property offset"
                );
                &page[loc..511]
            };
            visit(a.max(begin), b.min(limit), record)?;
        }
    }
    Ok(())
}
