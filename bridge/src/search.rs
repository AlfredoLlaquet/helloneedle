// SPDX-License-Identifier: GPL-3.0-or-later
// File walking, glob filtering and matcher configuration adapted from Clapgrep's
// core/src/search.rs by Leopold Luley and the Clapgrep contributors (see NOTICE.md).

use crate::{
    extract, paths,
    protocol::{Emitter, SearchRequest},
    revision, Failure,
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
    text: &'a str,
    start: usize,
    length: usize,
    block_start: usize,
    block_length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<&'a str>,
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
        let mut write_error = None;
        let location = compact_location(location);
        self.matcher
            .find_iter(text.as_bytes(), |m| {
                let (start, end) = (m.start(), m.end());
                // Explicit byte-mode regexes may split an encoded Unicode character.
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    return true;
                }
                if sink.count == MAX_RESULTS {
                    sink.truncated = true;
                    return false;
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
                let event = MatchEvent {
                    r#type: "match",
                    identity: &self.identity,
                    display_path: self.display_path,
                    kind,
                    mode: self.mode,
                    location: &location,
                    label,
                    text: &text[left..right],
                    start: utf16_len(&text[left..start]),
                    length: utf16_len(&text[start..end.min(right)]),
                    block_start,
                    block_length,
                    revision: self.revision,
                };
                match sink.send(&event) {
                    Ok(()) => {
                        sink.count += 1;
                        true
                    }
                    Err(err) => {
                        write_error = Some(err);
                        false
                    }
                }
            })
            .map_err(io::Error::other)?;
        match write_error {
            Some(err) => Err(err),
            None => Ok(()),
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
        if let Err(err) = stamp {
            sink.error(err, Some(path))?;
            continue;
        }
        let doc = match extract(path, &req.mode) {
            Ok(Some(doc)) => doc,
            Ok(None) => continue,
            Err(err) => {
                sink.error(err, Some(path))?;
                continue;
            }
        };
        if revision(path).as_ref().ok().map(String::as_str) != file.revision {
            sink.error(
                Failure::new("changed", "File changed while being read"),
                Some(path),
            )?;
            continue;
        }
        for block in doc.blocks {
            file.matches(&doc.kind, &block.label, &block.location, &block.text, sink)?;
            if sink.truncated {
                break;
            }
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
