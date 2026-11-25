// SPDX-License-Identifier: GPL-3.0-or-later
//! Recognize terminal escape sequences without retaining their contents. Source views
//! keep the original characters; a plain log presents the text a terminal would print.
use anyhow::{ensure, Result};

#[derive(Default, PartialEq)]
enum State {
    #[default]
    Text,
    Escape,
    Parameters,
    Intermediate,
    String,
    StringEscape,
}

#[derive(Default)]
pub(super) struct Ansi {
    state: State,
}

impl Ansi {
    pub(super) fn scan(
        &mut self,
        mut input: &str,
        mut visit: impl FnMut(&str, bool) -> Result<bool>,
    ) -> Result<bool> {
        use State::*;
        while !input.is_empty() {
            if self.state == Text {
                let index = input.find(['\x1b', '\x07']).unwrap_or(input.len());
                if index > 0 && !visit(&input[..index], false)? {
                    return Ok(false);
                }
                input = &input[index..];
                if input.is_empty() {
                    break;
                }
                ensure!(input.starts_with('\x1b'), super::text_stream::NotText);
                self.state = Escape;
                if !visit(&input[..1], true)? {
                    return Ok(false);
                }
                input = &input[1..];
            }
            let mut end = 0;
            for (i, ch) in input.char_indices() {
                self.state = match (&self.state, ch) {
                    (Escape, '[') => Parameters,
                    (Escape, ']') => String,
                    (Escape, '7' | '8' | '=' | '>' | 'D' | 'E' | 'H' | 'M' | 'c') => Text,
                    (Parameters, '0'..='?') => Parameters,
                    (Parameters | Intermediate, ' '..='/') => Intermediate,
                    (Parameters | Intermediate, '@'..='~') => Text,
                    (String, '\x07') | (StringEscape, '\\') => Text,
                    (String, '\x1b') => StringEscape,
                    (String, c) if !c.is_control() => String,
                    _ => return Err(super::text_stream::NotText.into()),
                };
                end = i + ch.len_utf8();
                if self.state == Text {
                    break;
                }
            }
            if end > 0 {
                if !visit(&input[..end], true)? {
                    return Ok(false);
                }
                input = &input[end..];
            }
        }
        Ok(true)
    }

    pub(super) fn finish(self) -> Result<()> {
        ensure!(self.state == State::Text, super::text_stream::NotText);
        Ok(())
    }
}
