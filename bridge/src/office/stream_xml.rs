// SPDX-License-Identifier: GPL-3.0-or-later
//! Pull XML events with a bounded token and active stack. Readers may materialize a
//! single logical unit (e.g. a formatted paragraph) with all inherited namespaces.
use super::{xml_encoding::Input, MAX_TEXT_BYTES};
use anyhow::{anyhow, bail, ensure, Result};
use quick_xml::{events::Event as Raw, name::ResolveResult, reader::NsReader, Writer};
use std::{
    collections::HashSet,
    fmt::Display,
    io::{Read, Take},
    str,
};

pub(super) struct Element {
    pub namespace: String,
    pub name: String,
    attributes: Vec<(String, String, String)>,
}

impl Element {
    pub fn is(&self, namespace: &str, local: &str) -> bool {
        self.namespace == namespace && self.name == local
    }
    pub fn attr(&self, namespace: &str, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(ns, name, _)| ns == namespace && name == local)
            .map(|(_, _, value)| value.as_str())
    }
}

pub(super) enum Event {
    Start(Element),
    Text(String),
    End,
}

struct Frame {
    bytes: usize,
    namespaces: Vec<(String, String)>,
}

pub(super) struct Xml<R: Read> {
    reader: NsReader<Take<Input<R>>>,
    buffer: Vec<u8>,
    raw: Option<Raw<'static>>,
    frames: Vec<Frame>,
    retained: usize,
    root_seen: bool,
}

fn invalid(error: impl Display) -> anyhow::Error {
    anyhow!("corrupt_document: invalid Office XML: {error}")
}

fn namespace(resolved: ResolveResult<'_>) -> Result<String> {
    Ok(match resolved {
        ResolveResult::Unbound => String::new(),
        ResolveResult::Bound(name) => str::from_utf8(name.as_ref()).map_err(invalid)?.to_string(),
        ResolveResult::Unknown(_) => bail!("corrupt_document: undeclared XML prefix"),
    })
}

impl<R: Read> Xml<R> {
    pub fn new(source: R) -> Result<Self> {
        let mut reader = NsReader::from_reader(Input::new(source)?.take(MAX_TEXT_BYTES as u64));
        reader.config_mut().expand_empty_elements = true;
        Ok(Self {
            reader,
            buffer: Vec::new(),
            raw: None,
            frames: Vec::new(),
            retained: 0,
            root_seen: false,
        })
    }

    pub fn next(&mut self) -> Result<Option<Event>> {
        self.advance(false)
    }

    fn advance(&mut self, capture: bool) -> Result<Option<Event>> {
        loop {
            crate::search::checkpoint()?;
            self.raw = None;
            self.reader.get_mut().set_limit(MAX_TEXT_BYTES as u64);
            self.buffer.clear();
            let event = self.reader.read_event_into(&mut self.buffer);
            ensure!(
                self.reader.get_ref().limit() != 0,
                "resource_limit: Office XML token"
            );
            let event = event.map_err(invalid)?;
            let value = match &event {
                Raw::Start(tag) => {
                    if self.frames.is_empty() {
                        ensure!(!self.root_seen, "corrupt_document: multiple XML roots");
                        self.root_seen = true;
                    }
                    self.retained += tag.len();
                    ensure!(
                        self.frames.len() < 512 && self.retained <= MAX_TEXT_BYTES,
                        "resource_limit: active Office XML markup"
                    );
                    let (ns, name) = self.reader.resolve_element(tag.name());
                    let mut element = Element {
                        namespace: namespace(ns)?,
                        name: str::from_utf8(name.as_ref()).map_err(invalid)?.to_string(),
                        attributes: Vec::new(),
                    };
                    let mut frame = Frame {
                        bytes: tag.len(),
                        namespaces: Vec::new(),
                    };
                    for attribute in tag.attributes() {
                        let a = attribute.map_err(invalid)?;
                        let key = str::from_utf8(a.key.as_ref()).map_err(invalid)?;
                        let value = a
                            .decode_and_unescape_value(self.reader.decoder())
                            .map_err(invalid)?
                            .into_owned();
                        if key == "xmlns" || key.starts_with("xmlns:") {
                            frame.namespaces.push((key.to_owned(), value));
                        } else {
                            let (ns, name) = self.reader.resolve_attribute(a.key);
                            element.attributes.push((
                                namespace(ns)?,
                                str::from_utf8(name.as_ref()).map_err(invalid)?.to_owned(),
                                value,
                            ));
                        }
                    }
                    self.frames.push(frame);
                    Event::Start(element)
                }
                Raw::End(_) => {
                    let frame = self
                        .frames
                        .pop()
                        .ok_or_else(|| invalid("unexpected closing element"))?;
                    self.retained -= frame.bytes;
                    Event::End
                }
                Raw::Text(value) => {
                    Event::Text(value.xml10_content().map_err(invalid)?.into_owned())
                }
                Raw::CData(value) => {
                    Event::Text(value.xml10_content().map_err(invalid)?.into_owned())
                }
                Raw::GeneralRef(value) => {
                    if let Some(c) = value.resolve_char_ref().map_err(invalid)? {
                        Event::Text(c.to_string())
                    } else {
                        let entity = value.decode().map_err(invalid)?;
                        Event::Text(
                            quick_xml::escape::resolve_xml_entity(&entity)
                                .ok_or_else(|| invalid("unknown entity"))?
                                .to_owned(),
                        )
                    }
                }
                Raw::DocType(_) => bail!("corrupt_document: DTD in Office XML"),
                Raw::Eof => {
                    ensure!(
                        self.root_seen && self.frames.is_empty(),
                        "corrupt_document: incomplete Office XML"
                    );
                    return Ok(None);
                }
                _ => continue,
            };
            if let Event::Text(text) = &value {
                ensure!(
                    !self.frames.is_empty() || text.trim().is_empty(),
                    "corrupt_document: text outside XML root"
                );
            }
            if capture || matches!(value, Event::Start(_)) {
                self.raw = Some(event.into_owned());
            }
            return Ok(Some(value));
        }
    }

    /// Consume the element whose Start was just returned. Expand its outer tag's
    /// inherited bindings once so even prefix redefinitions inside the fragment work.
    pub fn subtree(&mut self) -> Result<String> {
        let Some(Raw::Start(mut tag)) = self.raw.take() else {
            bail!("internal: missing Office XML start tag")
        };
        let depth = self.frames.len();
        let mut seen = HashSet::new();
        for (i, frame) in self.frames.iter().enumerate().rev() {
            for (key, value) in &frame.namespaces {
                if seen.insert(key.as_str()) && i + 1 < depth {
                    tag.push_attribute((key.as_str(), value.as_str()));
                }
            }
        }
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Raw::Start(tag))?;
        while self.frames.len() >= depth {
            self.advance(true)?
                .ok_or_else(|| invalid("unterminated XML fragment"))?;
            writer.write_event(
                self.raw
                    .take()
                    .ok_or_else(|| invalid("missing XML event"))?,
            )?;
            ensure!(
                writer.get_ref().len() <= MAX_TEXT_BYTES,
                "resource_limit: Office XML working unit"
            );
        }
        Ok(String::from_utf8(writer.into_inner()).map_err(invalid)?)
    }
}
