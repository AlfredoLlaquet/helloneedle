// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{extract, paths, revision, search, Block, Failure};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    path::Path,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchRequest {
    #[serde(default)]
    pub folder: String,
    pub folder_bytes: Option<String>,
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
    revision: Option<String>,
}

fn document_mode() -> String {
    "document".into()
}

fn valid_mode(mode: &str) -> bool {
    matches!(mode, "source" | "document")
}

pub(crate) struct Emitter<W> {
    pub count: usize,
    pub files: usize,
    pub errors: usize,
    pub truncated: bool,
    writer: W,
}

impl<W: Write> Emitter<W> {
    pub fn send<T: Serialize>(&mut self, event: &T) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, event)?;
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
                         "errors":self.errors,"truncated":self.truncated}))
    }
}

fn context_range(blocks: &[Block], index: usize, kind: &str) -> std::ops::Range<usize> {
    if kind == "pdf" {
        return index..index + 1;
    }
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

fn preview<W: Write>(request: PreviewRequest, events: &mut Emitter<W>) -> io::Result<()> {
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
    if request.revision.is_some_and(|wanted| wanted != before) {
        return events.error(
            Failure::new("changed", "File changed since the search"),
            Some(&path),
        );
    }
    let doc = match extract(&path, &request.mode) {
        Ok(Some(doc)) => doc,
        Ok(None) => {
            return events.error(
                Failure::new(
                    "unsupported_document",
                    "File has no supported text representation",
                ),
                Some(&path),
            )
        }
        Err(err) => return events.error(err, Some(&path)),
    };
    if revision(&path).as_ref().ok() != Some(&before) {
        return events.error(
            Failure::new("changed", "File changed while being read"),
            Some(&path),
        );
    }
    let mut index = match request.location.get("index") {
        None => 0,
        Some(value) => match value.as_u64().and_then(|n| usize::try_from(n).ok()) {
            Some(n) => n,
            None => {
                return events.error(Failure::new("location", "Invalid block index"), Some(&path))
            }
        },
    };
    if doc.kind == "pdf" {
        if let Some(page) = request.location.get("page") {
            if doc
                .blocks
                .get(index)
                .and_then(|block| block.location.get("page"))
                != Some(page)
            {
                let Some(first_on_page) = doc
                    .blocks
                    .iter()
                    .position(|block| block.location.get("page") == Some(page))
                else {
                    return events.error(
                        Failure::new("location", "Page is outside the document"),
                        Some(&path),
                    );
                };
                index = first_on_page;
            }
        }
    }
    if index >= doc.blocks.len() && !(doc.blocks.is_empty() && index == 0) {
        return events.error(
            Failure::new("location", "Block index is outside the document"),
            Some(&path),
        );
    }
    let range = if doc.blocks.is_empty() {
        0..0
    } else {
        context_range(&doc.blocks, index, &doc.kind)
    };
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Preview<'a> {
        r#type: &'static str,
        kind: String,
        mode: String,
        index: usize,
        total_blocks: usize,
        blocks: &'a [Block],
        revision: String,
    }
    events.files = 1;
    events.send(&Preview {
        r#type: "preview",
        kind: doc.kind,
        mode: request.mode,
        index: index - range.start,
        total_blocks: doc.blocks.len(),
        blocks: &doc.blocks[range],
        revision: before,
    })
}

/// Runs one request, streaming complete NDJSON events in emission order.
pub fn run<R: Read, W: Write>(command: &str, input: R, output: W) -> io::Result<()> {
    let mut events = Emitter {
        writer: output,
        count: 0,
        files: 0,
        errors: 0,
        truncated: false,
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
    fn hidden_ignored_glob_and_filename_controls_work_outside_git() {
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
        let mut req = json!({"folder":dir.0, "query":"needle", "pathPattern":"*.txt"});
        let limited = events("search", req.clone());
        assert_eq!(limited.last().unwrap()["count"], 1);
        for key in ["hidden", "ignored", "filenames"] {
            req[key] = json!(true);
        }
        let full = events("search", req);
        assert_eq!(full.last().unwrap()["count"], 4);
        assert!(full
            .iter()
            .any(|r| r["kind"] == "filename" && r["location"]["filename"] == true));
    }
}
