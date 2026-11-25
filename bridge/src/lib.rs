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
            "location",
        ];
        let prefix = message.split_once(':').map(|(code, _)| code);
        let code = known.into_iter().find(|c| Some(*c) == prefix).unwrap_or(
            if error.is::<std::io::Error>() {
                "read"
            } else {
                "format"
            },
        );
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

/// Choose a reader before opening a file or applying a format-specific size limit.
enum Reader {
    Skip,
    Lines(&'static str),
    Pdf,
    Docx,
    Buffered(BufferedReader),
}

enum BufferedReader {
    Html,
    Markdown,
    Office(String),
}

impl Reader {
    fn for_path(path: &Path, mode: &str) -> Self {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match extension.as_str() {
            "pdf" => Self::Pdf,
            "docx" => Self::Docx,
            "xlsx" | "pptx" | "odt" | "ods" | "odp" | "fodt" | "doc" | "rtf" => {
                Self::Buffered(BufferedReader::Office(extension))
            }
            "html" | "htm" | "xhtml" if mode == "document" => Self::Buffered(BufferedReader::Html),
            "html" | "htm" | "xhtml" => Self::Lines("html"),
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" if mode == "document" => {
                Self::Buffered(BufferedReader::Markdown)
            }
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Self::Lines("markdown"),
            // Formats without a text reader. Ambiguous extensions and extensionless
            // files go to the buffered text probe, so scripts and logs stay searchable.
            "7z" | "zip" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2" | "xz" | "txz"
            | "zst" | "lz4" | "cab" | "iso" | "dmg" | "deb" | "rpm" | "pkg" | "apk" | "jar"
            | "epub" | "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico"
            | "icns" | "heic" | "heif" | "avif" | "jxl" | "psd" | "mp4" | "m4v" | "mkv" | "mov"
            | "avi" | "wmv" | "webm" | "mpg" | "mpeg" | "mp3" | "m4a" | "aac" | "flac" | "ogg"
            | "opus" | "wav" | "wma" | "exe" | "dll" | "dylib" | "so" | "o" | "a" | "class"
            | "pyc" | "wasm" | "ttf" | "otf" | "woff" | "woff2" | "bin" | "sqlite" | "sqlite3" => {
                Self::Skip
            }
            _ => Self::Lines("text"),
        }
    }
}

impl BufferedReader {
    fn read(&self, path: &Path) -> Result<Document, Failure> {
        let doc = match self {
            Self::Html => documents::read_html(path).map_err(Failure::decoder)?,
            Self::Markdown => documents::read_markdown(path).map_err(Failure::decoder)?,
            Self::Office(ext) => office::extract(path, ext)
                .map_err(Failure::decoder)?
                .ok_or_else(|| {
                    Failure::new("unsupported_document", "Unsupported Office document")
                })?,
        };
        if !matches!(self, Self::Html) {
            let bytes = doc
                .blocks
                .iter()
                .fold(0usize, |size, b| size.saturating_add(b.text.len()));
            if bytes > MAX_TEXT_BYTES || doc.blocks.len() > MAX_BLOCKS {
                return Err(Failure::new(
                    "limit",
                    "Extracted document exceeds the text or block limit",
                ));
            }
        }
        if !doc
            .blocks
            .iter()
            .enumerate()
            .all(|(i, b)| b.location["index"].as_u64() == Some(i as u64))
        {
            return Err(Failure::new("format", "Invalid document block location"));
        }
        Ok(doc)
    }
}
