// SPDX-License-Identifier: GPL-3.0-or-later
//! Legacy Word text exports can encode an in-word nonbreaking hyphen as U+001E.
use super::source::NotText;
use anyhow::{ensure, Result};
use regex_syntax::is_word_character;

pub(super) struct WordHyphens {
    left_is_word: bool,
    pending: bool,
}

impl WordHyphens {
    pub fn new(inside_line: bool) -> Self {
        // A search bookmark inside a line may start exactly at a verified hyphen.
        Self {
            left_is_word: inside_line,
            pending: false,
        }
    }

    /// Supply an ordinary hyphen only after validating both neighbors. It
    /// occupies the same number of source bytes and UTF-16 units as U+001E.
    pub fn chunk(
        &mut self,
        mut text: &str,
        mut emit: impl FnMut(&str) -> Result<bool>,
    ) -> Result<bool> {
        if text.is_empty() {
            return Ok(true);
        }
        if self.pending {
            ensure!(text.starts_with(is_word_character), NotText);
            self.pending = false;
            if !emit("-")? {
                return Ok(false);
            }
        }
        while let Some((before, after)) = text.split_once('\x1e') {
            let left = before
                .chars()
                .next_back()
                .map_or(self.left_is_word, is_word_character);
            ensure!(left, NotText);
            if !before.is_empty() && !emit(before)? {
                return Ok(false);
            }
            if after.is_empty() {
                self.pending = true;
                return Ok(true);
            }
            ensure!(after.starts_with(is_word_character), NotText);
            if !emit("-")? {
                return Ok(false);
            }
            self.left_is_word = false;
            text = after;
        }
        self.left_is_word = text.ends_with(is_word_character);
        emit(text)
    }

    pub fn finish(self) -> Result<()> {
        ensure!(!self.pending, NotText);
        Ok(())
    }
}
