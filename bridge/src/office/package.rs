// SPDX-License-Identifier: GPL-3.0-or-later
use super::{MAX_TEXT_BYTES, MAX_XML_NODES};
use anyhow::{anyhow, bail, Result};
use roxmltree::{Document, ParsingOptions};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
};
use zip::{result::ZipError, ZipArchive};

const MAX_DIRECTORY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: u64 = 20_000;

pub(super) fn parse(xml: &str) -> Result<Document<'_>> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_XML_NODES,
            ..Default::default()
        },
    )
    .map_err(|e| match e {
        roxmltree::Error::NodesLimitReached | roxmltree::Error::AttributesLimitReached => {
            anyhow!("resource_limit: XML structure")
        }
        _ => anyhow!("corrupt_document: invalid XML: {e}"),
    })
}

pub(super) struct Package {
    zip: ZipArchive<File>,
}
#[derive(Clone)]
pub(super) struct Rel {
    pub kind: String,
    pub part: Option<String>,
}

impl Package {
    pub fn open(mut file: File) -> Result<Self> {
        // Inspect the fixed-size ZIP footer before the library reserves its central directory.
        let len = file.metadata()?.len();
        let mut header = [0u8; 8];
        if len >= 8 {
            file.read_exact(&mut header)?;
            if header == [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1] {
                // Encrypted OOXML wraps its payload in two named streams in an OLE directory.
                // Its entries are 128 bytes and start on a sector boundary. Read bounded
                // chunks instead of loading an entire legacy binary to identify those names.
                let mut buf = [0; 8192];
                let mut found_info = false;
                let mut found_package = false;
                file.rewind()?;
                let mut left = len;
                while left > 0 {
                    let chunk = left.min(buf.len() as u64) as usize;
                    file.read_exact(&mut buf[..chunk])?;
                    for entry in buf[..chunk].chunks_exact(128) {
                        if entry[66] != 2 {
                            continue;
                        }
                        let chars = u16::from_le_bytes([entry[64], entry[65]]) as usize;
                        let is_stream = |word: &str| {
                            chars == (word.len() + 1) * 2
                                && entry[..chars]
                                    .chunks_exact(2)
                                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                                    .eq(word.encode_utf16().chain(Some(0)))
                        };
                        found_info |= is_stream("EncryptionInfo");
                        found_package |= is_stream("EncryptedPackage");
                    }
                    if found_info && found_package {
                        bail!("encrypted_document: Office encryption container");
                    }
                    left -= chunk as u64;
                }
                bail!("unsupported_document: legacy compound file with an XML document extension");
            }
        }
        let len_tail = len.min(65535 + 22 + 20) as usize;
        file.seek(SeekFrom::End(-(len_tail as i64)))?;
        let mut tail = vec![0; len_tail];
        file.read_exact(&mut tail)?;
        let pos = (0..=len_tail.saturating_sub(22))
            .rev()
            .find(|&i| {
                let Some(footer) = tail.get(i..i + 22) else {
                    return false;
                };
                footer.starts_with(b"PK\x05\x06")
                    && i + 22 + u16::from_le_bytes([footer[20], footer[21]]) as usize == len_tail
            })
            .ok_or_else(|| anyhow!("corrupt_document: invalid ZIP directory"))?;
        let (this_disk, count, directory_bytes) =
            if pos >= 20 && tail[pos - 20..].starts_with(b"PK\x06\x07") {
                let locator = &tail[pos - 20..pos];
                let offset = u64::from_le_bytes(locator[8..16].try_into()?);
                let locator_start = len - (len_tail - pos + 20) as u64;
                if offset.saturating_add(56) > locator_start {
                    bail!("corrupt_document: ZIP64 directory position");
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut record = [0; 56];
                file.read_exact(&mut record)?;
                let size = u64::from_le_bytes(record[4..12].try_into()?);
                if !record.starts_with(b"PK\x06\x06")
                    || size < 44
                    || size != locator_start - offset - 12
                {
                    bail!("corrupt_document: ZIP64 directory record");
                }
                if size > MAX_DIRECTORY_BYTES {
                    bail!("resource_limit: ZIP64 directory");
                }
                (
                    u64::from_le_bytes(record[24..32].try_into()?),
                    u64::from_le_bytes(record[32..40].try_into()?),
                    u64::from_le_bytes(record[40..48].try_into()?),
                )
            } else {
                (
                    u16::from_le_bytes([tail[pos + 8], tail[pos + 9]]) as u64,
                    u16::from_le_bytes([tail[pos + 10], tail[pos + 11]]) as u64,
                    u32::from_le_bytes(tail[pos + 12..pos + 16].try_into()?) as u64,
                )
            };
        if this_disk != count {
            bail!("unsupported_document: multipart document archive");
        }
        if count > MAX_ENTRIES || directory_bytes > MAX_DIRECTORY_BYTES {
            bail!("resource_limit: ZIP directory");
        }
        file.rewind()?;
        let config = zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        };
        let zip = ZipArchive::with_config(config, file)
            .map_err(|e| anyhow!("corrupt_document: invalid ZIP: {e}"))?;
        if zip.len() as u64 > MAX_ENTRIES {
            bail!("resource_limit: ZIP entry count");
        }
        Ok(Self { zip })
    }
    pub fn required(&mut self, part: &str) -> Result<String> {
        self.optional(part)?
            .ok_or_else(|| anyhow!("corrupt_document: missing package part {part}"))
    }
    fn entry(&mut self, part: &str) -> Result<Option<zip::read::ZipFile<'_, File>>> {
        match self.zip.by_name(part) {
            Ok(f) => Ok(Some(f)),
            Err(ZipError::FileNotFound) => Ok(None),
            Err(e) if e.to_string().to_ascii_lowercase().contains("password") => {
                bail!("encrypted_document: ZIP entry")
            }
            Err(ZipError::UnsupportedArchive(_)) => {
                bail!("unsupported_document: ZIP compression or encryption")
            }
            Err(e) => bail!("corrupt_document: ZIP entry: {e}"),
        }
    }

    pub fn stream(&mut self, part: &str) -> Result<impl Read + '_> {
        self.entry(part)?
            .ok_or_else(|| anyhow!("corrupt_document: missing package part {part}"))
    }

    pub fn optional(&mut self, part: &str) -> Result<Option<String>> {
        let remaining = MAX_TEXT_BYTES as u64;
        let Some(file) = self.entry(part)? else {
            return Ok(None);
        };
        if file.size() > remaining {
            bail!("resource_limit: expanded ZIP data");
        }
        let mut data = Vec::new();
        file.take(remaining + 1)
            .read_to_end(&mut data)
            .map_err(|e| anyhow!("corrupt_document: ZIP entry: {e}"))?;
        if data.len() as u64 > remaining {
            bail!("resource_limit: expanded ZIP data");
        }
        Ok(Some(super::xml_encoding::decode(&data)?))
    }
    pub fn rels(&mut self, part: &str) -> Result<HashMap<String, Rel>> {
        let path = if part.is_empty() {
            "_rels/.rels".into()
        } else if let Some((dir, file)) = part.rsplit_once('/') {
            format!("{dir}/_rels/{file}.rels")
        } else {
            format!("_rels/{part}.rels")
        };
        let mut rels = HashMap::new();
        let Some(xml) = self.optional(&path)? else {
            return Ok(rels);
        };
        let doc = parse(&xml)?;
        if doc.root_element().tag_name().name() != "Relationships" {
            bail!("corrupt_document: relationships root");
        }
        for r in doc.root_element().children().filter(|n| {
            n.has_tag_name((
                "http://schemas.openxmlformats.org/package/2006/relationships",
                "Relationship",
            ))
        }) {
            let (Some(id), Some(kind), Some(target)) = (
                r.attribute("Id"),
                r.attribute("Type"),
                r.attribute("Target"),
            ) else {
                bail!("corrupt_document: incomplete relationship");
            };
            let resolved = if r.attribute("TargetMode") == Some("External") {
                None
            } else {
                normalize_part(part, target)
            };
            let kind = kind.rsplit('/').next().unwrap_or("").to_owned();
            if rels
                .insert(
                    id.into(),
                    Rel {
                        kind,
                        part: resolved,
                    },
                )
                .is_some()
            {
                bail!("corrupt_document: duplicate relationship ID");
            }
        }
        Ok(rels)
    }
}

fn normalize_part(base: &str, target: &str) -> Option<String> {
    if target.is_empty() || target.starts_with("//") || target.contains(['\\', '?', '#', ':']) {
        return None;
    }
    let mut segments: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base.rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())
            .unwrap_or_default()
    };
    for s in target.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            part => segments.push(part),
        }
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("/"))
    }
}
