// SPDX-License-Identifier: GPL-3.0-or-later
use super::package::{parse, Package, Rel};
use super::{cell_name, name, push_cell, Output, MAX_COLS, MAX_ROWS, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use roxmltree::{Document, Node};
use serde_json::json;
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

fn strings(n: Node<'_, '_>) -> String {
    let mut s = String::new();
    for el in n.children().filter(|n| n.is_element()) {
        if ss(el, "t") {
            s.push_str(el.text().unwrap_or(""))
        } else if ss(el, "r") {
            for t in el.children().filter(|&n| ss(n, "t")) {
                s.push_str(t.text().unwrap_or(""));
            }
        }
    }
    s
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
    cell_name(col, row)?;
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
    let mut shared = Vec::new();
    if let Some(rel) = rels.values().find(|r| r.kind == "sharedStrings") {
        let data = pkg.required(internal(rel)?)?;
        let parsed = parse(&data)?;
        for n in parsed.root_element().children().filter(|&n| ss(n, "si")) {
            shared.push(strings(n));
        }
    }
    for sheet in sheets.children().filter(|&n| ss(n, "sheet")) {
        let sheet_name = sheet
            .attribute("name")
            .ok_or_else(|| anyhow!("corrupt_document: unnamed sheet"))?;
        let rel = related(rels, rid(sheet))?;
        if rel.kind != "worksheet" {
            continue;
        }
        let part = pkg.required(internal(rel)?)?;
        let parsed = parse(&part)?;
        let data = child(parsed.root_element(), SHEET, "sheetData")
            .ok_or_else(|| anyhow!("corrupt_document: missing worksheet data"))?;
        let mut rownum = 0usize;
        for row in data.children().filter(|&n| ss(n, "row")) {
            rownum = match row.attribute("r") {
                Some(s) => s
                    .parse()
                    .map_err(|_| anyhow!("corrupt_document: invalid sheet row"))?,
                None => rownum + 1,
            };
            if rownum > MAX_ROWS {
                bail!("resource_limit: spreadsheet row");
            }
            let mut col = 0;
            for c in row.children().filter(|&n| ss(n, "c")) {
                let (r, cc) = if let Some(v) = c.attribute("r") {
                    coordinate(v)?
                } else {
                    (rownum, col + 1)
                };
                col = cc;
                let stored = child(c, SHEET, "v").and_then(|n| n.text());
                let value = match c.attribute("t") {
                    Some("inlineStr") => child(c, SHEET, "is").map(strings).unwrap_or_default(),
                    Some("s") => {
                        if let Some(v) = stored {
                            v.trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| shared.get(i))
                                .cloned()
                                .ok_or_else(|| {
                                    anyhow!("corrupt_document: invalid shared string index")
                                })?
                        } else {
                            String::new()
                        }
                    }
                    Some("b") => match stored {
                        Some("1") => String::from("TRUE"),
                        Some("0") => String::from("FALSE"),
                        _ => stored.unwrap_or("").into(),
                    },
                    _ => stored.unwrap_or("").into(),
                };
                push_cell(out, sheet_name, r, cc, value)?;
            }
        }
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
        let part = pkg.required(internal(rel)?)?;
        let doc = parse(&part)?;
        let body = child(doc.root_element(), PRES, "cSld")
            .ok_or_else(|| anyhow!("corrupt_document: missing slide content"))?;
        for (idx, p) in body.descendants().filter(|&n| is(n, DRAW, "p")).enumerate() {
            let mut text = String::new();
            for n in p.descendants() {
                if is(n, DRAW, "t") {
                    text.push_str(n.text().unwrap_or(""));
                } else if is(n, DRAW, "br") {
                    text.push('\n');
                }
            }
            if text.len() > MAX_TEXT_BYTES {
                bail!("resource_limit: slide paragraph");
            }
            let paragraph = idx + 1;
            out.push(
                text,
                json!({"slide":slide,"paragraph":paragraph}),
                format!("Slide {slide} · paragraph {paragraph}"),
                None,
            )?;
        }
    }
    Ok(())
}
