// SPDX-License-Identifier: GPL-3.0-or-later
//! The walker supplies a bounded lookahead of paths. One coordinator owns the
//! protocol writer, admission decisions, and all counts. Each reader keeps its
//! own file/decoder on one thread and sends only compact match excerpts.
use super::{
    file,
    resources::{Cost, Resources},
    FileCancellation, CANCEL, MAX_RESULTS,
};
use crate::{
    paths,
    protocol::{Emitter, SearchRequest},
    Failure,
};
use glob::{MatchOptions, Pattern};
use grep_regex::RegexMatcher;
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::json;
use std::{
    collections::VecDeque,
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Default)]
pub(super) struct Progress {
    pub names: usize,
    pub content: usize,
}

pub(super) struct Job {
    pub path: PathBuf,
    cost: Cost,
    pub published: Progress,
    pub revision: Option<String>,
    pub names_only: bool,
    counted: bool,
}

pub(super) struct Report {
    pub outcome: Result<bool, Failure>,
    pub revision: Option<String>,
}

pub(super) enum Event {
    Path(Job),
    WalkError(String),
    WalkDone,
    Hit {
        slot: usize,
        filename: bool,
        payload: Vec<u8>,
    },
    Finished {
        slot: usize,
        job: Job,
        report: Report,
        interrupted: bool,
    },
}

fn discover(
    folder: &Path,
    req: &SearchRequest,
    pattern: Option<Pattern>,
    credits: Receiver<()>,
    events: &SyncSender<Event>,
    stopping: &AtomicBool,
) {
    let walker = WalkBuilder::new(folder)
        .git_ignore(!req.ignored)
        .git_exclude(!req.ignored)
        .git_global(!req.ignored)
        .ignore(!req.ignored)
        .hidden(!req.hidden)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    let options = MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    for entry in walker {
        if stopping.load(Ordering::Relaxed) {
            return;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                if events.send(Event::WalkError(e.to_string())).is_err() {
                    return;
                }
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(folder).unwrap_or(path);
        if pattern
            .as_ref()
            .is_some_and(|p| !p.matches_with(&relative.to_string_lossy(), options))
        {
            continue;
        }
        // Each path occupies a credit until the coordinator dispatches or skips it.
        if credits.recv().is_err() {
            return;
        }
        let cost = Cost::for_path(path, &req.mode, req.regex);
        let job = Job {
            path: entry.into_path(),
            cost,
            published: Progress::default(),
            revision: None,
            names_only: false,
            counted: false,
        };
        if events.send(Event::Path(job)).is_err() {
            return;
        }
    }
}

struct Slot {
    jobs: SyncSender<Job>,
    busy: bool,
    published: Progress,
    overflow: Progress,
}

fn start_worker<'scope, 'env: 'scope>(
    scope: &'scope thread::Scope<'scope, 'env>,
    slot: usize,
    folder: &'env Path,
    req: &'env SearchRequest,
    matcher: &RegexMatcher,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
) -> io::Result<Slot> {
    let (jobs, input) = mpsc::sync_channel::<Job>(1);
    // A separate matcher cache per worker avoids contention inside regex searches.
    let matcher = matcher.clone();
    thread::Builder::new()
        .name(format!("file-search-{slot}"))
        .spawn_scoped(scope, move || {
            for job in input {
                let token = Arc::new(FileCancellation {
                    requested: Arc::clone(&stop),
                    observed: AtomicBool::new(false),
                });
                CANCEL.with_borrow_mut(|t| *t = Some(Arc::clone(&token)));
                let report = catch_unwind(AssertUnwindSafe(|| {
                    file::scan(slot, &job, folder, req, &matcher, &events)
                }))
                .unwrap_or_else(|_| Report {
                    outcome: Err(Failure::new(
                        "format",
                        "Document reader stopped unexpectedly",
                    )),
                    revision: None,
                });
                CANCEL.with_borrow_mut(|t| *t = None);
                let interrupted = token.observed.load(Ordering::Relaxed);
                if events
                    .send(Event::Finished {
                        slot,
                        job,
                        report,
                        interrupted,
                    })
                    .is_err()
                {
                    break;
                }
            }
        })?;
    Ok(Slot {
        jobs,
        busy: false,
        published: Progress::default(),
        overflow: Progress::default(),
    })
}

fn examined<W: Write>(sink: &mut Emitter<W>, last: &mut Instant) -> io::Result<()> {
    sink.files += 1;
    if sink.files == 1 || last.elapsed() >= Duration::from_millis(100) {
        sink.send(&json!({"type":"progress", "files":sink.files,"documentsScanned":sink.documents_scanned}))?;
        *last = Instant::now();
    }
    Ok(())
}

#[derive(Serialize)]
struct Discard {
    r#type: &'static str,
    #[serde(flatten)]
    identity: paths::Identity,
}

struct Coordinator<'a, W> {
    sink: &'a mut Emitter<W>,
    pending: VecDeque<Job>,
    retry: Vec<Job>,
    slots: Vec<Slot>,
    resources: Resources,
    last_progress: Instant,
    paused: bool,
    valid_overflow: bool,
}

impl<W: Write> Coordinator<'_, W> {
    fn hit(
        &mut self,
        slot: usize,
        filename: bool,
        payload: Vec<u8>,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        let task = &mut self.slots[slot];
        // Hold admission as soon as an extra occurrence exists. Final revision
        // checks on active files can still free slots or invalidate that extra hit.
        if self.paused || self.sink.count == MAX_RESULTS {
            self.paused = true;
            stop.store(true, Ordering::Relaxed);
            if filename {
                task.overflow.names += 1;
            } else {
                task.overflow.content += 1;
            }
        } else {
            self.sink.send_serialized(&payload)?;
            self.sink.count += 1;
            if filename {
                task.published.names += 1;
            } else {
                task.published.content += 1;
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        slot: usize,
        mut job: Job,
        report: Report,
        interrupted: bool,
    ) -> io::Result<()> {
        let task = &mut self.slots[slot];
        task.busy = false;
        self.resources.finish(job.cost);
        let mut published = task.published;
        // A reader interrupted at a chunk boundary can report an internal read
        // error. A real decoder/revision error still calls for a per-file rollback.
        let interrupted = interrupted
            && match &report.outcome {
                Ok(_) => true,
                Err(e) => e.message.contains("search interrupted"),
            };
        let content_valid = interrupted || matches!(report.outcome, Ok(true));
        if !content_valid && published.content != 0 {
            self.sink.send(&Discard {
                r#type: "discard",
                identity: paths::identity(&job.path),
            })?;
            self.sink.count -= published.content;
            published.content = 0;
        }
        if !interrupted {
            if let Err(error) = report.outcome {
                self.sink.error(error, Some(&job.path))?;
            }
        }
        let extra = task.overflow.names != 0 || (content_valid && task.overflow.content != 0);
        self.valid_overflow |= extra;
        if interrupted || extra {
            // Only in-flight jobs can require replay, and only if another file
            // invalidates visible matches. Keep paths, revisions and counts.
            job.names_only |= !content_valid;
            job.published = published;
            job.revision = report.revision;
            self.retry.push(job);
        } else if content_valid {
            self.sink.documents_scanned += 1;
        }
        Ok(())
    }
}

pub(super) fn run<W: Write>(
    req: &SearchRequest,
    folder: &Path,
    matcher: RegexMatcher,
    pattern: Option<Pattern>,
    sink: &mut Emitter<W>,
) -> io::Result<()> {
    let resources = Resources::new(req.search_workers, req.pdf_conversions);
    let depth = resources.workers.saturating_mul(4).clamp(32, 1024);
    let mut coordinator = Coordinator {
        sink,
        pending: VecDeque::new(),
        retry: Vec::new(),
        resources,
        slots: Vec::new(),
        last_progress: Instant::now(),
        paused: false,
        valid_overflow: false,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let stop_walker = AtomicBool::new(false);
    thread::scope(|scope| {
        let (events, input) = mpsc::sync_channel((coordinator.resources.workers * 2).clamp(8, 128));
        let (refill, credits) = mpsc::sync_channel(depth);
        for _ in 0..depth {
            refill.try_send(()).unwrap();
        }
        let walk_events = events.clone();
        let stop_walk = &stop_walker;
        thread::Builder::new()
            .name("search-folders".into())
            .spawn_scoped(scope, move || {
                let walked = catch_unwind(AssertUnwindSafe(|| {
                    discover(folder, req, pattern, credits, &walk_events, stop_walk)
                }));
                if walked.is_err() {
                    let _ = walk_events.send(Event::WalkError(
                        "Folder reader stopped unexpectedly".into(),
                    ));
                }
                let _ = walk_events.send(Event::WalkDone);
            })?;
        let mut walked = false;
        let result = (|| {
            loop {
                if coordinator.paused && coordinator.resources.active() == 0 {
                    if coordinator.sink.count == MAX_RESULTS && coordinator.valid_overflow {
                        coordinator.sink.truncated = true;
                        return Ok(());
                    }
                    coordinator.paused = false;
                    coordinator.valid_overflow = false;
                    cancel.store(false, Ordering::Relaxed);
                    for job in coordinator.retry.drain(..).rev() {
                        coordinator.pending.push_front(job);
                    }
                }
                if !coordinator.paused {
                    coordinator.resources.refresh();
                    while coordinator.resources.active() < coordinator.resources.workers {
                        let Some(index) = coordinator
                            .pending
                            .iter()
                            .position(|j| coordinator.resources.allows(j.cost))
                        else {
                            break;
                        };
                        let mut job = coordinator.pending.remove(index).unwrap();
                        if !job.counted {
                            // Never count mere lookahead, and never count a replay twice.
                            examined(coordinator.sink, &mut coordinator.last_progress)?;
                            job.counted = true;
                            let _ = refill.try_send(());
                        }
                        if job.cost.skip && !req.filenames {
                            continue;
                        }
                        let slot = match coordinator.slots.iter().position(|s| !s.busy) {
                            Some(slot) => slot,
                            None => {
                                let id = coordinator.slots.len();
                                let worker = start_worker(
                                    scope,
                                    id,
                                    folder,
                                    req,
                                    &matcher,
                                    events.clone(),
                                    Arc::clone(&cancel),
                                )?;
                                coordinator.slots.push(worker);
                                id
                            }
                        };
                        let worker = &mut coordinator.slots[slot];
                        worker.busy = true;
                        worker.published = job.published;
                        worker.overflow = Progress::default();
                        coordinator.resources.start(job.cost);
                        worker
                            .jobs
                            .send(job)
                            .map_err(|_| io::Error::other("file search thread stopped"))?;
                    }
                }
                if walked && coordinator.resources.active() == 0 && coordinator.pending.is_empty() {
                    return Ok(());
                }
                match input
                    .recv()
                    .map_err(|_| io::Error::other("file search stopped"))?
                {
                    Event::Path(job) => coordinator.pending.push_back(job),
                    Event::WalkError(error) => {
                        coordinator.sink.error(Failure::new("walk", error), None)?
                    }
                    Event::WalkDone => walked = true,
                    Event::Hit {
                        slot,
                        filename,
                        payload,
                    } => coordinator.hit(slot, filename, payload, &cancel)?,
                    Event::Finished {
                        slot,
                        job,
                        report,
                        interrupted,
                    } => coordinator.finish(slot, job, report, interrupted)?,
                }
            }
        })();
        cancel.store(true, Ordering::Relaxed);
        stop_walker.store(true, Ordering::Relaxed);
        coordinator.slots.clear();
        drop(refill);
        drop(input);
        result
    })
}
