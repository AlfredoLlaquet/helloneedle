// SPDX-License-Identifier: GPL-3.0-or-later
//! Decode an OOXML part incrementally. Word may store its XML in UTF-8 or UTF-16.
use encoding_rs::{Decoder, DecoderResult, Encoding, UTF_16BE, UTF_16LE, UTF_8};
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::ops::Range;

const BUFFER: usize = 64 * 1024;

pub(super) struct XmlInput<R: Read> {
    source: BufReader<io::Chain<Cursor<Vec<u8>>, R>>,
    decoder: Decoder,
    decoded: Box<[u8]>,
    available: Range<usize>,
    finished: bool,
}

impl<R: Read> XmlInput<R> {
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut head = Vec::with_capacity(4);
        input.by_ref().take(4).read_to_end(&mut head)?;
        let (encoding, skip) = Encoding::for_bom(&head).unwrap_or_else(|| {
            if head.starts_with(&[b'<', 0]) {
                (UTF_16LE, 0)
            } else if head.starts_with(&[0, b'<']) {
                (UTF_16BE, 0)
            } else {
                (UTF_8, 0)
            }
        });
        let mut head = Cursor::new(head);
        head.set_position(skip as u64);
        Ok(Self {
            source: BufReader::with_capacity(BUFFER, head.chain(input)),
            decoder: encoding.new_decoder_without_bom_handling(),
            decoded: vec![0; BUFFER].into_boxed_slice(),
            available: 0..0,
            finished: false,
        })
    }
}

impl<R: Read> BufRead for XmlInput<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.available.is_empty() && !self.finished {
            crate::search::checkpoint()?;
            let input = self.source.fill_buf()?;
            let last = input.is_empty();
            let (status, consumed, produced) =
                self.decoder
                    .decode_to_utf8_without_replacement(input, &mut self.decoded, last);
            if matches!(status, DecoderResult::Malformed(_, _)) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid Word XML encoding",
                ));
            }
            self.source.consume(consumed);
            self.available = 0..produced;
            self.finished = last && status == DecoderResult::InputEmpty;
        }
        Ok(&self.decoded[self.available.clone()])
    }

    fn consume(&mut self, count: usize) {
        self.available.start += count;
    }
}

impl<R: Read> Read for XmlInput<R> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = target.len().min(available.len());
        target[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}
