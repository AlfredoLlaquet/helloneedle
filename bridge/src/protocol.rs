// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{documents, office, paths, revision, search, Block, Document, Failure, Reader};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    path::Path,
};
mod match_copy;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchRequest {
    #[serde(default)]
    pub folder: String,
    pub folder_bytes: Option<String>,
    #[serde(default)]
    pub search_workers: usize,
    #[serde(default)]
    pub pdf_conversions: usize,
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub exclude_subdirectories: bool,
    #[serde(default)]
    pub filenames: bool,
    #[serde(default)]
    pub path_pattern: String,
    #[serde(default = "document_mode")]
    pub mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRequest {
    #[serde(default)]
    path: String,
    path_bytes: Option<String>,
    #[serde(default = "document_mode")]
    mode: String,
    #[serde(default)]
    location: Value,
    #[serde(default)]
    block_start: usize,
    #[serde(default)]
    block_length: usize,
    #[serde(default)]
    copy_match: bool,
    revision: Option<String>,
}

fn document_mode() -> String {
    "document".into()
}

fn valid_mode(mode: &str) -> bool {
    matches!(mode, "source" | "document")
}

pub(crate) struct Emitter<W> {
    pub count: u64,
    pub files: u64,
    pub documents_scanned: u64,
    pub errors: u64,
    writer: W,
}

impl<W: Write> Emitter<W> {
    pub fn send<T: Serialize>(&mut self, event: &T) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, event)?;
        self.end_line()
    }

    pub fn send_serialized(&mut self, event: &[u8]) -> io::Result<()> {
        self.writer.write_all(event)?;
        self.writer.write_all(b"\n")
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn end_line(&mut self) -> io::Result<()> {
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    pub fn error(&mut self, error: Failure, path: Option<&Path>) -> io::Result<()> {
        #[derive(Serialize)]
        struct ErrorEvent {
            r#type: &'static str,
            code: &'static str,
            message: String,
            #[serde(flatten)]
            file: Option<paths::Identity>,
        }
        self.errors += 1;
        self.send(&ErrorEvent {
            r#type: "error",
            code: error.code,
            message: error.message,
            file: path.map(paths::identity),
        })
    }

    fn done(&mut self) -> io::Result<()> {
        self.send(&json!({"type":"done","count":self.count,"files":self.files,
                         "documentsScanned":self.documents_scanned,
                         "errors":self.errors,"truncated":false}))
    }
}

fn context_range(blocks: &[Block], index: usize) -> std::ops::Range<usize> {
    let mut first = index.saturating_sub(12);
    let mut end = (index + 13).min(blocks.len());
    let item = &blocks[index].location;
    let same_group = |candidate: &Value| {
        (item.get("slide").is_some() && item["slide"] == candidate["slide"])
            || (item.get("row").is_some()
                && item.get("sheet").is_some()
                && item["row"] == candidate["row"]
                && item["sheet"] == candidate["sheet"])
    };
    while first > 0 && index - first < 64 && same_group(&blocks[first - 1].location) {
        first -= 1;
    }
    while end < blocks.len() && end - index < 65 && same_group(&blocks[end].location) {
        end += 1;
    }
    first..end
}

// A selected HTML paragraph can now exceed the desktop's entire protocol frame.
// Keep ordinary previews rich. For a giant paragraph, send the text around the
// selected occurrence and its original UTF-16 offset for Qt's highlighter.
pub(crate) fn clip_paragraph(block: &mut Block, focus: usize, length: usize) {
    block.html = None;
    const NEARBY_CHARS: usize = 8192;
    if block.text.len() <= 2 * NEARBY_CHARS {
        return;
    }
    let from = focus.saturating_sub(NEARBY_CHARS);
    let until = focus.saturating_add(length).saturating_add(NEARBY_CHARS);
    let mut units = 0;
    let mut left = None;
    let mut right = block.text.len();
    for (byte, ch) in block.text.char_indices() {
        if units >= until {
            right = byte;
            break;
        }
        if left.is_none() && units + ch.len_utf16() > from {
            left = Some((byte, units));
        }
        units += ch.len_utf16();
    }
    let (left, offset) = left.unwrap_or((right, units));
    block.location["textOffset"] = json!(offset);
    block.text = block.text[left..right].to_owned();
}

fn html_context(
    blocks: &mut [Block],
    index: usize,
    focus: usize,
    length: usize,
    mut range: std::ops::Range<usize>,
) -> std::ops::Range<usize> {
    // Even the worst-case JSON escaping of 8 MiB fits a 64 MiB frame.
    const MAX_CONTEXT: usize = 8 * 1024 * 1024;
    let size = |b: &Block| b.text.len() + b.html.as_ref().map_or(0, String::len);
    if size(&blocks[index]) > MAX_CONTEXT {
        clip_paragraph(&mut blocks[index], focus, length);
    }
    let mut total: usize = blocks[range.clone()].iter().map(size).sum();
    while total > MAX_CONTEXT {
        if index - range.start > range.end - 1 - index {
            total -= size(&blocks[range.start]);
            range.start += 1;
        } else {
            range.end -= 1;
            total -= size(&blocks[range.end]);
        }
    }
    range
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBlocks {
    kind: String,
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_blocks: Option<usize>,
    blocks: Vec<Block>,
}

fn buffered_preview(
    mut doc: Document,
    index: usize,
    focus: usize,
    length: usize,
) -> Result<PreviewBlocks, Failure> {
    if index >= doc.blocks.len() && !(doc.blocks.is_empty() && index == 0) {
        return Err(Failure::new(
            "location",
            "Block index is outside the document",
        ));
    }
    let mut range = if doc.blocks.is_empty() {
        0..0
    } else {
        context_range(&doc.blocks, index)
    };
    if doc.kind == "html" && !range.is_empty() {
        range = html_context(&mut doc.blocks, index, focus, length, range);
    }
    Ok(PreviewBlocks {
        kind: doc.kind,
        index: index - range.start,
        total_blocks: Some(doc.blocks.len()),
        blocks: doc.blocks.drain(range).collect(),
    })
}

fn load_preview(path: &Path, req: &PreviewRequest) -> Result<PreviewBlocks, Failure> {
    let index = match req.location.get("index") {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| Failure::new("location", "Invalid block index"))?,
    };
    let (kind, blocks, index, total_blocks) = match Reader::for_path(path, &req.mode) {
        Reader::Html => {
            return buffered_preview(
                crate::html_document(path)?,
                index,
                req.block_start,
                req.block_length,
            )
        }
        Reader::Lines(kind) => {
            let (blocks, selected, total) = documents::text_stream::preview(
                path,
                kind,
                &req.location,
                req.block_start,
                req.block_length,
            )
            .map_err(Failure::decoder)?;
            (kind, blocks, selected, total)
        }
        Reader::Pdf => {
            let (blocks, selected) =
                documents::pdf::preview(path, &req.location, req.block_start, req.block_length)
                    .map_err(Failure::decoder)?;
            ("pdf", blocks, selected, None)
        }
        Reader::Docx => {
            let (blocks, selected, total) =
                office::preview_docx(path, index, req.block_start, req.block_length)
                    .map_err(Failure::decoder)?;
            ("docx", blocks, selected, total)
        }
        Reader::Office(kind) => {
            let (blocks, selected, total) =
                office::preview(path, kind, index, req.block_start, req.block_length)
                    .map_err(Failure::decoder)?;
            (kind, blocks, selected, total)
        }
        Reader::Markdown => {
            let (blocks, selected, total) =
                documents::preview_markdown(path, index, req.block_start, req.block_length)
                    .map_err(Failure::decoder)?;
            ("markdown", blocks, selected, total)
        }
        Reader::Skip => ("filename", Vec::new(), 0, Some(0)),
    };
    Ok(PreviewBlocks {
        kind: kind.into(),
        blocks,
        index,
        total_blocks,
    })
}

fn preview<W: Write>(mut request: PreviewRequest, events: &mut Emitter<W>) -> io::Result<()> {
    if !valid_mode(&request.mode) {
        return events.error(
            Failure::new("request", "Mode must be document or source"),
            None,
        );
    }
    let path = match paths::decode(&request.path, request.path_bytes.as_deref()) {
        Ok(path) => path,
        Err(err) => return events.error(Failure::new("request", err), None),
    };
    let before = match revision(&path) {
        Ok(stamp) => stamp,
        Err(err) => return events.error(err, Some(&path)),
    };
    if request
        .revision
        .as_ref()
        .is_some_and(|wanted| wanted != &before)
    {
        return events.error(
            Failure::new("changed", "File changed since the search"),
            Some(&path),
        );
    }
    if request.copy_match {
        if let Err(err) = match_copy::stream(&path, &request, events) {
            return events.error(Failure::decoder(err), Some(&path));
        }
        if revision(&path).as_ref().ok() != Some(&before) {
            return events.error(
                Failure::new("changed", "File changed while being read"),
                Some(&path),
            );
        }
        events.files = 1;
        return events.send(&json!({"type":"copyReady", "length":request.block_length}));
    }
    // The desktop pages longer selections using blockStart. Each response stays
    // bounded even if a caller asks to preview a multi-gigabyte match at once.
    request.block_length = request.block_length.min(256 * 1024);
    let content = match load_preview(&path, &request) {
        Ok(content) => content,
        Err(err) => return events.error(err, Some(&path)),
    };
    if revision(&path).as_ref().ok() != Some(&before) {
        return events.error(
            Failure::new("changed", "File changed while being read"),
            Some(&path),
        );
    }
    #[derive(Serialize)]
    struct Preview {
        r#type: &'static str,
        mode: String,
        revision: String,
        #[serde(flatten)]
        content: PreviewBlocks,
    }
    events.files = 1;
    events.send(&Preview {
        r#type: "preview",
        mode: request.mode,
        revision: before,
        content,
    })
}

/// Runs one request, streaming complete NDJSON events in emission order.
pub fn run<R: Read, W: Write>(command: &str, input: R, output: W) -> io::Result<()> {
    let mut events = Emitter {
        writer: output,
        count: 0,
        files: 0,
        documents_scanned: 0,
        errors: 0,
    };
    let mut bytes = Vec::new();
    const MAX_REQUEST: u64 = 1024 * 1024;
    input.take(MAX_REQUEST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST {
        events.error(Failure::new("request", "Request exceeds 1 MiB"), None)?;
    } else {
        match command {
            "search" => match serde_json::from_slice::<SearchRequest>(&bytes) {
                Ok(request) if valid_mode(&request.mode) => search::run(request, &mut events)?,
                Ok(_) => events.error(
                    Failure::new("request", "Mode must be document or source"),
                    None,
                )?,
                Err(err) => events.error(Failure::new("request", err), None)?,
            },
            "preview" => match serde_json::from_slice::<PreviewRequest>(&bytes) {
                Ok(request) => preview(request, &mut events)?,
                Err(err) => events.error(Failure::new("request", err), None)?,
            },
            _ => events.error(Failure::new("request", "Expected search or preview"), None)?,
        }
    }
    events.done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "helloneedle-worker-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn events(command: &str, request: Value) -> Vec<Value> {
        let mut output = Vec::new();
        run(command, request.to_string().as_bytes(), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn errors_finish_the_request_and_reject_invalid_mode() {
        for request in [
            json!({"folder":".","query":"[","regex":true}),
            json!({"folder":".","query":"x","mode":"bad"}),
        ] {
            let response = events("search", request);
            assert_eq!(response.len(), 2);
            assert_eq!(response[0]["type"], "error");
            assert_eq!(response[1]["type"], "done");
            assert_eq!(response[1]["errors"], 1);
        }
    }

    #[test]
    fn bounded_excerpts_retain_absolute_utf16_offsets_and_stale_previews_fail() {
        let dir = Folder::new();
        let path = dir.0.join("two repeats.txt");
        let text = format!("{}😀needle and needle", "á".repeat(300));
        fs::write(&path, &text).unwrap();
        let response = events(
            "search",
            json!({"folder":dir.0,"query":"needle","mode":"source"}),
        );
        let hits: Vec<_> = response.iter().filter(|r| r["type"] == "match").collect();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["blockStart"], 302);
        assert_eq!(hits[1]["blockStart"], 313);
        assert_eq!(hits[0]["blockLength"], 6);
        assert!(hits[0]["start"].as_u64().unwrap() < 100);
        let mut preview = json!({"pathBytes":hits[1]["pathBytes"], "mode":"source",
                                 "location":hits[1]["location"],"revision":hits[1]["revision"]});
        assert_eq!(
            events("preview", preview.clone())[0]["blocks"][0]["text"],
            text
        );
        fs::write(path, "replacement").unwrap();
        assert_eq!(events("preview", preview.clone())[0]["code"], "changed");
        preview["location"]["index"] = json!(5);
        preview.as_object_mut().unwrap().remove("revision");
        assert_eq!(events("preview", preview)[0]["code"], "location");
    }

    #[test]
    fn recursive_and_folder_only_searches_honor_filters_outside_git() {
        let dir = Folder::new();
        for name in [
            "shown.txt",
            ".hidden.txt",
            "ignored.txt",
            "shown.md",
            "needle.txt",
        ] {
            fs::write(
                dir.0.join(name),
                if name == "needle.txt" {
                    "other"
                } else {
                    "needle"
                },
            )
            .unwrap();
        }
        fs::write(dir.0.join(".gitignore"), "ignored.txt\n").unwrap();
        let nested = dir.0.join("nested");
        fs::create_dir_all(nested.join("deeper")).unwrap();
        fs::write(nested.join("regular.txt"), "needle").unwrap();
        fs::write(nested.join("deeper/regular.txt"), "needle").unwrap();
        fs::write(nested.join("broken.docx"), "incomplete").unwrap();
        let mut req = json!({"folder":dir.0, "query":"needle", "pathPattern":"*.txt"});
        let recursive = events("search", req.clone());
        assert_eq!(recursive.last().unwrap()["count"], 3);
        assert_eq!(recursive.last().unwrap()["files"], 4);
        req["excludeSubdirectories"] = json!(false);
        assert_eq!(events("search", req.clone()).last(), recursive.last());
        req["excludeSubdirectories"] = json!(true);
        let root_only = events("search", req.clone());
        assert_eq!(root_only.last().unwrap()["count"], 1);
        assert_eq!(root_only.last().unwrap()["files"], 2);
        for key in ["hidden", "ignored", "filenames"] {
            req[key] = json!(true);
        }
        let full = events("search", req.clone());
        assert_eq!(full.last().unwrap()["count"], 4);
        assert_eq!(full.last().unwrap()["files"], 4);
        assert!(full
            .iter()
            .any(|r| r["kind"] == "filename" && r["location"]["filename"] == true));
        req["pathPattern"] = json!("");
        let shallow = events("search", req.clone());
        assert_eq!(shallow.last().unwrap()["errors"], 0);
        req["excludeSubdirectories"] = json!(false);
        let recursive = events("search", req);
        assert_eq!(recursive.last().unwrap()["errors"], 1);
    }

    #[test]
    fn a_file_with_more_than_ten_thousand_occurrences_finishes() {
        let dir = Folder::new();
        fs::write(dir.0.join("a.txt"), "no occurrences").unwrap();
        fs::write(dir.0.join("b.txt"), "x ".repeat(10_001)).unwrap();
        let response = events("search", json!({"folder":dir.0, "query":"x"}));
        assert_eq!(
            response[0],
            json!({"type":"progress","files":1,"documentsScanned":0})
        );
        let done = response.last().unwrap();
        assert_eq!(done["count"], 10_001);
        assert_eq!(done["files"], 2);
        assert_eq!(done["documentsScanned"], 2);
        assert_eq!(done["truncated"], false);
    }
}
