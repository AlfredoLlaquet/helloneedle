// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Output, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::{
    fs::File,
    io::{BufRead, Read},
    path::Path,
    time::Duration,
};

mod word97;

pub(super) fn extract(path: &Path, mut file: File, out: &mut Output) -> Result<()> {
    let mut sig = [0u8; 8];
    let n = file.read(&mut sig)?;
    if n < 2 {
        bail!("corrupt_document: truncated Word file");
    }
    if &sig != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
        && !matches!(
            &sig[..2],
            b"\xdb\xa5" | b"\xdc\xa5" | b"\x31\xbe" | b"\x32\xbe"
        )
    {
        bail!("unsupported_document: expected a legacy binary Word file");
    }
    if &sig == b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" && word97::read(file, out)? {
        return Ok(());
    }
    let mut command = crate::converters::command("antiword")
        .ok_or_else(|| anyhow!("missing_decoder: antiword"))?;
    let absolute = path.canonicalize()?;
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow!("unreadable_document: missing directory"))?;
    let leaf = absolute
        .file_name()
        .ok_or_else(|| anyhow!("unreadable_document: missing filename"))?;
    // On Windows canonicalize() returns a verbatim path. Antiword uses narrow C argv,
    // fopen and stat. Let the wide process API select the document's directory and
    // pass the usual relative filename. Prefixing the leaf also protects leading '-'.
    let input = Path::new(".").join(leaf);
    command
        .args(["-t", "-w", "0", "-m", "UTF-8.txt"])
        .current_dir(parent)
        .arg(&input);
    let mut consumer_error = None;
    let result = crate::converters::process::run(
        &mut command,
        "antiword",
        Duration::from_secs(20),
        |stream| match paragraphs(stream, out) {
            Ok(()) => Ok(true),
            Err(error) => {
                consumer_error = Some(error);
                Ok(false)
            }
        },
    )?;
    if let Some(error) = consumer_error {
        return Err(error);
    }
    if let Some((status, detail)) = result {
        let diag = detail.to_lowercase();
        let reason = diagnostic(detail.as_bytes(), &absolute, &input);
        let reason = if reason.is_empty() {
            format!("antiword {status}")
        } else {
            format!("antiword: {reason}")
        };
        if diag.contains("encrypted") || diag.contains("password") {
            bail!("encrypted_document: {reason}");
        }
        if diag.contains("mapping") || diag.contains("utf-8.txt") || diag.contains("fontnames") {
            bail!("missing_decoder: {reason}");
        }
        if diag.contains("not supported")
            || diag.contains("not a word document")
            || diag.contains("does not contain a word document")
            || diag.contains("this is an excel spreadsheet")
            || diag.contains("too small to handle")
        {
            bail!("unsupported_document: {reason}");
        }
        if diag.contains("for reading")
            || diag.contains("get the size")
            || diag.contains("filesize error")
        {
            bail!("unreadable_document: {reason}");
        }
        bail!("corrupt_document: {reason}");
    }
    Ok(())
}

fn paragraphs(stream: &mut dyn BufRead, out: &mut Output) -> Result<()> {
    let mut number = 0usize;
    let mut line = Vec::new();
    loop {
        crate::search::checkpoint()?;
        let chunk = stream.fill_buf()?;
        let newline = chunk.iter().position(|b| *b == b'\n');
        let end = newline.unwrap_or(chunk.len());
        if line.len().saturating_add(end) > MAX_TEXT_BYTES {
            bail!("resource_limit: Word decoder paragraph");
        }
        line.extend_from_slice(&chunk[..end]);
        let eof = chunk.is_empty();
        stream.consume(end + usize::from(newline.is_some()));
        if newline.is_some() || eof {
            if line.last() == Some(&b'\r') && newline.is_some() {
                line.pop();
            }
            let text = std::str::from_utf8(&line)
                .map_err(|_| anyhow!("corrupt_document: invalid Word decoder text"))?;
            if !text.trim().is_empty() {
                number += 1;
                out.push(
                    text.to_owned(),
                    json!({"paragraph":number}),
                    format!("Paragraph {number}"),
                    None,
                )?;
            }
            line.clear();
        }
        if eof {
            return Ok(());
        }
    }
}

fn diagnostic(bytes: &[u8], absolute: &Path, argument: &Path) -> String {
    let mut message = String::from_utf8_lossy(bytes).into_owned();
    for path in [absolute, argument] {
        if let Some(spelling) = path.to_str().filter(|s| !s.is_empty()) {
            message = message.replace(spelling, "<document>");
        }
    }
    let clean: String = message
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    clean
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_decoder_reason_is_single_line_and_bounded() {
        let absolute = Path::new("personal/report.doc");
        let input = Path::new("report.doc");
        assert_eq!(
            diagnostic(
                "error:\r\npersonal/report.doc\tcannot be opened\u{202e}".as_bytes(),
                absolute,
                input
            ),
            "error: <document> cannot be opened"
        );
        assert_eq!(diagnostic(&[b'x'; 8000], absolute, input).len(), 240);
    }
}
