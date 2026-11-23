// SPDX-License-Identifier: GPL-3.0-or-later
//! Poppler supplies reading order and word boxes from the same text layer.

use super::MAX_BLOCKS;
use crate::{Block, Document, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, ensure, Context, Result};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use serde_json::{json, Value};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_WORDS: usize = 100_000;
const MAX_PAGES: usize = 8_192;
const DECODE_TIMEOUT: Duration = Duration::from_secs(45);

pub(super) fn read(path: &Path) -> Result<Document> {
    // A file named "-something.pdf" must remain a path, including when the caller used a relative name.
    let path = std::path::absolute(path)?;
    let mut command = Command::new(poppler_tool());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for a GUI-launched decoder
    }
    command
        .args(["-bbox-layout", "-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .env("LC_ALL", "C");
    let (status, output, diagnostics) = run(&mut command)?;
    if !status.success() {
        let diagnostics = String::from_utf8_lossy(&diagnostics).to_ascii_lowercase();
        if status.code() == Some(3)
            || diagnostics.contains("password")
            || diagnostics.contains("permission")
        {
            bail!("protected: PDF text is password-protected or extraction is restricted");
        }
        bail!("invalid_document: Poppler could not read the PDF");
    }
    parse(&output)
}

fn poppler_tool() -> PathBuf {
    let name = if cfg!(windows) {
        "pdftotext.exe"
    } else {
        "pdftotext"
    };
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name(name);
        if sibling.is_file() {
            return sibling;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let on_path = std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
        });
        if !on_path {
            for directory in ["/opt/local/bin", "/opt/homebrew/bin", "/usr/local/bin"] {
                let installed = Path::new(directory).join(name);
                if installed.is_file() {
                    return installed;
                }
            }
        }
    }
    PathBuf::from(name)
}

fn run(command: &mut Command) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => anyhow!("tool_missing: pdftotext (Poppler utilities)"),
            _ => anyhow!("read: pdftotext could not start: {err}"),
        })?;
    let stdout = child
        .stdout
        .take()
        .context("read: missing Poppler output stream")?;
    let stderr = child
        .stderr
        .take()
        .context("read: missing Poppler diagnostic stream")?;
    thread::scope(|scope| {
        // Read both pipes while Poppler runs. Bound retained bytes even if a PDF expands substantially.
        let output = scope.spawn(move || {
            let mut data = Vec::new();
            stdout
                .take(MAX_TEXT_BYTES as u64 + 1)
                .read_to_end(&mut data)
                .map(|_| data)
        });
        let diagnostics = scope.spawn(move || {
            let mut stream = stderr;
            let mut first = Vec::new();
            (&mut stream).take(4096).read_to_end(&mut first)?;
            io::copy(&mut stream, &mut io::sink())?;
            Ok::<_, io::Error>(first)
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < DECODE_TIMEOUT => {
                    thread::sleep(Duration::from_millis(15))
                }
                outcome => {
                    let _ = child.kill();
                    let _ = child.wait();
                    match outcome {
                        Err(err) => return Err(err.into()),
                        _ => bail!("too_large: PDF exceeded the decoder time limit"),
                    }
                }
            }
        };
        let output = output
            .join()
            .map_err(|_| anyhow!("read: Poppler output thread failed"))??;
        let diagnostic = diagnostics
            .join()
            .map_err(|_| anyhow!("read: Poppler diagnostic thread failed"))??;
        ensure!(
            output.len() <= MAX_TEXT_BYTES,
            "too_large: PDF text and geometry exceed limit"
        );
        Ok((status, output, diagnostic))
    })
}

struct Page {
    number: usize,
    width: f64,
    height: f64,
    first_block: usize,
}

#[derive(Default)]
struct Line {
    text: String,
    utf16: usize,
    words: Vec<Value>,
}

impl Line {
    fn push(&mut self, word: Word) {
        if word.text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push(' ');
            self.utf16 += 1;
        }
        let start = self.utf16;
        let length = word.text.encode_utf16().count();
        self.text.push_str(&word.text);
        self.utf16 += length;
        self.words.push(
            json!({"start":start, "length":length, "x":word.x, "y":word.y, "w":word.w, "h":word.h}),
        );
    }
    fn block(self, page: &Page, index: usize) -> Block {
        Block::new(
            self.text,
            json!({
                "index":index, "page":page.number, "width":page.width, "height":page.height, "words":self.words
            }),
            format!("Page {}", page.number),
        )
    }
}

struct Word {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

// Poppler writes UTF-8 and plain decimal attributes. External entities are never resolved.
fn number(tag: &BytesStart<'_>, name: &[u8]) -> Result<f64> {
    for attribute in tag.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == name {
            let value: f64 = std::str::from_utf8(&attribute.value)?.parse()?;
            ensure!(
                value.is_finite(),
                "invalid_document: nonfinite PDF coordinate"
            );
            return Ok(value);
        }
    }
    bail!("invalid_document: missing PDF coordinate")
}

fn parse(xml: &[u8]) -> Result<Document> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut page = None::<Page>;
    let mut line = None::<Line>;
    let mut word = None::<Word>;
    let mut pages = 0;
    let mut words = 0;
    let mut blocks = Vec::new();
    loop {
        match reader
            .read_event()
            .context("invalid_document: malformed PDF text data")?
        {
            Event::Start(tag) => match tag.name().as_ref() {
                b"page" => {
                    ensure!(page.is_none(), "invalid_document: nested PDF pages");
                    pages += 1;
                    ensure!(pages <= MAX_PAGES, "too_large: too many PDF pages");
                    let (width, height) = (number(&tag, b"width")?, number(&tag, b"height")?);
                    ensure!(
                        width > 0.0 && height > 0.0,
                        "invalid_document: invalid PDF page size"
                    );
                    page = Some(Page {
                        number: pages,
                        width,
                        height,
                        first_block: blocks.len(),
                    });
                }
                b"line" if page.is_some() => {
                    line = Some(Line::default());
                }
                b"word" if line.is_some() => {
                    words += 1;
                    ensure!(words <= MAX_WORDS, "too_large: too many PDF words");
                    let (x, y) = (number(&tag, b"xMin")?, number(&tag, b"yMin")?);
                    let (right, bottom) = (number(&tag, b"xMax")?, number(&tag, b"yMax")?);
                    word = Some(Word {
                        text: String::new(),
                        x,
                        y,
                        w: (right - x).max(0.0),
                        h: (bottom - y).max(0.0),
                    });
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(word) = word.as_mut() {
                    word.text.push_str(std::str::from_utf8(&text)?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(word) = word.as_mut() {
                    let entity = format!("&{};", std::str::from_utf8(&reference)?);
                    word.text.push_str(&quick_xml::escape::unescape(&entity)?);
                }
            }
            Event::End(tag) => match tag.name().as_ref() {
                b"word" => {
                    if let (Some(line), Some(word)) = (line.as_mut(), word.take()) {
                        line.push(word);
                    }
                }
                b"line" => {
                    if let (Some(page), Some(line)) = (page.as_ref(), line.take()) {
                        if !line.text.is_empty() {
                            ensure!(blocks.len() < MAX_BLOCKS, "too_large: too many PDF lines");
                            blocks.push(line.block(page, blocks.len()));
                        }
                    }
                }
                b"page" => {
                    if let Some(page) = page.take() {
                        // Navigating across an image-only page in a mixed document must still work.
                        if blocks.len() == page.first_block {
                            ensure!(blocks.len() < MAX_BLOCKS, "too_large: too many PDF lines");
                            blocks.push(Line::default().block(&page, blocks.len()));
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(
        pages != 0 && page.is_none(),
        "invalid_document: PDF has no complete pages"
    );
    ensure!(
        blocks.iter().any(|b| !b.text.trim().is_empty()),
        "no_text_layer: PDF contains no searchable text"
    );
    for block in &mut blocks {
        block.location["pageCount"] = json!(pages);
    }
    Ok(Document {
        kind: "pdf".into(),
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_coordinates_disambiguate_repetitions_after_supplementary_characters() {
        let pdftext = r#"<doc><page width="100" height="200"/><page width="500" height="400"><flow><block><line>
          <word xMin="0" yMin="10" xMax="20" yMax="31">🪡&amp;</word>
          <word xMin="40" yMin="10" xMax="60" yMax="31">needle</word>
          <word xMin="80" yMin="10" xMax="100" yMax="31">needle</word>
          </line></block></flow></page></doc>"#;
        let doc = parse(pdftext.as_bytes()).unwrap();
        assert_eq!(doc.blocks.len(), 2);
        assert!(doc.blocks[0].text.is_empty());
        let block = &doc.blocks[1];
        assert_eq!(block.text, "🪡& needle needle");
        assert_eq!(block.location["index"], 1);
        assert_eq!(block.location["page"], 2);
        assert_eq!(block.location["pageCount"], 2);
        let boxes = &block.location["words"];
        assert_eq!(boxes[1]["start"], 4);
        assert_eq!(boxes[2]["start"], 11);
        assert_eq!(boxes[2]["x"], 80.0);
        assert_eq!(boxes[2]["h"], 21.0);
    }

    #[test]
    fn image_only_and_malformed_geometry_are_explicit_errors() {
        assert!(parse(br#"<doc><page width="23" height="12"/></doc>"#)
            .err()
            .unwrap()
            .to_string()
            .contains("no_text_layer"));
        assert!(parse(br#"<doc><page width="NaN" height="10"/></doc>"#).is_err());
    }
}
