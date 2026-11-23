// SPDX-License-Identifier: GPL-3.0-or-later
use super::{paragraph_location, Output, RichText, Style, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use encoding_rs::Encoding;
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct State {
    style: Style,
    page: u16,
    font: Option<i32>,
    fallback_length: usize,
    hidden: bool,
    ignored: bool,
    starred: bool,
    font_table: bool,
    alternatives: Option<u8>,
    unicode_alternative: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            style: Style::default(), page: 1252, font: None, fallback_length: 1,
            hidden: false, ignored: false, starred: false, font_table: false,
            alternatives: None, unicode_alternative: false,
        }
    }
}

struct Reader<'a> {
    out: &'a mut Output,
    state: State,
    parents: Vec<State>,
    fonts: HashMap<i32, u16>,
    default_font: Option<i32>,
    ansi: Vec<u8>,
    fallback: usize,
    high_surrogate: Option<u16>,
    paragraph: RichText,
    number: usize,
}

pub(super) fn extract(bytes: &[u8], out: &mut Output) -> Result<()> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if !bytes.starts_with(b"{\\rtf") {
        bail!("unsupported_document: missing RTF signature");
    }
    if bytes.get(5) != Some(&b'1') || bytes.get(6).is_some_and(u8::is_ascii_digit) {
        bail!("unsupported_document: RTF version");
    }
    let mut reader = Reader {
        out, state: State::default(), parents: Vec::new(), fonts: HashMap::new(),
        default_font: None, ansi: Vec::new(), fallback: 0, high_surrogate: None,
        paragraph: RichText::default(), number: 0,
    };
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        i += 1;
        match ch {
            b'{' => {
                reader.flush_ansi()?;
                if reader.parents.len() >= 256 { bail!("resource_limit: RTF group depth"); }
                let mut child = reader.state;
                child.alternatives = None;
                child.unicode_alternative = false;
                if let Some(branch) = reader.state.alternatives.as_mut() {
                    *branch = branch.saturating_add(1);
                    child.ignored |= *branch != 2;
                    child.unicode_alternative = *branch == 2;
                }
                reader.parents.push(reader.state);
                reader.state = child;
                reader.fallback = 0;
            }
            b'}' => {
                reader.flush_ansi()?;
                reader.state = reader.parents.pop().ok_or_else(|| anyhow!("corrupt_document: unmatched RTF brace"))?;
                reader.fallback = 0;
                if reader.parents.is_empty() {
                    if bytes[i..].iter().any(|b| !b.is_ascii_whitespace() && *b != 0) {
                        bail!("corrupt_document: unexpected data after RTF group");
                    }
                    reader.end_paragraph()?;
                    return Ok(());
                }
            }
            b'\\' => {
                let c = *bytes.get(i).ok_or_else(|| anyhow!("corrupt_document: truncated RTF control"))?;
                i += 1;
                if c == b'\'' {
                    let hex = bytes.get(i..i+2).ok_or_else(|| anyhow!("corrupt_document: short RTF hex escape"))?;
                    let hi = (hex[0] as char).to_digit(16);
                    let lo = (hex[1] as char).to_digit(16);
                    let (Some(hi), Some(lo)) = (hi, lo) else { bail!("corrupt_document: invalid RTF hex escape"); };
                    reader.byte((hi * 16 + lo) as u8)?;
                    i += 2;
                } else if c.is_ascii_alphabetic() {
                    let start = i - 1;
                    while bytes.get(i).is_some_and(u8::is_ascii_alphabetic) { i += 1; }
                    let word = &bytes[start..i];
                    let value_start = i;
                    if bytes.get(i) == Some(&b'-') && bytes.get(i+1).is_some_and(u8::is_ascii_digit) { i += 1; }
                    while bytes.get(i).is_some_and(u8::is_ascii_digit) { i += 1; }
                    let val = if i > value_start {
                        Some(std::str::from_utf8(&bytes[value_start..i])?.parse::<i32>()
                            .map_err(|_| anyhow!("corrupt_document: RTF parameter out of range"))?)
                    } else { None };
                    if bytes.get(i) == Some(&b' ') { i += 1; }
                    if word == b"bin" {
                        let size = val.and_then(|n| usize::try_from(n).ok())
                            .ok_or_else(|| anyhow!("corrupt_document: invalid RTF binary length"))?;
                        if size > bytes.len() - i { bail!("corrupt_document: truncated RTF binary data"); }
                        i += size;
                    }
                    if !reader.skip_fallback() {
                        reader.flush_ansi()?;
                        reader.control(word, val)?;
                    }
                } else if !reader.skip_fallback() {
                    reader.flush_ansi()?;
                    match c {
                        b'*' => reader.state.starred = true,
                        b'\\' | b'{' | b'}' => reader.literal(&(c as char).to_string())?,
                        b'~' => reader.literal("\u{a0}")?,
                        b'-' => reader.literal("\u{ad}")?,
                        b'_' => reader.literal("\u{2011}")?,
                        b'\r' | b'\n' if reader.visible() => {
                            if c == b'\r' && bytes.get(i) == Some(&b'\n') { i += 1; }
                            reader.end_paragraph()?;
                        }
                        _ => {},
                    }
                }
            }
            b'\r' | b'\n' => {},
            b => reader.byte(b)?,
        }
    }
    bail!("corrupt_document: unclosed RTF group")
}

impl Reader<'_> {
    fn visible(&self) -> bool { !self.state.ignored && !self.state.hidden }

    fn skip_fallback(&mut self) -> bool {
        if self.fallback == 0 { return false; }
        self.fallback -= 1;
        true
    }

    fn byte(&mut self, b: u8) -> Result<()> {
        if !self.skip_fallback() && self.visible() {
            if self.ansi.len() >= MAX_TEXT_BYTES { bail!("resource_limit: RTF text run"); }
            self.ansi.push(b);
        }
        Ok(())
    }

    fn flush_ansi(&mut self) -> Result<()> {
        if self.ansi.is_empty() { return Ok(()); }
        self.flush_surrogate()?;
        if self.ansi.is_ascii() {
            self.paragraph.append(std::str::from_utf8(&self.ansi)?, self.state.style)?;
        } else {
            let page = self.state.font.or(self.default_font).and_then(|f| self.fonts.get(&f))
                .copied().unwrap_or(self.state.page);
            let text = decode(&self.ansi, page)?;
            self.paragraph.append(&text, self.state.style)?;
        }
        self.ansi.clear();
        Ok(())
    }

    fn flush_surrogate(&mut self) -> Result<()> {
        if self.high_surrogate.take().is_some() {
            self.paragraph.append("\u{fffd}", self.state.style)?;
        }
        Ok(())
    }

    fn literal(&mut self, s: &str) -> Result<()> {
        if self.visible() {
            self.flush_surrogate()?;
            self.paragraph.append(s, self.state.style)?;
        }
        Ok(())
    }

    fn unicode(&mut self, n: i32) -> Result<()> {
        if !(-32768..=65535).contains(&n) { bail!("corrupt_document: invalid RTF Unicode unit"); }
        let unit = n as u16;
        self.fallback = self.state.fallback_length;
        if !self.visible() { return Ok(()); }
        if (0xd800..=0xdbff).contains(&unit) {
            self.flush_surrogate()?;
            self.high_surrogate = Some(unit);
        } else {
            let scalar = if let Some(high) = self.high_surrogate.take() {
                if (0xdc00..=0xdfff).contains(&unit) {
                    0x10000 + (u32::from(high - 0xd800) << 10) + u32::from(unit - 0xdc00)
                } else {
                    self.paragraph.append("\u{fffd}", self.state.style)?;
                    u32::from(unit)
                }
            } else { u32::from(unit) };
            let mut buf = [0; 4];
            let ch = char::from_u32(scalar).unwrap_or('\u{fffd}');
            self.paragraph.append(ch.encode_utf8(&mut buf), self.state.style)?;
        }
        Ok(())
    }

    fn end_paragraph(&mut self) -> Result<()> {
        self.flush_surrogate()?;
        self.number += 1;
        let (text, html) = std::mem::take(&mut self.paragraph).finish(None);
        let (loc, label) = paragraph_location(self.number, None, None);
        self.out.push(text, loc, label, Some(html))
    }

    fn control(&mut self, word: &[u8], arg: Option<i32>) -> Result<()> {
        if self.state.starred {
            self.state.starred = false;
            if !(word == b"ud" && self.state.unicode_alternative) { self.state.ignored = true; }
        }
        match word {
            b"fonttbl" => { self.state.font_table = true; self.state.ignored = true; }
            b"f" if self.state.font_table => self.state.font = arg,
            b"fcharset" | b"cpg" if self.state.font_table => {
                if let (Some(font), Some(n)) = (self.state.font, arg) {
                    let page = if word == b"cpg" { u16::try_from(n).ok() } else { charset(n) };
                    if let Some(page) = page { self.fonts.insert(font, page); }
                }
            }
            b"colortbl" | b"stylesheet" | b"info" | b"pict" | b"object" | b"shp" |
            b"nonshppict" | b"fieldinst" | b"fldinst" | b"datafield" | b"filetbl" |
            b"header" | b"headerl" | b"headerr" | b"headerf" | b"footer" | b"footerl" |
            b"footerr" | b"footerf" | b"annotation" | b"footnote" | b"listtable" |
            b"listoverridetable" | b"xmlnstbl" | b"themedata" | b"datastore" |
            b"generator" | b"listtext" | b"pntext" | b"xe" | b"tc" => self.state.ignored = true,
            _ if self.state.ignored => {},
            b"upr" => self.state.alternatives = Some(0),
            b"deff" => self.default_font = arg,
            b"f" => self.state.font = arg,
            b"ansi" => self.state.page = 1252,
            b"mac" => self.state.page = 10000,
            b"pc" => self.state.page = 437,
            b"pca" => self.state.page = 850,
            b"ansicpg" => self.state.page = arg.and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| anyhow!("corrupt_document: invalid RTF code page"))?,
            b"uc" => self.state.fallback_length = arg.and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n <= 255).ok_or_else(|| anyhow!("corrupt_document: invalid RTF fallback length"))?,
            b"u" => self.unicode(arg.ok_or_else(|| anyhow!("corrupt_document: missing RTF Unicode value"))?)?,
            b"v" | b"deleted" => self.state.hidden = arg != Some(0),
            b"b" => self.state.style.bold = arg != Some(0),
            b"i" => self.state.style.italic = arg != Some(0),
            b"ul" | b"uldb" | b"ulw" => self.state.style.underline = arg != Some(0),
            b"ulnone" => self.state.style.underline = false,
            b"strike" => self.state.style.strike = arg != Some(0),
            b"plain" => { self.state.style = Style::default(); self.state.font = None; self.state.hidden = false; }
            b"par" | b"row" | b"page" | b"sect" if self.visible() => self.end_paragraph()?,
            b"line" => self.literal("\n")?,
            b"tab" | b"cell" => self.literal("\t")?,
            b"emdash" => self.literal("—")?,
            b"endash" => self.literal("–")?,
            b"bullet" => self.literal("•")?,
            b"lquote" => self.literal("‘")?,
            b"rquote" => self.literal("’")?,
            b"ldblquote" => self.literal("“")?,
            b"rdblquote" => self.literal("”")?,
            b"enspace" => self.literal("\u{2002}")?,
            b"emspace" => self.literal("\u{2003}")?,
            b"qmspace" => self.literal("\u{2005}")?,
            _ => {},
        }
        Ok(())
    }
}

fn charset(n: i32) -> Option<u16> {
    Some(match n {
        77 => 10000, 128 => 932, 129 => 949, 130 => 1361, 134 => 936, 136 => 950,
        161 => 1253, 162 => 1254, 163 => 1258, 177 => 1255, 178 => 1256,
        186 => 1257, 204 => 1251, 222 => 874, 238 => 1250, 254 => 437, _ => return None,
    })
}

fn decode(bytes: &[u8], page: u16) -> Result<std::borrow::Cow<'_, str>> {
    let table = match page { 437 => Some(CP437), 850 => Some(CP850), _ => None };
    if let Some(table) = table {
        let hi: Vec<_> = table.chars().collect();
        return Ok(bytes.iter().map(|b| if *b < 128 { *b as char } else { hi[(*b - 128) as usize] }).collect());
    }
    let label = match page {
        65001 => "utf-8".into(), 10000 => "macintosh".into(), 932 => "shift_jis".into(),
        936 => "gbk".into(), 949 => "euc-kr".into(), 950 => "big5".into(), 866 => "ibm866".into(),
        28591..=28599 => format!("iso-8859-{}", page - 28590),
        1250..=1258 | 874 => format!("windows-{page}"),
        _ => bail!("unsupported_document: RTF code page {page}"),
    };
    let encoding = Encoding::for_label(label.as_bytes())
        .ok_or_else(|| anyhow!("unsupported_document: RTF code page {page}"))?;
    encoding.decode_without_bom_handling_and_without_replacement(bytes)
        .ok_or_else(|| anyhow!("corrupt_document: invalid RTF text for code page {page}"))
}

// Upper halves of the original IBM PC and Western European DOS code pages.
const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";
const CP850: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒáíóúñÑªº¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀ÓßÔÒõÕµþÞÚÛÙýÝ¯´≡±‗¾¶§÷¸°¨·¹³²■ ";
