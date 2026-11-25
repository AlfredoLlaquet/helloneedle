// SPDX-License-Identifier: GPL-3.0-or-later
//! Send the exact selection in bounded frames. The client commits the clipboard
//! only after the whole selection and its file revision have been verified.
use super::{Emitter, PreviewRequest};
use crate::{documents, office, Block, Reader};
use anyhow::{anyhow, ensure, Result};
use serde_json::json;
use std::{io::Write, path::Path};

fn emit_span(
    text: &str,
    start: usize,
    length: usize,
    emit: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| anyhow!("location: invalid match length"))?;
    let mut position = 0;
    let mut first = None;
    for (byte, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        if position == start {
            first = Some(byte);
        }
        if position == end {
            let left = first.ok_or_else(|| anyhow!("location: invalid match position"))?;
            if left < byte {
                emit(&text[left..byte])?;
            }
            return Ok(());
        }
        if byte == text.len() {
            break;
        }
        let next = position + ch.len_utf16();
        ensure!(
            !(position < start && start < next || position < end && end < next),
            "location: match splits a Unicode character"
        );
        if let Some(left) = first {
            if byte - left >= 64 * 1024 {
                emit(&text[left..byte])?;
                first = Some(byte);
            }
        }
        position = next;
    }
    anyhow::bail!("location: match is outside the document")
}

pub(super) fn stream<W: Write>(
    path: &Path,
    request: &PreviewRequest,
    events: &mut Emitter<W>,
) -> Result<()> {
    let (start, length) = (request.block_start, request.block_length);
    let mut emit = |part: &str| {
        events.send(&json!({"type": "matchText", "text": part}))?;
        Ok(())
    };
    if request.location["filename"] == true {
        return emit_span(
            &path.file_name().unwrap_or_default().to_string_lossy(),
            start,
            length,
            &mut emit,
        );
    }
    let reader = Reader::for_path(path, &request.mode);
    if let Reader::Lines(kind) = reader {
        return documents::text_stream::copy_match(
            path,
            kind,
            &request.location,
            start,
            length,
            emit,
        );
    }
    let target = request.location["index"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| anyhow!("location: invalid selected block"))?;
    let mut found = false;
    let mut visit = |block: Block| -> Result<bool> {
        if block.location["index"] != target {
            return Ok(true);
        }
        emit_span(&block.text, start, length, &mut emit)?;
        found = true;
        Ok(false)
    };
    match reader {
        Reader::Docx => office::visit_docx(path, &mut visit)?,
        Reader::Office(kind) => office::visit_window(path, kind, target, &mut visit)?,
        Reader::Markdown => documents::visit_markdown(path, &mut visit)?,
        Reader::Pdf => documents::pdf::scan(path, &mut visit)?,
        Reader::Html => {
            let doc = crate::html_document(path)
                .map_err(|err| anyhow!("{}: {}", err.code, err.message))?;
            if let Some(block) = doc.blocks.into_iter().nth(target) {
                emit_span(&block.text, start, length, &mut emit)?;
                found = true;
            }
        }
        Reader::Skip | Reader::Lines(_) => {}
    }
    ensure!(found, "location: match is outside the document");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_utf16_span_is_transferred_across_many_frames() {
        let wanted = "bé🧵".repeat(90_000);
        let source = format!("🧵left{wanted}right");
        let mut parts = Vec::new();
        emit_span(&source, 6, wanted.encode_utf16().count(), &mut |s| {
            parts.push(s.to_owned());
            Ok(())
        })
        .unwrap();
        assert!(parts.len() > 2);
        assert_eq!(parts.concat(), wanted);
        assert!(emit_span("a🧵z", 2, 1, &mut |_| Ok(())).is_err());
        assert!(emit_span("short", 0, 7, &mut |_| Ok(())).is_err());
        assert!(emit_span("a", 1, 0, &mut |_| Ok(())).is_ok());
    }
}
