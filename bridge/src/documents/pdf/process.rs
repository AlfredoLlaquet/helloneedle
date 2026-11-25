// SPDX-License-Identifier: GPL-3.0-or-later
//! Read a converter's stdout directly, drain stderr concurrently, and time only
//! periods spent waiting for the converter (not time spent searching its output).

use anyhow::{anyhow, Context, Result};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(45);
const DIAGNOSTIC_BYTES: usize = 4096;

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

struct Activity {
    origin: Instant,
    since: AtomicU64,
    idle: Duration,
}

impl Activity {
    fn waiting(&self) {
        self.since.store(
            self.origin.elapsed().as_millis() as u64 + 1,
            Ordering::Relaxed,
        );
    }

    fn timed_out(&self) -> bool {
        let since = self.since.load(Ordering::Relaxed);
        since != 0
            && self.origin.elapsed().as_millis() as u64 + 1 - since >= self.idle.as_millis() as u64
    }
}

struct ProgressReader<'a, R> {
    stream: R,
    activity: &'a Activity,
    eof: bool,
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.activity.waiting();
        let result = self.stream.read(buffer);
        self.activity.since.store(0, Ordering::Relaxed);
        self.eof = matches!(result, Ok(0)) && !buffer.is_empty();
        result
    }
}

enum Finish {
    Wait,
    Stop,
}

enum Completion {
    Exited(ExitStatus),
    TimedOut,
    Interrupted,
    Stopped,
}

fn monitor(
    mut child: Child,
    activity: &Activity,
    finish: Receiver<Finish>,
    cancel: Option<&crate::search::FileCancellation>,
) -> io::Result<Completion> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Completion::Exited(status));
        }
        let completion = match finish.recv_timeout(Duration::from_millis(100)) {
            Ok(Finish::Stop) | Err(RecvTimeoutError::Disconnected) => Completion::Stopped,
            Ok(Finish::Wait) => continue,
            Err(RecvTimeoutError::Timeout) if cancel.is_some_and(|t| t.stopped()) => {
                Completion::Interrupted
            }
            Err(RecvTimeoutError::Timeout) if activity.timed_out() => Completion::TimedOut,
            Err(RecvTimeoutError::Timeout) => continue,
        };
        let _ = child.kill();
        child.wait()?;
        return Ok(completion);
    }
}

fn diagnostics(mut stream: impl Read) -> io::Result<String> {
    // Converters may print many harmless warnings before their actual error.
    // Keep the tail while always draining the pipe.
    let mut tail = Vec::new();
    let mut buffer = [0; DIAGNOSTIC_BYTES];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let excess = (tail.len() + count).saturating_sub(DIAGNOSTIC_BYTES);
        tail.drain(..excess);
        tail.extend_from_slice(&buffer[..count]);
    }
    Ok(String::from_utf8_lossy(&tail)
        .split(|ch: char| ch.is_control() || ch.is_whitespace())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" "))
}

/// `false` from the consumer means the caller has what it needs. Terminate
/// the child immediately in that case, including when the result limit is hit.
pub(super) fn run(
    command: &mut Command,
    consume: impl FnOnce(&mut dyn BufRead) -> Result<bool>,
) -> Result<()> {
    run_with_idle(command, consume, IDLE)
}

fn run_with_idle(
    command: &mut Command,
    consume: impl FnOnce(&mut dyn BufRead) -> Result<bool>,
    idle: Duration,
) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("read: could not start PDF helper")?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let activity = Activity {
        origin: Instant::now(),
        since: AtomicU64::new(0),
        idle,
    };
    let cancel = crate::search::cancellation();
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let waiter = scope.spawn(|| monitor(child, &activity, receiver, cancel.as_deref()));
        let diagnostic = scope.spawn(|| diagnostics(stderr));
        let mut reader = BufReader::with_capacity(
            64 * 1024,
            ProgressReader {
                stream: stdout,
                activity: &activity,
                eof: false,
            },
        );
        let parsed = consume(&mut reader);
        let stopped = matches!(parsed, Ok(false));
        let stop_child = stopped || (parsed.is_err() && !reader.get_ref().eof);
        drop(reader);
        activity.waiting();
        let _ = sender.send(if stop_child {
            Finish::Stop
        } else {
            Finish::Wait
        });
        let completed = waiter
            .join()
            .map_err(|_| anyhow!("read: PDF helper monitor stopped"))??;
        let diagnostic = diagnostic
            .join()
            .map_err(|_| anyhow!("read: PDF diagnostic reader stopped"))??;
        if stopped {
            return Ok(());
        }
        match completed {
            Completion::Interrupted => Err(anyhow!("search interrupted")),
            Completion::TimedOut => Err(anyhow!(
                "resource_limit: PDF helper made no progress for {} seconds. {diagnostic}",
                idle.as_secs()
            )),
            Completion::Exited(status) if !status.success() => {
                Err(DecoderFailure { status, diagnostic }.into())
            }
            _ => parsed.map(|_| ()),
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn drain(stream: &mut dyn BufRead) -> Result<bool> {
        io::copy(stream, &mut io::sink())?;
        Ok(true)
    }

    #[test]
    fn idle_watchdog_allows_continuing_progress_and_stops_a_stalled_child() {
        let mut working = Command::new("sh");
        working.args(["-c", "for i in 1 2 3 4 5; do printf .; sleep 0.35; done"]);
        run_with_idle(&mut working, drain, Duration::from_millis(1400)).unwrap();
        let mut stalled = Command::new("sh");
        stalled.args(["-c", "printf .; exec sleep 20"]);
        let err = run_with_idle(&mut stalled, drain, Duration::from_millis(300)).unwrap_err();
        assert!(err.to_string().contains("made no progress"), "{err}");
    }
}
