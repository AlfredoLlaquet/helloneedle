// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Output, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::{
    env,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod word97;

fn executable() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "antiword.exe"
    } else {
        "antiword"
    };
    let bundled = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)));
    let path = env::var_os("PATH").unwrap_or_default();
    let dirs = env::split_paths(&path).filter(|dir| dir.is_absolute());
    #[cfg(target_os = "macos")]
    let dirs =
        dirs.chain(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from));
    bundled
        .into_iter()
        .chain(dirs.map(|dir| dir.join(name)))
        .find(|p| p.is_file())
}

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
    let program = executable().ok_or_else(|| anyhow!("missing_decoder: antiword"))?;
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
    let mut command = Command::new(&program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW when launched by the desktop
    }
    if env::var_os("ANTIWORDHOME").is_none() {
        if let Some(prefix) = program.parent().and_then(Path::parent) {
            let resources = prefix.join("share").join("antiword");
            if resources.join("UTF-8.txt").is_file() && resources.join("fontnames").is_file() {
                command.env("ANTIWORDHOME", resources);
            }
        }
    }
    let mut child = command
        .args(["-t", "-w", "0", "-m", "UTF-8.txt"])
        .current_dir(parent)
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("missing_decoder: antiword: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = thread::spawn(move || {
        let mut b = Vec::new();
        stdout
            .take(MAX_TEXT_BYTES as u64 + 1)
            .read_to_end(&mut b)
            .map(|_| b)
    });
    let diagnostics = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.take(8192).read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("resource_limit: Word decoder timeout");
        }
        thread::sleep(Duration::from_millis(15));
    };
    let b = output
        .join()
        .map_err(|_| anyhow!("corrupt_document: Word decoder output"))??;
    let diagnostic_bytes = diagnostics.join().unwrap_or_default();
    let diag = String::from_utf8_lossy(&diagnostic_bytes).to_lowercase();
    if b.len() > MAX_TEXT_BYTES {
        bail!("resource_limit: Word decoder output");
    }
    if !status.success() {
        let reason = diagnostic(&diagnostic_bytes, &absolute, &input);
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
    let text = std::str::from_utf8(&b)
        .map_err(|_| anyhow!("corrupt_document: invalid Word decoder text"))?;
    let mut number = 0;
    for line in text.lines() {
        if !line.trim().is_empty() {
            number += 1;
            out.push(
                line.to_owned(),
                json!({"paragraph":number}),
                format!("Paragraph {number}"),
                None,
            )?;
        }
    }
    Ok(())
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
