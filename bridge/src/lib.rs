// SPDX-License-Identifier: GPL-3.0-or-later

mod converters;
pub mod documents;
pub mod office;
mod paths;
pub mod protocol;
mod search;

use serde::Serialize;
use serde_json::Value;
use std::{fs, path::Path};

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_BLOCKS: usize = 200_000;

#[derive(Debug, Serialize)]
pub struct Block {
    pub text: String,
    pub location: Value,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

impl Block {
    pub fn new(text: impl Into<String>, location: Value, label: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            location,
            label: label.into(),
            html: None,
        }
    }

    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }
}

#[derive(Debug)]
pub struct Document {
    pub kind: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    fn decoder(error: anyhow::Error) -> Self {
        let message = format!("{error:#}");
        let known = [
            "read",
            "limit",
            "tool_missing",
            "protected",
            "no_text_layer",
            "too_large",
            "unsupported_encoding",
            "invalid_document",
            "corrupt_document",
            "unsupported_document",
            "encrypted_document",
            "resource_limit",
            "missing_decoder",
            "unreadable_document",
        ];
        let prefix = message.split_once(':').map(|(code, _)| code);
        let code = known
            .into_iter()
            .find(|c| Some(*c) == prefix)
            .unwrap_or("format");
        Self::new(code, message)
    }
}

fn revision(path: &Path) -> Result<String, Failure> {
    let meta = fs::metadata(path).map_err(|e| Failure::new("read", e))?;
    if !meta.is_file() {
        return Err(Failure::new("read", "Expected a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}",
            meta.len(),
            meta.dev(),
            meta.ino(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec()
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}",
            meta.file_size(),
            meta.last_write_time(),
            meta.creation_time()
        ))
    }
}

fn extract(path: &Path, mode: &str) -> Result<Option<Document>, Failure> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let large_html = mode == "document" && documents::is_html(&extension);
    if !large_html
        && fs::metadata(path)
            .map_err(|e| Failure::new("read", e))?
            .len()
            > MAX_FILE_BYTES
    {
        return Err(Failure::new("limit", "File exceeds the 64 MiB input limit"));
    }
    let document = match office::extract(path, &extension).map_err(Failure::decoder)? {
        Some(document) => Some(document),
        None => documents::extract(path, &extension, mode).map_err(Failure::decoder)?,
    };
    if let Some(ref doc) = document {
        let bytes = doc
            .blocks
            .iter()
            .fold(0usize, |size, block| size.saturating_add(block.text.len()));
        if !large_html && (bytes > MAX_TEXT_BYTES || doc.blocks.len() > MAX_BLOCKS) {
            return Err(Failure::new(
                "limit",
                "Extracted document exceeds the text or block limit",
            ));
        }
        if !doc
            .blocks
            .iter()
            .enumerate()
            .all(|(i, b)| b.location["index"].as_u64() == Some(i as u64))
        {
            return Err(Failure::new("format", "Invalid document block location"));
        }
    }
    Ok(document)
}
