// SPDX-License-Identifier: GPL-3.0-or-later
//! Read a converter's stdout directly, drain stderr concurrently, and time only
//! periods spent waiting for the converter (not time spent searching its output).

use anyhow::{anyhow, Context, Result};
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const DIAGNOSTIC_BYTES: usize = 4096;

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

    fn made_progress(&self) {
        // A warning on stderr is real converter activity even if stdout is
        // still quiet. Do not start the timer while our caller processes text.
        let _ = self
            .since
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |since| {
                (since != 0).then(|| self.origin.elapsed().as_millis() as u64 + 1)
            });
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

fn diagnostics(mut stream: impl Read, activity: &Activity) -> io::Result<String> {
    // Converters may print many harmless warnings before their actual error.
    // Keep the tail while always draining the pipe.
    let mut tail = Vec::new();
    let mut buffer = [0; DIAGNOSTIC_BYTES];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        activity.made_progress();
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

/// Stream stdout with backpressure and always drain stderr. An early consumer
/// stop terminates and reaps the converter. Nonzero exits return their status
/// and a bounded diagnostic tail for the format-specific reader to interpret.
pub(crate) fn run(
    command: &mut Command,
    label: &str,
    idle: Duration,
    consume: impl FnOnce(&mut dyn BufRead) -> Result<bool>,
) -> Result<Option<(ExitStatus, String)>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("read: could not start {label}"))?;
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
        let diagnostic = scope.spawn(|| diagnostics(stderr, &activity));
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
            .map_err(|_| anyhow!("read: {label} monitor stopped"))??;
        let diagnostic = diagnostic
            .join()
            .map_err(|_| anyhow!("read: {label} diagnostic reader stopped"))??;
        if stopped {
            return Ok(None);
        }
        match completed {
            Completion::Interrupted => Err(anyhow!("search interrupted")),
            Completion::TimedOut => Err(anyhow!(
                "resource_limit: {label} made no progress for {} seconds. {diagnostic}",
                idle.as_secs()
            )),
            Completion::Exited(status) if !status.success() => Ok(Some((status, diagnostic))),
            _ => parsed.map(|_| None),
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
        run(
            &mut working,
            "test helper",
            Duration::from_millis(1400),
            drain,
        )
        .unwrap();
        let mut stalled = Command::new("sh");
        stalled.args(["-c", "printf .; exec sleep 20"]);
        let err = run(
            &mut stalled,
            "test helper",
            Duration::from_millis(300),
            drain,
        )
        .unwrap_err();
        assert!(err.to_string().contains("made no progress"), "{err}");
    }
    #[test]
    fn drains_stderr_to_its_last_diagnostic_and_counts_its_progress() {
        let mut child = Command::new("sh");
        child.args(["-c", "for n in 1 2 3 4 5; do printf . >&2; sleep 0.35; done; head -c 16000 /dev/zero >&2; printf late-reason >&2; exit 1"]);
        let result = run(
            &mut child,
            "test helper",
            Duration::from_millis(1400),
            drain,
        )
        .unwrap()
        .unwrap();
        assert!(result.1.ends_with("late-reason"));
    }
}
