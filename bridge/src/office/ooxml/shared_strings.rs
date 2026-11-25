// SPDX-License-Identifier: GPL-3.0-or-later
//! A shared-string index has arbitrary lookup order. Keep small workbooks in
//! memory and spill both bytes and offsets when their working set grows.
use anyhow::{anyhow, ensure, Result};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_file() -> io::Result<File> {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    loop {
        let path = std::env::temp_dir().join(format!(
            "helloneedle-strings-{}-{stamp}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
            options
                .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
                .share_mode(0);
        }
        let file = match options.open(&path) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            other => other?,
        };
        #[cfg(unix)]
        std::fs::remove_file(&path)?;
        return Ok(file);
    }
}

struct Spool {
    data: BufWriter<File>,
    index: BufWriter<File>,
    offset: u64,
}

impl Spool {
    fn new() -> Result<Self> {
        Ok(Self {
            data: BufWriter::new(temporary_file()?),
            index: BufWriter::new(temporary_file()?),
            offset: 0,
        })
    }
    fn append(&mut self, text: &str) -> Result<()> {
        self.index.write_all(&self.offset.to_le_bytes())?;
        self.index.write_all(&(text.len() as u64).to_le_bytes())?;
        self.data.write_all(text.as_bytes())?;
        self.offset = self
            .offset
            .checked_add(text.len() as u64)
            .ok_or_else(|| anyhow!("resource_limit: shared string spool length"))?;
        Ok(())
    }
}

pub(super) struct Builder {
    small: Vec<String>,
    used: usize,
    limit: usize,
    spool: Option<Spool>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            small: Vec::new(),
            used: 0,
            limit: 1024 * 1024,
            spool: None,
        }
    }
}

impl Builder {
    pub fn append(&mut self, value: String) -> Result<()> {
        if let Some(spool) = self.spool.as_mut() {
            return spool.append(&value);
        }
        self.used += std::mem::size_of::<String>() + value.capacity();
        if self.used <= self.limit {
            self.small.push(value);
        } else {
            let mut spool = Spool::new()?;
            for s in std::mem::take(&mut self.small) {
                spool.append(&s)?;
            }
            spool.append(&value)?;
            self.spool = Some(spool);
        }
        Ok(())
    }

    pub fn finish(self) -> Result<SharedStrings> {
        match self.spool {
            None => Ok(SharedStrings::Memory(self.small)),
            Some(spool) => Ok(SharedStrings::Disk {
                index: BufReader::new(spool.index.into_inner().map_err(|e| e.into_error())?),
                data: BufReader::new(spool.data.into_inner().map_err(|e| e.into_error())?),
            }),
        }
    }
}

pub(super) enum SharedStrings {
    Memory(Vec<String>),
    Disk {
        index: BufReader<File>,
        data: BufReader<File>,
    },
}

impl SharedStrings {
    pub fn get(&mut self, n: &str) -> Result<String> {
        let invalid = || anyhow!("corrupt_document: invalid shared string index");
        let number: usize = n.trim().parse().map_err(|_| invalid())?;
        match self {
            Self::Memory(values) => values.get(number).cloned().ok_or_else(invalid),
            Self::Disk { index, data } => {
                let pos = (number as u64).checked_mul(16).ok_or_else(invalid)?;
                index.seek(SeekFrom::Start(pos))?;
                let mut record = [0; 16];
                index.read_exact(&mut record).map_err(|e| {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        invalid()
                    } else {
                        e.into()
                    }
                })?;
                let offset = u64::from_le_bytes(record[..8].try_into()?);
                let size: usize = u64::from_le_bytes(record[8..].try_into()?).try_into()?;
                ensure!(
                    size <= super::MAX_TEXT_BYTES,
                    "resource_limit: spreadsheet shared string"
                );
                data.seek(SeekFrom::Start(offset))?;
                let mut bytes = vec![0; size];
                data.read_exact(&mut bytes)?;
                Ok(String::from_utf8(bytes)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spill_handles_empty_duplicate_and_random_lookup() {
        let mut builder = Builder {
            limit: 3,
            ..Builder::default()
        };
        for s in ["", "alpha", "beta", "alpha", "末"] {
            builder.append(s.to_string()).unwrap();
        }
        let mut strings = builder.finish().unwrap();
        assert!(matches!(strings, SharedStrings::Disk { .. }));
        for (i, s) in [(4, "末"), (0, ""), (3, "alpha"), (2, "beta"), (1, "alpha")] {
            assert_eq!(strings.get(&i.to_string()).unwrap(), s);
        }
        assert!(strings.get("5").is_err());
    }
}
