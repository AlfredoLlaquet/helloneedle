// SPDX-License-Identifier: GPL-3.0-or-later
use super::package::parse;
use super::{paragraph_location, push_cell, Output, RichText, Style, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use roxmltree::Node;
use serde_json::json;
use std::collections::{HashMap, HashSet};
mod stream;
pub(super) use stream::extract;

const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const PRESENTATION: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";

#[derive(Default)]
struct Styles {
    definitions: HashMap<(String, String), (String, Formatting)>,
    resolved: HashMap<(String, String), Formatting>,
    bytes: usize,
}
#[derive(Clone, Copy, Default)]
struct Formatting {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
}
fn styles(root: Node<'_, '_>, out: &mut Styles) -> Result<()> {
    for n in root
        .descendants()
        .filter(|n| n.has_tag_name((STYLE, "style")))
    {
        let (Some(name), Some(family)) =
            (n.attribute((STYLE, "name")), n.attribute((STYLE, "family")))
        else {
            continue;
        };
        if !matches!(family, "text" | "paragraph") {
            continue;
        }
        let props = n
            .children()
            .find(|n| n.has_tag_name((STYLE, "text-properties")));
        let attr = |ns, local| props.and_then(|p| p.attribute((ns, local)));
        let parent = n.attribute((STYLE, "parent-style-name")).unwrap_or("");
        let key = (family.to_owned(), name.to_owned());
        if let Some((old_parent, _)) = out.definitions.get(&key) {
            out.bytes -= old_parent.len();
        } else {
            out.bytes = out.bytes.saturating_add(name.len() + family.len() + 192);
        }
        out.bytes = out.bytes.saturating_add(parent.len());
        if out.bytes > MAX_TEXT_BYTES {
            bail!("resource_limit: OpenDocument style table");
        }
        out.definitions.insert(
            key,
            (
                parent.into(),
                Formatting {
                    bold: attr(FO, "font-weight")
                        .map(|v| v == "bold" || v.parse::<u32>().is_ok_and(|n| n >= 600)),
                    italic: attr(FO, "font-style").map(|v| v == "italic" || v == "oblique"),
                    underline: attr(STYLE, "text-underline-style").map(|v| v != "none"),
                    strike: attr(STYLE, "text-line-through-style").map(|v| v != "none"),
                },
            ),
        );
    }
    Ok(())
}

impl Formatting {
    fn inherit(self, base: Self) -> Self {
        Self {
            bold: self.bold.or(base.bold),
            italic: self.italic.or(base.italic),
            underline: self.underline.or(base.underline),
            strike: self.strike.or(base.strike),
        }
    }
}
impl Styles {
    fn resolve(&mut self) -> Result<()> {
        // Resolve each style once. The explicit chain avoids recursion and
        // distinguishes malformed cycles from arbitrarily deep valid styles.
        for key in self.definitions.keys() {
            if self.resolved.contains_key(key) {
                continue;
            }
            let mut chain = Vec::new();
            let mut seen = HashSet::new();
            let mut current = key.clone();
            let mut inherited = loop {
                if let Some(saved) = self.resolved.get(&current) {
                    break *saved;
                }
                let Some((parent, format)) = self.definitions.get(&current) else {
                    break Formatting::default();
                };
                if !seen.insert(current.clone()) {
                    bail!("corrupt_document: circular OpenDocument style inheritance");
                }
                chain.push((current.clone(), *format));
                if parent.is_empty() {
                    break Formatting::default();
                }
                current.1.clone_from(parent);
            };
            while let Some((key, format)) = chain.pop() {
                inherited = format.inherit(inherited);
                self.resolved.insert(key, inherited);
            }
        }
        self.definitions.clear();
        Ok(())
    }
}
fn styled(map: &Styles, base: Style, family: &str, name: &str) -> Style {
    if name.is_empty() {
        return base;
    }
    let f = map
        .resolved
        .get(&(family.to_owned(), name.to_owned()))
        .copied()
        .unwrap_or_default();
    Style {
        bold: f.bold.unwrap_or(base.bold),
        italic: f.italic.unwrap_or(base.italic),
        underline: f.underline.unwrap_or(base.underline),
        strike: f.strike.unwrap_or(base.strike),
    }
}

fn repeat(s: Option<&str>) -> Result<usize> {
    let n = s
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| anyhow!("corrupt_document: invalid OpenDocument repeat count"))?;
    if n == 0 {
        bail!("corrupt_document: zero repeat count");
    }
    Ok(n)
}

/// All coordinates are represented as zero-based counts until emission. This
/// allows the largest representable one-based coordinate without overflow.
fn advance(base: usize, count: usize) -> Result<usize> {
    base.checked_add(count)
        .ok_or_else(|| anyhow!("resource_limit: OpenDocument coordinate overflow"))
}
fn heading(n: Node<'_, '_>) -> Option<usize> {
    n.has_tag_name((TEXT, "h")).then(|| {
        n.attribute((TEXT, "outline-level"))
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(1)
    })
}
fn is_para(n: Node<'_, '_>) -> bool {
    n.has_tag_name((TEXT, "p")) || n.has_tag_name((TEXT, "h"))
}
fn ignore(n: Node<'_, '_>) -> bool {
    n.has_tag_name((OFFICE, "annotation"))
        || n.has_tag_name((TEXT, "tracked-changes"))
        || n.has_tag_name((TEXT, "note"))
        || n.has_tag_name((PRESENTATION, "notes"))
}

fn paragraph(n: Node<'_, '_>, map: &Styles) -> Result<RichText> {
    let mut rich = RichText::default();
    let mut pending = false;
    let name = n.attribute((TEXT, "style-name")).unwrap_or("");
    inline(
        n,
        map,
        styled(map, Style::default(), "paragraph", name),
        &mut rich,
        &mut pending,
        0,
    )?;
    Ok(rich)
}
fn inline(
    n: Node<'_, '_>,
    map: &Styles,
    style: Style,
    rich: &mut RichText,
    pending: &mut bool,
    depth: usize,
) -> Result<()> {
    if depth > 128 {
        bail!("resource_limit: OpenDocument text nesting");
    }
    for c in n.children() {
        if c.is_text() {
            for segment in c
                .text()
                .unwrap_or("")
                .split_inclusive([' ', '\t', '\n', '\r'])
            {
                let word = segment.trim_end_matches([' ', '\t', '\n', '\r']);
                if !word.is_empty() {
                    if *pending && !rich.text.is_empty() {
                        rich.append(" ", style)?;
                    }
                    *pending = false;
                    rich.append(word, style)?;
                }
                if word.len() < segment.len() {
                    *pending = true;
                }
            }
        } else if c.is_element() && c.tag_name().namespace() == Some(TEXT) {
            match c.tag_name().name() {
                "s" | "tab" | "line-break" => {
                    if *pending && !rich.text.is_empty() {
                        rich.append(" ", style)?;
                    }
                    *pending = false;
                    match c.tag_name().name() {
                        "s" => {
                            let count = repeat(c.attribute((TEXT, "c")))?;
                            if count
                                > MAX_TEXT_BYTES.saturating_sub(rich.text.len() + rich.html.len())
                                    / 2
                            {
                                bail!("resource_limit: OpenDocument paragraph whitespace");
                            }
                            rich.append(&" ".repeat(count), style)?;
                        }
                        "tab" => rich.append("\t", style)?,
                        _ => rich.append("\n", style)?,
                    }
                }
                "note" | "ruby-text" | "tracked-changes" | "bookmark" | "bookmark-start"
                | "bookmark-end" => {}
                _ => {
                    let st = c
                        .attribute((TEXT, "style-name"))
                        .map_or(style, |name| styled(map, style, "text", name));
                    inline(c, map, st, rich, pending, depth + 1)?;
                }
            }
        }
    }
    Ok(())
}

fn writer(
    n: Node<'_, '_>,
    map: &Styles,
    cell: Option<(usize, usize, usize)>,
    counts: &mut (usize, usize),
    out: &mut Output,
    depth: usize,
) -> Result<()> {
    if depth > 128 {
        bail!("resource_limit: OpenDocument nesting");
    }
    for c in n.children().filter(|n| n.is_element()) {
        if is_para(c) {
            counts.0 = advance(counts.0, 1)?;
            let h = heading(c);
            let (text, html) = paragraph(c, map)?.finish(h);
            let (loc, label) = paragraph_location(counts.0, h, cell);
            out.push(text, loc, label, Some(html))?;
        } else if c.has_tag_name((TABLE, "table")) {
            counts.1 = advance(counts.1, 1)?;
            let table = counts.1;
            let mut last_row = 0;
            for row in table_rows(c) {
                writer_row(row, table, &mut last_row, map, counts, out, depth + 1)?;
            }
        } else if !ignore(c) {
            writer(c, map, cell, counts, out, depth + 1)?;
        }
    }
    Ok(())
}

/// Blank Writer paragraphs affect later paragraph numbers even though they have
/// no searchable block. Count a physically repeated empty cell just once.
fn blank_paragraphs(n: Node<'_, '_>, map: &Styles, depth: usize) -> Result<Option<usize>> {
    if depth > 128 {
        bail!("resource_limit: OpenDocument nesting");
    }
    let mut count = 0;
    for child in n.children().filter(|n| n.is_element()) {
        if is_para(child) {
            if !paragraph(child, map)?.text.trim().is_empty() {
                return Ok(None);
            }
            count = advance(count, 1)?;
        } else if child.has_tag_name((TABLE, "table")) {
            return Ok(None);
        } else if !ignore(child) {
            let Some(inner) = blank_paragraphs(child, map, depth + 1)? else {
                return Ok(None);
            };
            count = advance(count, inner)?;
        }
    }
    Ok(Some(count))
}

fn expanded(count: usize, copies: usize) -> Result<usize> {
    count
        .checked_mul(copies)
        .ok_or_else(|| anyhow!("resource_limit: OpenDocument logical count overflow"))
}

fn writer_row(
    row: Node<'_, '_>,
    table: usize,
    last_row: &mut usize,
    map: &Styles,
    counts: &mut (usize, usize),
    out: &mut Output,
    depth: usize,
) -> Result<()> {
    let rows = repeat(row.attribute((TABLE, "number-rows-repeated")))?;
    let end = advance(*last_row, rows)?;
    let mut col = 0;
    let mut cells = Vec::new();
    let mut whole_row_blank = true;
    let mut blank_count = 0;
    for cell in row.children().filter(|c| is_cell(*c)) {
        let cols = repeat(cell.attribute((TABLE, "number-columns-repeated")))?;
        let after = advance(col, cols)?;
        if cell.has_tag_name((TABLE, "table-cell")) {
            let blanks = if cell.first_element_child().is_none() {
                Some(0)
            } else if rows > 1 || cols > 1 {
                blank_paragraphs(cell, map, depth)?
            } else {
                None
            };
            if let Some(count) = blanks {
                blank_count = advance(blank_count, expanded(count, cols)?)?;
            } else {
                whole_row_blank = false;
            }
            if blanks != Some(0) {
                cells.push((col..after, cell, blanks));
            }
        }
        col = after;
    }
    if whole_row_blank {
        counts.0 = advance(counts.0, expanded(rows, blank_count)?)?;
    } else {
        for r in *last_row..end {
            for (range, cell, blanks) in &cells {
                if let Some(count) = blanks {
                    counts.0 = advance(counts.0, expanded(*count, range.len())?)?;
                    continue;
                }
                for c in range.clone() {
                    crate::search::checkpoint()?;
                    writer(*cell, map, Some((table, r + 1, c + 1)), counts, out, depth)?;
                }
            }
        }
    }
    *last_row = end;
    Ok(())
}

fn is_cell(n: Node<'_, '_>) -> bool {
    n.has_tag_name((TABLE, "table-cell")) || n.has_tag_name((TABLE, "covered-table-cell"))
}
fn table_rows<'a, 'i>(table: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    table.descendants().filter(move |n| {
        n.has_tag_name((TABLE, "table-row"))
            && n.ancestors().find(|a| a.has_tag_name((TABLE, "table"))) == Some(table)
    })
}
fn cell_text(n: Node<'_, '_>, map: &Styles) -> Result<String> {
    let mut text = String::new();
    for p in n.children().filter(|c| is_para(*c)) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&paragraph(p, map)?.text);
        if text.len() > MAX_TEXT_BYTES {
            bail!("resource_limit: spreadsheet cell");
        }
    }
    let attr = match n.attribute((OFFICE, "value-type")) {
        Some("date") => "date-value",
        Some("time") => "time-value",
        Some("boolean") => "boolean-value",
        Some("string") => "string-value",
        _ => "value",
    };
    if let Some(value) = n.attribute((OFFICE, attr)).filter(|s| !s.is_empty()) {
        if text.is_empty() {
            text.push_str(value);
        } else if value != text && attr != "string-value" {
            text.push('\n');
            text.push_str(value);
        }
    }
    Ok(text)
}
fn spreadsheet_row(
    row: Node<'_, '_>,
    sheet: &str,
    last_row: &mut usize,
    map: &Styles,
    out: &mut Output,
) -> Result<()> {
    let end_row = advance(
        *last_row,
        repeat(row.attribute((TABLE, "number-rows-repeated")))?,
    )?;
    let mut col = 0;
    let mut blocks_per_row = 0usize;
    let mut cells = Vec::new();
    let mut retained_bytes = 0usize;
    for cell in row.children().filter(|n| is_cell(*n)) {
        let end_col = advance(
            col,
            repeat(cell.attribute((TABLE, "number-columns-repeated")))?,
        )?;
        if cell.has_tag_name((TABLE, "table-cell")) {
            let value = cell_text(cell, map)?;
            if !value.trim().is_empty() {
                retained_bytes = retained_bytes
                    .saturating_add(value.len() + std::mem::size_of::<(usize, usize, String)>());
                if retained_bytes > MAX_TEXT_BYTES {
                    bail!("resource_limit: OpenDocument live row");
                }
                blocks_per_row = advance(blocks_per_row, end_col - col)?;
                let may_match = if let Some(matcher) = out
                    .repeat_matcher
                    .filter(|_| end_row - *last_row > 1 || end_col - col > 1)
                {
                    use grep_matcher::Matcher;
                    // A false answer proves every copy can be skipped. Positive
                    // candidates use the normal matcher and UTF-16 offsets.
                    matcher.is_match(value.as_bytes())?
                } else {
                    true
                };
                cells.push((col..end_col, value, may_match));
            }
        }
        col = end_col;
    }
    if blocks_per_row != 0 {
        let rows = end_row - *last_row;
        let skip_rows = if cells.iter().all(|(_, _, candidate)| !candidate) {
            rows
        } else {
            (out.window_start.saturating_sub(out.index) / blocks_per_row).min(rows)
        };
        out.advance(
            skip_rows
                .checked_mul(blocks_per_row)
                .ok_or_else(|| anyhow!("resource_limit: logical Office block index overflow"))?,
        )?;
        for r in *last_row + skip_rows..end_row {
            for (range, value, may_match) in &cells {
                let skipped = if *may_match {
                    out.window_start.saturating_sub(out.index).min(range.len())
                } else {
                    range.len()
                };
                out.advance(skipped)?;
                for c in range.start + skipped..range.end {
                    crate::search::checkpoint()?;
                    push_cell(out, sheet, r + 1, c + 1, value.clone())?;
                }
            }
        }
    }
    *last_row = end_row;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(body: &str, styles: &str) -> String {
        format!(
            r#"<o:document xmlns:o="{OFFICE}" xmlns:t="{TEXT}" xmlns:g="{TABLE}" xmlns:s="{STYLE}" xmlns:f="{FO}"><o:automatic-styles>{styles}</o:automatic-styles><o:body>{body}</o:body></o:document>"#
        )
    }
    #[test]
    fn deep_styles_and_outline_levels_survive_and_style_cycles_are_errors() {
        let mut chain = String::from(
            r#"<s:style s:family="text" s:name="s0"><s:text-properties f:font-weight="bold"/></s:style>"#,
        );
        for i in 1..65 {
            chain.push_str(&format!(
                r#"<s:style s:family="text" s:name="s{i}" s:parent-style-name="s{}"/>"#,
                i - 1
            ));
        }
        let body = r#"<o:text><t:h t:outline-level="21"><t:span t:style-name="s64">Target</t:span></t:h></o:text>"#;
        let mut out = Output::default();
        extract(document(body, &chain).as_bytes(), None, "fodt", &mut out).unwrap();
        assert_eq!(out.blocks[0].location["heading_level"], 21);
        assert!(out.blocks[0]
            .html
            .as_ref()
            .unwrap()
            .contains("<b>Target</b>"));
        chain.push_str(r#"<s:style s:family="text" s:name="s0" s:parent-style-name="s64"/>"#);
        assert!(
            extract(document(body, &chain).as_bytes(), None, "fodt", &mut out)
                .unwrap_err()
                .to_string()
                .contains("circular")
        );
    }
    #[test]
    fn writer_expands_logical_coordinates_and_counts_nested_tables() {
        let body = r#"<o:text><g:table><g:table-row g:number-rows-repeated="2"><g:table-cell g:number-columns-repeated="2"><t:p>A</t:p></g:table-cell><g:covered-table-cell/><g:table-cell><g:table><g:table-row><g:table-cell><t:p>B</t:p></g:table-cell></g:table-row></g:table></g:table-cell></g:table-row><g:table-row><g:table-cell><t:p>C</t:p></g:table-cell></g:table-row></g:table></o:text>"#;
        let mut out = Output::default();
        extract(document(body, "").as_bytes(), None, "odt", &mut out).unwrap();
        let coords: Vec<_> = out
            .blocks
            .iter()
            .map(|b| {
                (
                    b.text.as_str(),
                    b.location["table"].as_u64().unwrap(),
                    b.location["row"].as_u64().unwrap(),
                    b.location["column"].as_u64().unwrap(),
                    b.location["paragraph"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            coords,
            [
                ("A", 1, 1, 1, 1),
                ("A", 1, 1, 2, 2),
                ("B", 2, 1, 1, 3),
                ("A", 1, 2, 1, 4),
                ("A", 1, 2, 2, 5),
                ("B", 3, 1, 1, 6),
                ("C", 1, 3, 1, 7)
            ]
        );
    }
    #[test]
    fn sparse_open_spreadsheet_exceeds_excel_dimensions() {
        let body = r#"<o:spreadsheet><g:table g:name="Sheet"><g:table-row g:number-rows-repeated="2000000"><g:table-cell g:number-columns-repeated="60000"/></g:table-row><g:table-row g:number-rows-repeated="2"><g:covered-table-cell g:number-columns-repeated="16384"/><g:table-cell g:number-columns-repeated="2" o:value-type="string" o:string-value="Target"/></g:table-row></g:table></o:spreadsheet>"#;
        let mut out = Output::default();
        extract(document(body, "").as_bytes(), None, "ods", &mut out).unwrap();
        let cells: Vec<_> = out.blocks.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(
            cells,
            [
                "Sheet!XFE2000001",
                "Sheet!XFF2000001",
                "Sheet!XFE2000002",
                "Sheet!XFF2000002"
            ]
        );
    }
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn empty_repeated_writer_paragraphs_advance_the_later_location() {
        let body = r#"<o:text><g:table><g:table-row g:number-rows-repeated="1000000"><g:table-cell g:number-columns-repeated="1000000"><t:p/></g:table-cell></g:table-row></g:table><t:p>Tail</t:p></o:text>"#;
        let mut out = Output::default();
        extract(document(body, "").as_bytes(), None, "fodt", &mut out).unwrap();
        assert_eq!(out.blocks.len(), 1);
        assert_eq!(out.blocks[0].location["paragraph"], 1_000_000_000_001u64);
    }
    #[test]
    fn more_than_a_million_explicit_spaces_fit_one_paragraph() {
        let mut out = Output::default();
        let body = r#"<o:text><t:p>X<t:s t:c="1000001"/>Y</t:p></o:text>"#;
        extract(document(body, "").as_bytes(), None, "fodt", &mut out).unwrap();
        assert_eq!(out.blocks[0].text.len(), 1_000_003);
    }
}
