// SPDX-License-Identifier: GPL-3.0-or-later
//! Estimates control how many readers may run together. Each reader retains its
//! format-specific guards. The first file always runs, even on a small machine.
use crate::Reader;
use std::{
    path::Path,
    time::{Duration, Instant},
};

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Cost {
    pub memory: u64,
    pub pdf: bool,
    pub skip: bool,
}

impl Cost {
    pub fn for_path(path: &Path, mode: &str, regex: bool) -> Self {
        let reader = Reader::for_path(path, mode);
        let bytes = |max: u64| path.metadata().map_or(max, |m| m.len());
        let memory = match &reader {
            Reader::Skip => 0,
            Reader::Lines(_) if !regex => 4 * MIB,
            // The largest live regex line is limited to 256 MiB plus decoder buffers.
            Reader::Lines(_) => 4 * MIB + bytes(256 * MIB).saturating_mul(3).min(256 * MIB),
            // Include room for the page and the external Poppler process.
            Reader::Pdf => 256 * MIB,
            Reader::Docx => 160 * MIB,
            Reader::Html | Reader::Markdown => {
                4 * MIB + bytes(1024 * MIB).saturating_mul(160).min(1024 * MIB)
            }
            // Compressed Office files can expand far beyond their size on disk.
            Reader::Office(_) => 256 * MIB,
        };
        Self {
            memory,
            pdf: matches!(reader, Reader::Pdf),
            skip: matches!(reader, Reader::Skip),
        }
    }
}

pub(super) struct Resources {
    pub workers: usize,
    pdf_limit: usize,
    active: usize,
    pdfs: usize,
    reserved: u64,
    budget: u64,
    checked: Instant,
}

impl Resources {
    pub fn new(workers: usize, pdfs: usize) -> Self {
        let workers = if workers == 0 {
            (crate::available_processors() / 2).max(1)
        } else {
            workers.min(crate::available_processors())
        };
        let pdf_limit = if pdfs == 0 { 2 } else { pdfs }.min(workers);
        Self {
            workers,
            pdf_limit,
            active: 0,
            pdfs: 0,
            reserved: 0,
            budget: Self::budget(),
            checked: Instant::now(),
        }
    }

    fn budget() -> u64 {
        // Keep headroom for the interactive preview, OS, and other applications.
        super::memory::available_bytes().unwrap_or(1024 * MIB) / 2
    }
    pub fn refresh(&mut self) {
        if self.checked.elapsed() >= Duration::from_secs(1) {
            self.budget = Self::budget();
            self.checked = Instant::now();
        }
    }
    pub fn allows(&self, cost: Cost) -> bool {
        self.active < self.workers
            && (!cost.pdf || self.pdfs < self.pdf_limit)
            && (self.active == 0 || self.reserved.saturating_add(cost.memory) <= self.budget)
    }
    pub fn start(&mut self, cost: Cost) {
        self.active += 1;
        self.pdfs += usize::from(cost.pdf);
        self.reserved += cost.memory;
    }
    pub fn finish(&mut self, cost: Cost) {
        self.active -= 1;
        self.pdfs -= usize::from(cost.pdf);
        self.reserved -= cost.memory;
    }
    pub fn active(&self) -> usize {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expensive_files_wait_while_independent_smaller_jobs_fit() {
        let mut pool = Resources::new(1, 1);
        pool.workers = 4;
        pool.pdf_limit = 1;
        pool.budget = 400 * MIB;
        let pdf = Cost {
            memory: 256 * MIB,
            pdf: true,
            skip: false,
        };
        let heavy = Cost {
            memory: 500 * MIB,
            ..Default::default()
        };
        let text = Cost {
            memory: 4 * MIB,
            ..Default::default()
        };
        pool.start(pdf);
        assert!(!pool.allows(pdf) && !pool.allows(heavy) && pool.allows(text));
        pool.start(text);
        pool.finish(pdf);
        assert!(pool.allows(pdf) && !pool.allows(heavy));
        pool.finish(text);
        assert!(pool.allows(heavy));
    }
}
