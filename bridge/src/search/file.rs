// SPDX-License-Identifier: GPL-3.0-or-later
// File walking, glob filtering and matcher configuration adapted from Clapgrep's
// core/src/search.rs by Leopold Luley and the Clapgrep contributors (see NOTICE.md).

use crate::{
    documents::{pdf, text_stream},
    office, paths,
    protocol::SearchRequest,
    revision, Block, Failure, Reader,
};

use grep_matcher::Matcher;

use super::pool::Event;
use super::SearchQuery;
use serde::Serialize;
use serde_json::{json, Value};
use std::{io, path::Path, sync::mpsc::SyncSender};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MatchEvent<'a> {
    r#type: &'static str,
    #[serde(flatten)]
    identity: &'a paths::Identity,
    display_path: &'a str,
    kind: &'a str,
    mode: &'a str,
    location: &'a Value,
    label: &'a str,
    #[serde(flatten)]
    excerpt: Excerpt<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Excerpt<'a> {
    text: &'a str,
    start: usize,
    length: usize,
    block_start: usize,
    block_length: usize,
}

struct FileSearch<'a> {
    identity: paths::Identity,
    display_path: &'a str,
    mode: &'a str,
    query: &'a SearchQuery,
    revision: Option<&'a str>,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn compact_location(location: &Value) -> Value {
    let Some(object) = location.as_object() else {
        return location.clone();
    };
    Value::Object(
        object
            .iter()
            // PDF word geometry belongs to the selected preview, not every search hit.
            .filter(|(key, _)| !matches!(key.as_str(), "words" | "boxes" | "geometry"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

impl FileSearch<'_> {
    fn emit(
        &self,
        kind: &str,
        label: &str,
        location: &Value,
        excerpt: Excerpt<'_>,
        output: &FileOutput<'_>,
    ) -> io::Result<bool> {
        if super::stopped() {
            return Ok(false);
        }
        let event = MatchEvent {
            r#type: "match",
            identity: &self.identity,
            display_path: self.display_path,
            kind,
            mode: self.mode,
            location,
            label,
            excerpt,
            revision: self.revision,
        };
        let payload = serde_json::to_vec(&event)?;
        output
            .events
            .send(Event::Hit {
                slot: output.slot,
                filename: kind == "filename",
                payload,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "search output closed"))?;
        Ok(true)
    }

    fn matches(
        &self,
        kind: &str,
        label: &str,
        location: &Value,
        text: &str,
        sink: &FileOutput,
    ) -> io::Result<()> {
        let mut previous_end = 0;
        let mut utf16_end = 0;
        let location = compact_location(location);
        self.query
            .matcher
            .try_find_iter(text.as_bytes(), |m| {
                let (start, end) = (m.start(), m.end());
                // Explicit byte-mode regexes may split an encoded Unicode character.
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    return Ok(true);
                }
                let block_start = utf16_end + utf16_len(&text[previous_end..start]);
                let block_length = utf16_len(&text[start..end]);
                previous_end = end;
                utf16_end = block_start + block_length;
                let left = text[..start]
                    .char_indices()
                    .rev()
                    .nth(79)
                    .map_or(0, |(i, _)| i);
                let right = text[start..]
                    .char_indices()
                    .nth(240)
                    .map_or(text.len(), |(i, _)| start + i);
                self.emit(
                    kind,
                    label,
                    &location,
                    Excerpt {
                        text: &text[left..right],
                        start: utf16_len(&text[left..start]),
                        length: utf16_len(&text[start..end.min(right)]),
                        block_start,
                        block_length,
                    },
                    sink,
                )
            })
            .map_err(io::Error::other)?
    }

    fn search_content(
        &self,
        path: &Path,
        reader: Reader,
        sink: &FileOutput,
    ) -> Result<bool, Failure> {
        let block_hit = |kind: &str, block: Block| -> anyhow::Result<bool> {
            self.matches(kind, &block.label, &block.location, &block.text, sink)?;
            Ok(!super::stopped())
        };
        match reader {
            Reader::Skip => Ok(false),
            Reader::Lines(kind) => text_stream::search(
                path,
                kind,
                &self.query.matcher,
                self.query.window_width,
                |location, text, start, length, block_start, block_length| {
                    let label = format!("Line {}", location["line"]);
                    Ok(self.emit(
                        kind,
                        &label,
                        location,
                        Excerpt {
                            text,
                            start,
                            length,
                            block_start,
                            block_length,
                        },
                        sink,
                    )?)
                },
            )
            .map_err(Failure::decoder),
            Reader::Pdf => {
                let mut has_text = false;
                pdf::scan(path, |block| {
                    has_text |= !block.text.is_empty();
                    block_hit("pdf", block)
                })
                .map_err(Failure::decoder)?;
                Ok(has_text)
            }
            Reader::Docx => {
                office::visit_docx(path, |block| block_hit("docx", block))
                    .map_err(Failure::decoder)?;
                Ok(true)
            }
            Reader::Markdown => {
                crate::documents::visit_markdown(path, |block| block_hit("markdown", block))
                    .map_err(Failure::decoder)?;
                Ok(true)
            }
            Reader::Office(kind) => {
                office::visit_matching(path, kind, &self.query.matcher, |block| {
                    block_hit(kind, block)
                })
                .map_err(Failure::decoder)?;
                Ok(true)
            }
            Reader::Html => {
                let doc = crate::html_document(path)?;
                for block in doc.blocks {
                    if !block_hit(&doc.kind, block).map_err(Failure::decoder)? {
                        break;
                    }
                }
                Ok(true)
            }
        }
    }
}

struct FileOutput<'a> {
    slot: usize,
    events: &'a SyncSender<Event>,
}

pub(super) fn scan(
    slot: usize,
    path: &Path,
    root: &Path,
    request: &SearchRequest,
    query: &SearchQuery,
    events: &SyncSender<Event>,
) -> Result<bool, Failure> {
    let stamp = revision(path);
    let current = stamp.as_ref().ok().cloned();
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let search = FileSearch {
        identity: paths::identity(path),
        display_path: &relative,
        mode: &request.mode,
        query,
        revision: current.as_deref(),
    };
    let output = FileOutput { slot, events };
    if request.filenames {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        search
            .matches(
                "filename",
                "Filename",
                &json!({"index":0,"filename":true}),
                &name,
                &output,
            )
            .map_err(|e| Failure::new("read", e))?;
    }
    let reader = Reader::for_path(path, &request.mode);
    if matches!(reader, Reader::Skip) {
        return Ok(false);
    }
    stamp?;
    let outcome = search.search_content(path, reader, &output);
    if revision(path).ok() != current {
        return Err(Failure::new("changed", "File changed while being read"));
    }
    outcome
}
