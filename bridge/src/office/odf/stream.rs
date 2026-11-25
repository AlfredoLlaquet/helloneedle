// SPDX-License-Identifier: GPL-3.0-or-later
//! Retain only the style table and the paragraph or physical table row being
//! projected. Repeated coordinates are expanded from that row on demand.
use super::*;
use crate::office::stream_xml::{Element, Event, Xml};
use anyhow::ensure;
use std::io::Read;

#[derive(PartialEq, Eq)]
enum Frame {
    Root,
    Body,
    Content,
    Other,
    Ignored,
    Table,
    Slide,
}

fn excluded(tag: &Element) -> bool {
    tag.is(OFFICE, "annotation")
        || tag.is(TEXT, "tracked-changes")
        || tag.is(TEXT, "note")
        || tag.is(PRESENTATION, "notes")
}

pub(in crate::office) fn extract(
    input: impl Read,
    external_styles: Option<&str>,
    kind: &str,
    out: &mut Output,
) -> Result<()> {
    let mut styles_map = Styles::default();
    if let Some(s) = external_styles {
        styles(parse(s)?.root_element(), &mut styles_map)?;
    }
    let expected = match kind {
        "ods" => "spreadsheet",
        "odp" => "presentation",
        _ => "text",
    };
    let mut xml = Xml::new(input)?;
    let mut frames = Vec::new();
    let mut body_depth = None;
    let mut ignored_depth = None;
    let mut table_depth = None;
    let mut table_name = String::new();
    let mut table_number = 0;
    let mut last_row = 0usize;
    let mut slide_depth = None;
    let mut slide_number = 0usize;
    let mut counts = (0usize, 0usize); // paragraphs, tables
    let mut body_seen = false;
    while let Some(event) = xml.next()? {
        let tag = match event {
            Event::Start(tag) => tag,
            Event::End => {
                let depth = frames.len();
                if ignored_depth == Some(depth) {
                    ignored_depth = None;
                }
                if table_depth == Some(depth) {
                    table_depth = None;
                }
                if slide_depth == Some(depth) {
                    slide_depth = None;
                }
                if body_depth == Some(depth) {
                    body_depth = None;
                }
                frames.pop();
                continue;
            }
            Event::Text(_) => continue,
        };
        let depth = frames.len() + 1;
        let parent = frames.last();
        let frame = if ignored_depth.is_some() {
            Frame::Other
        } else if parent.is_none() {
            ensure!(
                tag.is(OFFICE, "document-content") || tag.is(OFFICE, "document"),
                "corrupt_document: invalid OpenDocument root"
            );
            Frame::Root
        } else if parent == Some(&Frame::Root) && tag.is(OFFICE, "body") {
            Frame::Body
        } else if parent == Some(&Frame::Body) && tag.is(OFFICE, expected) {
            ensure!(!body_seen, "corrupt_document: duplicate OpenDocument body");
            styles_map.resolve()?;
            body_seen = true;
            body_depth = Some(depth);
            Frame::Content
        } else if body_depth.is_none() {
            if !body_seen && tag.is(STYLE, "style") {
                let local = xml.subtree()?;
                styles(parse(&local)?.root_element(), &mut styles_map)?;
                continue;
            }
            Frame::Other
        } else if excluded(&tag) {
            ignored_depth = Some(depth);
            Frame::Ignored
        } else if tag.is(TABLE, "table")
            && (expected == "text"
                || (expected == "spreadsheet" && parent == Some(&Frame::Content)))
        {
            if table_depth.is_some() {
                ignored_depth = Some(depth);
                Frame::Ignored
            } else {
                table_depth = Some(depth);
                last_row = 0;
                counts.1 = advance(counts.1, 1)?;
                table_number = counts.1;
                if expected == "spreadsheet" {
                    table_name = tag
                        .attr(TABLE, "name")
                        .ok_or_else(|| anyhow!("corrupt_document: unnamed sheet"))?
                        .to_owned();
                }
                Frame::Table
            }
        } else if table_depth.is_some() && tag.is(TABLE, "table-row") {
            let local = xml.subtree()?;
            let doc = parse(&local)?;
            if expected == "spreadsheet" {
                spreadsheet_row(
                    doc.root_element(),
                    &table_name,
                    &mut last_row,
                    &styles_map,
                    out,
                )?;
            } else {
                writer_row(
                    doc.root_element(),
                    table_number,
                    &mut last_row,
                    &styles_map,
                    &mut counts,
                    out,
                    1,
                )?;
            }
            continue;
        } else if expected == "presentation"
            && parent == Some(&Frame::Content)
            && tag.is(DRAW, "page")
        {
            slide_depth = Some(depth);
            slide_number = advance(slide_number, 1)?;
            counts.0 = 0;
            Frame::Slide
        } else if table_depth.is_none()
            && (expected == "text" || slide_depth.is_some())
            && (tag.is(TEXT, "h") || tag.is(TEXT, "p"))
        {
            let fragment = xml.subtree()?;
            let doc = parse(&fragment)?;
            let root = doc.root_element();
            counts.0 = advance(counts.0, 1)?;
            let h = heading(root);
            let (text, html) = paragraph(root, &styles_map)?.finish(h);
            let (location, label) = if slide_depth.is_some() {
                let mut location = json!({"slide":slide_number,"paragraph":counts.0});
                if let Some(level) = h {
                    location["heading_level"] = json!(level);
                }
                (
                    location,
                    format!("Slide {slide_number} · paragraph {}", counts.0),
                )
            } else {
                paragraph_location(counts.0, h, None)
            };
            out.push(text, location, label, Some(html))?;
            continue;
        } else {
            Frame::Other
        };
        frames.push(frame);
    }
    ensure!(
        body_seen,
        "corrupt_document: missing OpenDocument {expected} body"
    );
    Ok(())
}
