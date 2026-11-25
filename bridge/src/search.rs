// SPDX-License-Identifier: GPL-3.0-or-later
// File traversal and matcher configuration adapted from Clapgrep (see NOTICE.md).

mod file;
mod memory;
mod pool;
mod resources;
#[cfg(test)]
mod tests;

use crate::{
    paths,
    protocol::{Emitter, SearchRequest},
    Failure,
};
use glob::Pattern;
use grep_regex::RegexMatcherBuilder;
use std::{
    cell::RefCell,
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

const MAX_RESULTS: usize = 10_000;

// Accessed once per read buffer or matching block. The desktop owns an entire worker
// process group. This token also stops competing readers when the result limit is reached.
thread_local! {
    static CANCEL: RefCell<Option<Arc<FileCancellation>>> = const { RefCell::new(None) };
}
pub(crate) struct FileCancellation {
    requested: Arc<AtomicBool>,
    observed: AtomicBool,
}

impl FileCancellation {
    pub(crate) fn stopped(&self) -> bool {
        if self.requested.load(Ordering::Relaxed) {
            self.observed.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

pub(crate) fn cancellation() -> Option<Arc<FileCancellation>> {
    CANCEL.with_borrow(Clone::clone)
}

pub(crate) fn stopped() -> bool {
    CANCEL.with_borrow(|token| token.as_ref().is_some_and(|t| t.stopped()))
}

pub(crate) fn checkpoint() -> io::Result<()> {
    if stopped() {
        Err(io::Error::other("search interrupted"))
    } else {
        Ok(())
    }
}

pub(crate) fn run<W: Write>(req: SearchRequest, sink: &mut Emitter<W>) -> io::Result<()> {
    if req.query.is_empty() {
        return sink.error(Failure::new("query", "Enter a search expression"), None);
    }
    let matcher = match RegexMatcherBuilder::new()
        .case_insensitive(!req.case_sensitive)
        .fixed_strings(!req.regex)
        .build(&req.query)
    {
        Ok(matcher) => matcher,
        Err(err) => return sink.error(Failure::new("query", err), None),
    };
    let pattern = match req.path_pattern.as_str() {
        "" => None,
        value => match Pattern::new(value) {
            Ok(pattern) => Some(pattern),
            Err(err) => return sink.error(Failure::new("query", err), None),
        },
    };
    let folder = match paths::decode(&req.folder, req.folder_bytes.as_deref()) {
        Ok(p) => match std::path::absolute(&p) {
            Ok(absolute) => absolute,
            Err(err) => return sink.error(Failure::new("folder", err), Some(&p)),
        },
        Err(err) => return sink.error(Failure::new("folder", err), None),
    };
    if !folder.is_dir() {
        return sink.error(
            Failure::new("folder", "Search folder does not exist or is inaccessible"),
            Some(&folder),
        );
    }
    pool::run(&req, &folder, matcher, pattern, sink)
}
