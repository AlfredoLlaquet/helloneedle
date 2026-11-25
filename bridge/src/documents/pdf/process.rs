// SPDX-License-Identifier: GPL-3.0-or-later
//! Map the generic converter monitor's exit status to PDF-specific recovery.
use crate::converters::process;
use anyhow::Result;
use std::{
    fmt,
    io::BufRead,
    process::{Command, ExitStatus},
    time::Duration,
};

#[derive(Debug)]
pub(super) struct DecoderFailure {
    status: ExitStatus,
    diagnostic: String,
}

impl DecoderFailure {
    pub(super) fn crashed(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            self.status.signal().is_some()
        }
        #[cfg(not(unix))]
        {
            // Windows reports exception codes as signed statuses. The MS CRT
            // may also return EXIT_FAILURE/3 with its abort diagnostic.
            self.status.code().is_some_and(|code| code < 0)
                || self.diagnostic.to_ascii_lowercase().contains("abort")
        }
    }
}

impl fmt::Display for DecoderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lower = self.diagnostic.to_ascii_lowercase();
        let protected = !self.crashed()
            && (self.status.code() == Some(3)
                || lower.contains("password")
                || lower.contains("permission"));
        let code = if protected {
            "protected"
        } else {
            "invalid_document"
        };
        write!(f, "{code}: pdftotext ")?;
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = self.status.signal() {
                match signal {
                    6 => write!(f, "crashed (SIGABRT, signal {signal})")?,
                    11 => write!(f, "crashed (SIGSEGV, signal {signal})")?,
                    _ => write!(f, "crashed (signal {signal})")?,
                }
            } else {
                write!(
                    f,
                    "exited with code {}",
                    self.status.code().unwrap_or_default()
                )?;
            }
        }
        #[cfg(not(unix))]
        write!(
            f,
            "exited with code {}",
            self.status.code().unwrap_or_default()
        )?;
        if !self.diagnostic.is_empty() {
            write!(f, ": {}", self.diagnostic)?;
        }
        Ok(())
    }
}

impl std::error::Error for DecoderFailure {}

pub(super) fn run(
    command: &mut Command,
    consume: impl FnOnce(&mut dyn BufRead) -> Result<bool>,
) -> Result<()> {
    if let Some((status, diagnostic)) =
        process::run(command, "PDF helper", Duration::from_secs(45), consume)?
    {
        return Err(DecoderFailure { status, diagnostic }.into());
    }
    Ok(())
}
