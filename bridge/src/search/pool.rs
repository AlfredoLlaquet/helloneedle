// SPDX-License-Identifier: GPL-3.0-or-later
//! The walker supplies a bounded lookahead of paths. One coordinator owns the
//! protocol writer, admission decisions, and all counts. Each reader keeps its
//! own file/decoder on one thread and sends only compact match excerpts.
use super::{
    file,
    resources::{Cost, Resources},
    FileCancellation, SearchQuery, CANCEL,
};
use crate::{
    paths,
    protocol::{Emitter, SearchRequest},
    Failure,
};
use glob::{MatchOptions, Pattern};
use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::json;
use std::{
    collections::VecDeque,
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct Job {
    path: PathBuf,
    cost: Cost,
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
        outcome: Result<bool, Failure>,
    },
}

fn discover(
    folder: &Path,
    req: &SearchRequest,
    pattern: Option<Pattern>,
    credits: Receiver<()>,
    events: &SyncSender<Event>,
    cancellation: &FileCancellation,
) {
    let walker = WalkBuilder::new(folder)
        .git_ignore(!req.ignored)
        .git_exclude(!req.ignored)
        .git_global(!req.ignored)
        .ignore(!req.ignored)
        .hidden(!req.hidden)
        .require_git(false)
        .follow_links(false)
        .max_depth(req.exclude_subdirectories.then_some(1))
        .build();
    let options = MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    for entry in walker {
        if cancellation.stopped() {
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
        };
        if events.send(Event::Path(job)).is_err() {
            return;
        }
    }
}

struct Slot {
    jobs: SyncSender<Job>,
    busy: bool,
    content_hits: u64,
}

fn start_worker<'scope, 'env: 'scope>(
    scope: &'scope thread::Scope<'scope, 'env>,
    slot: usize,
    folder: &'env Path,
    req: &'env SearchRequest,
    query: &SearchQuery,
    events: SyncSender<Event>,
    cancellation: Arc<FileCancellation>,
) -> io::Result<Slot> {
    let (jobs, input) = mpsc::sync_channel::<Job>(1);
    // A separate matcher cache per worker avoids contention inside regex searches.
    let query = query.clone();
    thread::Builder::new()
        .name(format!("file-search-{slot}"))
        .spawn_scoped(scope, move || {
            CANCEL.with_borrow_mut(|flag| *flag = Some(cancellation));
            for job in input {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    file::scan(slot, &job.path, folder, req, &query, &events)
                }))
                .unwrap_or_else(|_| {
                    Err(Failure::new(
                        "format",
                        "Document reader stopped unexpectedly",
                    ))
                });
                if events.send(Event::Finished { slot, job, outcome }).is_err() {
                    break;
                }
            }
        })?;
    Ok(Slot {
        jobs,
        busy: false,
        content_hits: 0,
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
    slots: Vec<Slot>,
    resources: Resources,
    last_progress: Instant,
}

impl<W: Write> Coordinator<'_, W> {
    fn hit(&mut self, slot: usize, filename: bool, payload: Vec<u8>) -> io::Result<()> {
        self.sink.send_serialized(&payload)?;
        if self.sink.count == 0 {
            self.sink.flush()?;
        }
        self.sink.count += 1;
        if !filename {
            self.slots[slot].content_hits += 1;
        }
        Ok(())
    }

    fn finish(&mut self, slot: usize, job: Job, outcome: Result<bool, Failure>) -> io::Result<()> {
        let task = &mut self.slots[slot];
        task.busy = false;
        self.resources.finish(job.cost);
        if matches!(outcome, Ok(true)) {
            self.sink.documents_scanned += 1;
        } else {
            // A late decode failure, binary marker or changed file invalidates
            // this file's content. Its filename and other files' hits survive.
            if task.content_hits != 0 {
                self.sink.send(&Discard {
                    r#type: "discard",
                    identity: paths::identity(&job.path),
                })?;
                self.sink.count -= task.content_hits;
            }
            if let Err(error) = outcome {
                self.sink.error(error, Some(&job.path))?;
            }
        }
        task.content_hits = 0;
        Ok(())
    }
}

pub(super) fn run<W: Write>(
    req: &SearchRequest,
    folder: &Path,
    query: SearchQuery,
    pattern: Option<Pattern>,
    sink: &mut Emitter<W>,
) -> io::Result<()> {
    let resources = Resources::new(req.search_workers, req.pdf_conversions);
    let depth = resources.workers.saturating_mul(4).clamp(32, 1024);
    let mut coordinator = Coordinator {
        sink,
        pending: VecDeque::new(),
        resources,
        slots: Vec::new(),
        last_progress: Instant::now(),
    };
    let cancellation = Arc::new(FileCancellation::default());
    thread::scope(|scope| {
        let (events, input) = mpsc::sync_channel((coordinator.resources.workers * 2).clamp(8, 128));
        let (refill, credits) = mpsc::sync_channel(depth);
        for _ in 0..depth {
            refill.try_send(()).unwrap();
        }
        let walk_events = events.clone();
        let walk_cancellation = Arc::clone(&cancellation);
        thread::Builder::new()
            .name("search-folders".into())
            .spawn_scoped(scope, move || {
                let walked = catch_unwind(AssertUnwindSafe(|| {
                    discover(
                        folder,
                        req,
                        pattern,
                        credits,
                        &walk_events,
                        &walk_cancellation,
                    )
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
                coordinator.resources.refresh();
                while coordinator.resources.active() < coordinator.resources.workers {
                    let Some(index) = coordinator
                        .pending
                        .iter()
                        .position(|j| coordinator.resources.allows(j.cost))
                    else {
                        break;
                    };
                    let job = coordinator.pending.remove(index).unwrap();
                    examined(coordinator.sink, &mut coordinator.last_progress)?;
                    let _ = refill.try_send(());
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
                                &query,
                                events.clone(),
                                Arc::clone(&cancellation),
                            )?;
                            coordinator.slots.push(worker);
                            id
                        }
                    };
                    let worker = &mut coordinator.slots[slot];
                    worker.busy = true;
                    coordinator.resources.start(job.cost);
                    worker
                        .jobs
                        .send(job)
                        .map_err(|_| io::Error::other("file search thread stopped"))?;
                }
                if walked && coordinator.resources.active() == 0 && coordinator.pending.is_empty() {
                    return Ok(());
                }
                // Flush when caught up, including a lone provisional match from
                // a reader that subsequently waits for more document input.
                let event = match input.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => {
                        coordinator.sink.flush()?;
                        input
                            .recv()
                            .map_err(|_| io::Error::other("file search stopped"))?
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(io::Error::other("file search stopped"))
                    }
                };
                match event {
                    Event::Path(job) => coordinator.pending.push_back(job),
                    Event::WalkError(error) => {
                        coordinator.sink.error(Failure::new("walk", error), None)?
                    }
                    Event::WalkDone => walked = true,
                    Event::Hit {
                        slot,
                        filename,
                        payload,
                    } => coordinator.hit(slot, filename, payload)?,
                    Event::Finished { slot, job, outcome } => {
                        coordinator.finish(slot, job, outcome)?
                    }
                }
            }
        })();
        cancellation.stop();
        coordinator.slots.clear();
        drop(refill);
        drop(input);
        result
    })
}
