// SPDX-License-Identifier: GPL-3.0-or-later
use super::package::{parse, Package, Rel};
use super::stream_xml::{Element, Event, Xml};
use super::{name, push_cell, Output, MAX_COLS, MAX_ROWS, MAX_TEXT_BYTES};
use std::io::Read;
mod shared_strings;
use anyhow::{anyhow, bail, Result};
use roxmltree::{Document, Node};
use serde_json::json;
use shared_strings::{Builder, SharedStrings};
use std::collections::HashMap;

const WORD: &str = "wordprocessingml";
const SHEET: &str = "spreadsheetml";
const PRES: &str = "presentationml";
const DRAW: &str = "drawingml";
fn namespace(ns: Option<&str>, group: &str) -> bool {
    let ns = ns.unwrap_or("");
    [
        ("http://schemas.openxmlformats.org/", "/2006/main"),
        ("http://purl.oclc.org/ooxml/", "/main"),
    ]
    .iter()
    .any(|(prefix, suffix)| {
        ns.strip_prefix(prefix).and_then(|v| v.strip_suffix(suffix)) == Some(group)
    })
}
fn is(n: Node<'_, '_>, group: &str, local: &str) -> bool {
    n.is_element() && name(n) == local && namespace(n.tag_name().namespace(), group)
}
fn word(n: Node<'_, '_>, tag: &str) -> bool {
    is(n, WORD, tag)
}
fn ss(n: Node<'_, '_>, tag: &str) -> bool {
    is(n, SHEET, tag)
}
fn rid<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attributes()
        .find(|a| {
            a.name() == "id"
                && matches!(
                    a.namespace(),
                    Some(
                        "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
                            | "http://purl.oclc.org/ooxml/officeDocument/relationships"
                    )
                )
        })
        .map(|a| a.value())
}
fn wval<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == "val" && namespace(a.namespace(), WORD))
        .map(|a| a.value())
}
fn child<'a, 'i>(n: Node<'a, 'i>, group: &str, local: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|&v| is(v, group, local))
}

pub(super) fn main_part(pkg: &mut Package, fallback: &str) -> Result<String> {
    let root = pkg.rels("")?;
    if root.is_empty() {
        Ok(fallback.to_owned())
    } else {
        root.values()
            .find(|r| r.kind == "officeDocument")
            .and_then(|r| r.part.clone())
            .ok_or_else(|| anyhow!("corrupt_document: missing office document relationship"))
    }
}

pub(super) fn extract(pkg: &mut Package, kind: &str, out: &mut Output) -> Result<()> {
    let main = main_part(
        pkg,
        if kind == "xlsx" {
            "xl/workbook.xml"
        } else {
            "ppt/presentation.xml"
        },
    )?;
    let text = pkg.required(&main)?;
    let doc = parse(&text)?;
    let rels = pkg.rels(&main)?;
    match kind {
        "xlsx" => workbook(pkg, &rels, &doc, out),
        _ => presentation(pkg, &rels, &doc, out),
    }
}

fn related<'a>(rels: &'a HashMap<String, Rel>, id: Option<&str>) -> Result<&'a Rel> {
    id.and_then(|id| rels.get(id))
        .ok_or_else(|| anyhow!("corrupt_document: missing part relationship"))
}
fn internal(rel: &Rel) -> Result<&str> {
    rel.part
        .as_deref()
        .ok_or_else(|| anyhow!("unsupported_document: external or invalid part"))
}

pub(super) fn named_heading(s: &str) -> Option<usize> {
    let name = s.to_ascii_lowercase();
    name.strip_prefix("heading")?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=9).contains(n))
}
fn outline(ppr: Node<'_, '_>) -> Option<usize> {
    wval(child(ppr, WORD, "outlineLvl")?)?
        .parse::<usize>()
        .ok()
        .filter(|v| *v < 9)
        .map(|n| n + 1)
}
pub(super) fn word_headings(pkg: &mut Package, main: &str) -> Result<HashMap<String, usize>> {
    let rels = pkg.rels(main)?;
    let mut headings = HashMap::new();
    if let Some(rel) = rels.values().find(|r| r.kind == "styles") {
        if let Some(text) = pkg.optional(internal(rel)?)? {
            let styles = parse(&text)?;
            for s in styles
                .root_element()
                .children()
                .filter(|&n| word(n, "style"))
            {
                let id = s
                    .attributes()
                    .find(|a| a.name() == "styleId")
                    .map(|a| a.value());
                let level = child(s, WORD, "pPr").and_then(outline).or_else(|| {
                    child(s, WORD, "name")
                        .and_then(wval)
                        .and_then(named_heading)
                });
                if let (Some(id), Some(l)) = (id, level) {
                    headings.insert(id.into(), l);
                }
            }
        }
    }
    Ok(headings)
}

fn coordinate(v: &str) -> Result<(usize, usize)> {
    let split = v
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| anyhow!("corrupt_document: invalid cell reference"))?;
    let mut col = 0usize;
    for b in v[..split].bytes() {
        if !b.is_ascii_alphabetic() || col > MAX_COLS {
            bail!("corrupt_document: invalid cell reference");
        }
        col = col * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize;
    }
    let row = v[split..]
        .parse::<usize>()
        .map_err(|_| anyhow!("corrupt_document: invalid cell row"))?;
    if !(1..=MAX_COLS).contains(&col) || !(1..=MAX_ROWS).contains(&row) {
        bail!("resource_limit: spreadsheet coordinates");
    }
    Ok((row, col))
}

fn workbook(
    pkg: &mut Package,
    rels: &HashMap<String, Rel>,
    doc: &Document<'_>,
    out: &mut Output,
) -> Result<()> {
    let root = doc.root_element();
    let sheets = child(root, SHEET, "sheets")
        .filter(|_| ss(root, "workbook"))
        .ok_or_else(|| anyhow!("corrupt_document: invalid workbook"))?;
    let mut strings = Builder::default();
    if let Some(rel) = rels.values().find(|r| r.kind == "sharedStrings") {
        shared_table(pkg.stream(internal(rel)?)?, &mut strings)?;
    }
    let mut shared = strings.finish()?;
    for sheet in sheets.children().filter(|&n| ss(n, "sheet")) {
        let sheet_name = sheet
            .attribute("name")
            .ok_or_else(|| anyhow!("corrupt_document: unnamed sheet"))?;
        let rel = related(rels, rid(sheet))?;
        if rel.kind != "worksheet" {
            continue;
        }
        worksheet(pkg.stream(internal(rel)?)?, sheet_name, &mut shared, out)?;
    }
    Ok(())
}

fn presentation(
    pkg: &mut Package,
    rels: &HashMap<String, Rel>,
    doc: &Document<'_>,
    out: &mut Output,
) -> Result<()> {
    let root = doc.root_element();
    if !is(root, PRES, "presentation") {
        bail!("corrupt_document: invalid presentation");
    }
    let Some(slides) = child(root, PRES, "sldIdLst") else {
        return Ok(());
    };
    for (idx, id) in slides
        .children()
        .filter(|&n| is(n, PRES, "sldId"))
        .enumerate()
    {
        let slide = idx + 1;
        let rel = related(rels, rid(id))?;
        if rel.kind != "slide" {
            bail!("corrupt_document: slide relationship");
        }
        slide_text(pkg.stream(internal(rel)?)?, slide, out)?;
    }
    Ok(())
}

fn tag_is(tag: &Element, group: &str, name: &str) -> bool {
    tag.name == name && namespace(Some(&tag.namespace), group)
}

fn add_text(to: &mut String, extra: &str) -> Result<()> {
    anyhow::ensure!(
        extra.len() <= MAX_TEXT_BYTES - to.len(),
        "resource_limit: Office text working unit"
    );
    to.push_str(extra);
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    Ignore,
    Root,
    Data,
    Row,
    Cell,
    Rich,
    Run,
    Text,
    Value,
}

fn rich_child(parent: NodeKind, tag: &Element) -> NodeKind {
    if parent == NodeKind::Rich && tag_is(tag, SHEET, "r") {
        NodeKind::Run
    } else if matches!(parent, NodeKind::Rich | NodeKind::Run) && tag_is(tag, SHEET, "t") {
        NodeKind::Text
    } else {
        NodeKind::Ignore
    }
}

fn shared_table(source: impl Read, strings: &mut Builder) -> Result<()> {
    let mut xml = Xml::new(source)?;
    let mut stack = Vec::new();
    let mut value = String::new();
    while let Some(event) = xml.next()? {
        match event {
            Event::Start(tag) => {
                let kind = match stack.last().copied() {
                    None if tag_is(&tag, SHEET, "sst") => NodeKind::Root,
                    None => bail!("corrupt_document: invalid shared string table"),
                    Some(NodeKind::Root) if tag_is(&tag, SHEET, "si") => NodeKind::Rich,
                    Some(parent) => rich_child(parent, &tag),
                };
                stack.push(kind);
            }
            Event::Text(text) if stack.last() == Some(&NodeKind::Text) => {
                add_text(&mut value, &text)?
            }
            Event::End if stack.pop() == Some(NodeKind::Rich) => {
                strings.append(std::mem::take(&mut value))?
            }
            _ => {}
        }
    }
    Ok(())
}

fn worksheet(
    source: impl Read,
    sheet: &str,
    strings: &mut SharedStrings,
    out: &mut Output,
) -> Result<()> {
    let mut xml = Xml::new(source)?;
    let mut stack = Vec::new();
    let (mut row, mut col, mut cell_row) = (0usize, 0usize, 0usize);
    let mut cell_type = String::new();
    let mut visible = String::new();
    let mut stored = None::<String>;
    let mut saw_data = false;
    while let Some(event) = xml.next()? {
        match event {
            Event::Start(tag) => {
                let kind = match stack.last().copied() {
                    None if tag_is(&tag, SHEET, "worksheet") => NodeKind::Root,
                    None => bail!("corrupt_document: invalid worksheet"),
                    Some(NodeKind::Root) if tag_is(&tag, SHEET, "sheetData") => {
                        saw_data = true;
                        NodeKind::Data
                    }
                    Some(NodeKind::Data) if tag_is(&tag, SHEET, "row") => {
                        row = match tag.attr("", "r") {
                            Some(s) => s
                                .parse()
                                .map_err(|_| anyhow!("corrupt_document: invalid sheet row"))?,
                            None => row + 1,
                        };
                        anyhow::ensure!(
                            (1..=MAX_ROWS).contains(&row),
                            "resource_limit: spreadsheet row"
                        );
                        col = 0;
                        NodeKind::Row
                    }
                    Some(NodeKind::Row) if tag_is(&tag, SHEET, "c") => {
                        (cell_row, col) = match tag.attr("", "r") {
                            Some(address) => coordinate(address)?,
                            None => (row, col + 1),
                        };
                        anyhow::ensure!(col <= MAX_COLS, "resource_limit: spreadsheet column");
                        cell_type.clear();
                        cell_type.push_str(tag.attr("", "t").unwrap_or(""));
                        visible.clear();
                        stored = None;
                        NodeKind::Cell
                    }
                    Some(NodeKind::Cell) if tag_is(&tag, SHEET, "v") => {
                        stored = Some(String::new());
                        NodeKind::Value
                    }
                    Some(NodeKind::Cell) if tag_is(&tag, SHEET, "is") => NodeKind::Rich,
                    Some(parent) => rich_child(parent, &tag),
                };
                stack.push(kind);
            }
            Event::Text(text) => match stack.last() {
                Some(NodeKind::Text) => add_text(&mut visible, &text)?,
                Some(NodeKind::Value) => add_text(stored.as_mut().unwrap(), &text)?,
                _ => {}
            },
            Event::End if stack.pop() == Some(NodeKind::Cell) => {
                let value = match (cell_type.as_str(), stored.take()) {
                    ("inlineStr", _) => std::mem::take(&mut visible),
                    ("s", Some(index)) if !index.is_empty() => strings.get(&index)?,
                    ("b", Some(s)) if s == "1" => "TRUE".into(),
                    ("b", Some(s)) if s == "0" => "FALSE".into(),
                    (_, value) => value.unwrap_or_default(),
                };
                push_cell(out, sheet, cell_row, col, value)?;
            }
            Event::End => {}
        }
    }
    anyhow::ensure!(saw_data, "corrupt_document: missing worksheet data");
    Ok(())
}

fn slide_text(source: impl Read, slide: usize, out: &mut Output) -> Result<()> {
    let mut xml = Xml::new(source)?;
    let (mut depth, mut body, mut paragraph, mut text_depth) = (0usize, None, None, None);
    let mut saw_body = false;
    let mut number = 0;
    let mut value = String::new();
    while let Some(event) = xml.next()? {
        match event {
            Event::Start(tag) => {
                depth += 1;
                if depth == 2 && tag_is(&tag, PRES, "cSld") {
                    body = Some(depth);
                    saw_body = true;
                } else if body.is_some() && tag_is(&tag, DRAW, "p") && paragraph.is_none() {
                    paragraph = Some(depth);
                    number += 1;
                } else if paragraph.is_some() {
                    if tag_is(&tag, DRAW, "t") {
                        text_depth = Some(depth);
                    } else if tag_is(&tag, DRAW, "br") {
                        add_text(&mut value, "\n")?;
                    }
                }
            }
            Event::Text(s) if text_depth == Some(depth) => add_text(&mut value, &s)?,
            Event::Text(_) => {}
            Event::End => {
                if text_depth == Some(depth) {
                    text_depth = None;
                }
                if paragraph == Some(depth) {
                    out.push(
                        std::mem::take(&mut value),
                        json!({"slide":slide,"paragraph":number}),
                        format!("Slide {slide} · paragraph {number}"),
                        None,
                    )?;
                    paragraph = None;
                }
                if body == Some(depth) {
                    body = None;
                }
                depth -= 1;
            }
        }
    }
    anyhow::ensure!(saw_body, "corrupt_document: missing slide content");
    Ok(())
}
