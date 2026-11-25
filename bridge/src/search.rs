// SPDX-License-Identifier: GPL-3.0-or-later
// File walking, glob filtering and matcher configuration adapted from Clapgrep's
// core/src/search.rs by Leopold Luley and the Clapgrep contributors (see NOTICE.md).

use crate::{
    documents::{pdf, text_stream},
    office, paths,
    protocol::{Emitter, SearchRequest},
    revision, Block, Failure, Reader,
};
use glob::{MatchOptions, Pattern};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};

const MAX_RESULTS: usize = 10_000;

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
    matcher: &'a RegexMatcher,
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
    fn emit<W: Write>(
        &self,
        kind: &str,
        label: &str,
        location: &Value,
        excerpt: Excerpt<'_>,
        sink: &mut Emitter<W>,
    ) -> io::Result<bool> {
        if sink.count == MAX_RESULTS {
            sink.truncated = true;
            return Ok(false);
        }
        sink.send(&MatchEvent {
            r#type: "match",
            identity: &self.identity,
            display_path: self.display_path,
            kind,
            mode: self.mode,
            location,
            label,
            excerpt,
            revision: self.revision,
        })?;
        sink.count += 1;
        Ok(true)
    }

    fn matches<W: Write>(
        &self,
        kind: &str,
        label: &str,
        location: &Value,
        text: &str,
        sink: &mut Emitter<W>,
    ) -> io::Result<()> {
        let mut previous_end = 0;
        let mut utf16_end = 0;
        let location = compact_location(location);
        self.matcher
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

    fn search_content<W: Write>(
        &self,
        path: &Path,
        reader: Reader,
        req: &SearchRequest,
        sink: &mut Emitter<W>,
    ) -> Result<bool, Failure> {
        let mut block_hit = |kind: &str, block: Block| -> anyhow::Result<bool> {
            self.matches(kind, &block.label, &block.location, &block.text, sink)?;
            Ok(!sink.truncated)
        };
        match reader {
            Reader::Skip => Ok(false),
            Reader::Lines(kind) => text_stream::search(
                path,
                kind,
                self.matcher,
                &req.query,
                req.regex,
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
            Reader::Buffered(reader) => {
                let doc = reader.read(path)?;
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

pub(crate) fn run<W: Write>(req: SearchRequest, sink: &mut Emitter<W>) -> io::Result<()> {
    if req.query.is_empty() {
        return sink.error(Failure::new("query", "Enter a search expression"), None);
    }
    let matcher = match RegexMatcherBuilder::new()
        .case_insensitive(!req.case_sensitive)
        .fixed_strings(!req.regex)
        .build(&req.query)
    {
        Ok(matcher) => matcher,
        Err(err) => return sink.error(Failure::new("query", err), None),
    };
    let pattern = match req.path_pattern.as_str() {
        "" => None,
        value => match Pattern::new(value) {
            Ok(pattern) => Some(pattern),
            Err(err) => return sink.error(Failure::new("query", err), None),
        },
    };
    let folder = match paths::decode(&req.folder, req.folder_bytes.as_deref()) {
        Ok(p) => match std::path::absolute(&p) {
            Ok(absolute) => absolute,
            Err(err) => return sink.error(Failure::new("folder", err), Some(&p)),
        },
        Err(err) => return sink.error(Failure::new("folder", err), None),
    };
    if !folder.is_dir() {
        return sink.error(
            Failure::new("folder", "Search folder does not exist or is inaccessible"),
            Some(&folder),
        );
    }
    let walker = WalkBuilder::new(&folder)
        .git_ignore(!req.ignored)
        .git_exclude(!req.ignored)
        .git_global(!req.ignored)
        .ignore(!req.ignored)
        .hidden(!req.hidden)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    let glob_options = MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let mut last_progress = Instant::now();
    for item in walker {
        let entry = match item {
            Ok(entry) => entry,
            Err(err) => {
                sink.error(Failure::new("walk", err), None)?;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|ty| ty.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(&folder).unwrap_or(path);
        if !matches_path(pattern.as_ref(), relative, glob_options) {
            continue;
        }
        sink.files += 1;
        if sink.files == 1 || last_progress.elapsed() >= Duration::from_millis(100) {
            sink.send(&json!({"type":"progress", "files":sink.files, "documentsScanned":sink.documents_scanned}))?;
            last_progress = Instant::now();
        }
        let reader = Reader::for_path(path, &req.mode);
        if matches!(reader, Reader::Skip) && !req.filenames {
            continue;
        }
        let display_path = relative.to_string_lossy();
        let stamp = revision(path);
        let file = FileSearch {
            identity: paths::identity(path),
            display_path: &display_path,
            mode: &req.mode,
            matcher: &matcher,
            revision: stamp.as_ref().ok().map(String::as_str),
        };
        if req.filenames {
            let name = entry.file_name().to_string_lossy();
            file.matches(
                "filename",
                "Filename",
                &json!({"index":0,"filename":true}),
                &name,
                sink,
            )?;
            if sink.truncated {
                break;
            }
        }
        if matches!(reader, Reader::Skip) {
            continue;
        }
        if let Err(err) = stamp {
            sink.error(err, Some(path))?;
            continue;
        }
        let before_content = sink.count;
        let outcome = file.search_content(path, reader, &req, sink);
        let outcome = if revision(path).as_ref().ok().map(String::as_str) != file.revision {
            Err(Failure::new("changed", "File changed while being read"))
        } else {
            outcome
        };
        // Results stream immediately. Withdraw provisional content hits if a later
        // read fails or the file changes; independent filename hits remain valid.
        if !matches!(outcome, Ok(true)) {
            if sink.count > before_content {
                #[derive(Serialize)]
                struct Discard<'a> {
                    r#type: &'static str,
                    #[serde(flatten)]
                    identity: &'a paths::Identity,
                }
                sink.send(&Discard {
                    r#type: "discard",
                    identity: &file.identity,
                })?;
                sink.count = before_content;
            }
            sink.truncated = false;
        }
        match outcome {
            Err(err) => sink.error(err, Some(path))?,
            Ok(true) if !sink.truncated => sink.documents_scanned += 1,
            _ => {}
        }
        if sink.truncated {
            break;
        }
    }
    Ok(())
}

fn matches_path(pattern: Option<&Pattern>, path: &Path, options: MatchOptions) -> bool {
    // Display strings can contain replacement characters for non-UTF-8 Unix
    // filenames. The file is always opened using its original native path.
    pattern.is_none_or(|p| p.matches_with(&path.to_string_lossy(), options))
}
