// SPDX-License-Identifier: GPL-3.0-or-later
//! Read the Document representation of HTML without retaining embedded resources.
use super::{encoding, markup};
use crate::{Block, Document};
use anyhow::Result;
use encoding_rs::{Decoder, DecoderResult, Encoding, UTF_8, WINDOWS_1252};
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    states::RawKind, Doctype, Tag, TagKind, Token as TreeToken, TokenSink, TokenSinkResult,
};
use html5ever::tree_builder::{TreeBuilder, TreeSink};
use html5ever::{Attribute, LocalName, QualName};
use html5gum::{DefaultEmitter, Emitter, IoReader, State, Token, Tokenizer};
use scraper::{Html, HtmlTreeSink};
use std::cell::Cell;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

pub(super) const DOCUMENT_MEMORY: usize = 1024 * 1024 * 1024;
const LIMIT_MESSAGE: &str = "too_large: HTML exceeds the document memory budget";
const ENCODING_ERROR: &str = "unsupported_encoding: invalid HTML text encoding";
const NODE_RESERVATION: usize = 512;
const ATTRIBUTE_RESERVATION: usize = 192;
// Account for the tokenizer's growing buffer, token, and the tree builder's copy.
const TEXT_COPIES: usize = 4;

// Conservative working-set reservations include tokenizer buffers and tree storage,
// then the text, rich fragments and per-block metadata that coexist with the tree.
// The fixed-size decoder and I/O buffers do not grow with the input.
pub(super) struct Budget {
    remaining: Cell<usize>,
    exceeded: Cell<bool>,
}

impl Budget {
    fn new(bytes: usize) -> Self {
        Self {
            remaining: Cell::new(bytes),
            exceeded: Cell::new(false),
        }
    }

    fn fits(&self, bytes: usize) -> bool {
        if bytes > self.remaining.get() {
            self.exceeded.set(true);
        }
        !self.exceeded.get()
    }

    fn charge(&self, bytes: usize) {
        if self.fits(bytes) {
            self.remaining.set(self.remaining.get() - bytes);
        }
    }

    pub(super) fn reserve(&self, bytes: usize) -> Result<()> {
        self.charge(bytes);
        self.check()?;
        Ok(())
    }

    pub(super) fn release(&self, bytes: usize) {
        self.remaining.set(self.remaining.get() + bytes);
    }

    fn check(&self) -> io::Result<()> {
        if self.exceeded.get() {
            return Err(io::Error::other(LIMIT_MESSAGE));
        }
        Ok(())
    }
}

// BOM and the initial HTML charset declaration are inspected before tokenization.
// Decode a bounded buffer at a time, including encodings with state across reads.
enum HtmlCharset {
    Selected(&'static Encoding),
    WindowsFallback,
}

struct Decoded<'a, R: Read> {
    input: BufReader<R>,
    decoder: Decoder,
    windows_fallback: bool,
    malformed: &'a Cell<bool>,
    buffer: Box<[u8; 32 * 1024]>,
    available: std::ops::Range<usize>,
    finished: bool,
    budget: &'a Budget,
}

impl<'a, R: Read> Decoded<'a, R> {
    fn new(input: R, charset: HtmlCharset, budget: &'a Budget, malformed: &'a Cell<bool>) -> Self {
        let encoding = match charset {
            HtmlCharset::Selected(enc) => enc,
            HtmlCharset::WindowsFallback => WINDOWS_1252,
        };
        Self {
            input: BufReader::with_capacity(64 * 1024, input),
            windows_fallback: matches!(charset, HtmlCharset::WindowsFallback),
            malformed,
            decoder: encoding.new_decoder_without_bom_handling(),
            buffer: Box::new([0; 32 * 1024]),
            available: 0..0,
            finished: false,
            budget,
        }
    }
}

impl<R: Read> Read for Decoded<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.budget.check()?;
        while self.available.is_empty() && !self.finished {
            crate::search::checkpoint()?;
            let input = self.input.fill_buf()?;
            let last = input.is_empty();
            let (status, consumed, produced) =
                self.decoder
                    .decode_to_utf8_without_replacement(input, self.buffer.as_mut(), last);
            // Saved web pages may quote control characters in shell examples. They are
            // valid decoded characters and the HTML tokenizer can handle them. Keep
            // the existing NUL guard for binary data masquerading as HTML.
            if matches!(status, DecoderResult::Malformed(_, _)) {
                self.malformed.set(true);
                return Err(io::Error::new(io::ErrorKind::InvalidData, ENCODING_ERROR));
            }
            if self.buffer[..produced].contains(&0)
                || (self.windows_fallback && encoding::windows_controls(&input[..consumed]))
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, ENCODING_ERROR));
            }
            self.input.consume(consumed);
            self.available = 0..produced;
            self.finished = last && status == DecoderResult::InputEmpty;
        }
        let n = output.len().min(self.available.len());
        output[..n].copy_from_slice(&self.buffer[self.available.start..self.available.start + n]);
        self.available.start += n;
        Ok(n)
    }
}

pub(super) fn read(input: impl Read + Seek) -> Result<Document> {
    read_with_budget(input, DOCUMENT_MEMORY)
}

fn read_with_budget(mut input: impl Read + Seek, limit: usize) -> Result<Document> {
    let mut head = Vec::with_capacity(1024);
    crate::search::checkpoint()?;
    input.by_ref().take(1024).read_to_end(&mut head)?;
    let (marked, skip) = encoding::marked(&head, true);
    input.seek(SeekFrom::Start(skip as u64))?;
    let malformed = Cell::new(false);
    let first = read_as(
        &mut input,
        HtmlCharset::Selected(marked.unwrap_or(UTF_8)),
        limit,
        &malformed,
    );
    // Format projection delivers hits only after the tree has been built. If
    // unmarked UTF-8 fails, its partial tree is already dropped and we replay
    // the same bytes exactly once. ASCII and valid UTF-8 need no scan ahead.
    if marked.is_none() && malformed.get() {
        input.rewind()?;
        return read_as(input, HtmlCharset::WindowsFallback, limit, &malformed);
    }
    first
}

fn read_as(
    input: impl Read,
    charset: HtmlCharset,
    limit: usize,
    malformed: &Cell<bool>,
) -> Result<Document> {
    let mut blocks = Vec::new();
    visit_as(input, charset, limit, malformed, false, &mut |block| {
        blocks.push(block);
        Ok(true)
    })?;
    Ok(Document {
        kind: "html".into(),
        blocks,
    })
}

/// Generated CommonMark HTML is already decoded. Preserve its HTML5 tree semantics,
/// including raw HTML, and send completed projected blocks to the caller.
pub(super) fn rendered(
    input: &[u8],
    available: usize,
    visit: &mut dyn FnMut(Block) -> Result<bool>,
) -> Result<()> {
    visit_as(
        input,
        HtmlCharset::Selected(UTF_8),
        available,
        &Cell::new(false),
        true,
        visit,
    )
}

fn visit_as(
    input: impl Read,
    charset: HtmlCharset,
    limit: usize,
    malformed: &Cell<bool>,
    stream_blocks: bool,
    visit: &mut dyn FnMut(Block) -> Result<bool>,
) -> Result<()> {
    let budget = Budget::new(limit);
    let input = IoReader::new(Decoded::new(input, charset, &budget, malformed));
    let tree = TreeBuilder::new(HtmlTreeSink::new(Html::new_document()), Default::default());
    let context = Context::default();
    let emitter = Filter::new(&budget, &context);
    let mut tokenizer = Tokenizer::new_with_emitter(input, emitter);
    let mut svg_depth: usize = 0;
    let mut svg_html_depth: usize = 0;
    while let Some(token) = tokenizer.next() {
        let token = token?;
        budget.check()?;
        // An inline SVG is an image too. Keep its root in the HTML tree so that
        // its neighbors retain their normal structure, but discard the drawing.
        if svg_depth != 0 {
            // Within an SVG integration point, HTML raw text may contain a
            // literal "</svg>" (for instance in JS, CSS, or a textarea).
            if svg_html_depth != 0 {
                if let Token::StartTag(tag) = &token {
                    if let Some(state) = html5gum::naive_next_state(&tag.name) {
                        tokenizer.set_state(state);
                    }
                }
            }
            match &token {
                Token::StartTag(tag) if tag.name == b"svg" && !tag.self_closing => svg_depth += 1,
                Token::StartTag(tag)
                    if matches!(&tag.name[..], b"foreignobject" | b"desc" | b"title")
                        && !tag.self_closing =>
                {
                    svg_html_depth += 1
                }
                Token::EndTag(tag)
                    if matches!(&tag.name[..], b"foreignobject" | b"desc" | b"title") =>
                {
                    svg_html_depth = svg_html_depth.saturating_sub(1)
                }
                Token::EndTag(tag) if tag.name == b"svg" => svg_depth -= 1,
                // HTML5 allows ordinary HTML to break out of unclosed foreign content.
                Token::StartTag(tag) if svg_html_depth == 0 && html_breakout(tag) => {
                    svg_depth = 0;
                }
                _ => {}
            }
            if svg_depth != 0 {
                continue;
            }
            context.image.set(false);
            svg_html_depth = 0;
        }
        // Only tokens handed to the tree builder become persistent structure.
        // End tags can also create nodes during HTML5 error recovery.
        let token_size = match &token {
            Token::StartTag(t) => t.attributes.iter().fold(
                NODE_RESERVATION + TEXT_COPIES * t.name.len(),
                |size, (key, value)| {
                    size.saturating_add(
                        ATTRIBUTE_RESERVATION + TEXT_COPIES * (key.len() + value.len()),
                    )
                },
            ),
            Token::EndTag(_) | Token::Comment(_) | Token::ProcessingInstruction(_) => {
                NODE_RESERVATION
            }
            _ => 0,
        };
        budget.reserve(token_size)?;
        let mut opaque_raw_text = false;
        let mut svg_root = false;
        let token = match token {
            Token::StartTag(tag) => {
                opaque_raw_text = matches!(
                    &tag.name[..],
                    b"style" | b"script" | b"iframe" | b"noscript" | b"textarea"
                );
                svg_root = tag.name == b"svg" && !tag.self_closing;
                let mut attrs = Vec::with_capacity(tag.attributes.len());
                for (name, value) in tag.attributes {
                    attrs.push(Attribute {
                        name: QualName::new(
                            None,
                            Default::default(),
                            LocalName::from(utf8(&name)?),
                        ),
                        value: StrTendril::from_slice(utf8(&value)?),
                    });
                }
                TreeToken::TagToken(Tag {
                    kind: TagKind::StartTag,
                    name: LocalName::from(utf8(&tag.name)?),
                    self_closing: tag.self_closing,
                    attrs,
                })
            }
            Token::EndTag(tag) => TreeToken::TagToken(Tag {
                kind: TagKind::EndTag,
                name: LocalName::from(utf8(&tag.name)?),
                self_closing: false,
                attrs: vec![],
            }),
            Token::String(text) => TreeToken::CharacterTokens(StrTendril::from_slice(utf8(&text)?)),
            Token::Doctype(d) => TreeToken::DoctypeToken(Doctype {
                name: if d.name.is_empty() {
                    None
                } else {
                    Some(StrTendril::from_slice(utf8(&d.name)?))
                },
                public_id: d
                    .public_identifier
                    .as_deref()
                    .map(|v| utf8(v).map(StrTendril::from_slice))
                    .transpose()?,
                system_id: d
                    .system_identifier
                    .as_deref()
                    .map(|v| utf8(v).map(StrTendril::from_slice))
                    .transpose()?,
                force_quirks: d.force_quirks,
            }),
            // Comments have no visible content, but are still markup boundaries.
            Token::Comment(_) | Token::ProcessingInstruction(_) => {
                TreeToken::CommentToken(StrTendril::new())
            }
            Token::Error(_) => continue,
        };
        let result = tree.process_token(token, 1);
        context
            .foreign
            .set(tree.adjusted_current_node_present_but_not_in_html_namespace());
        if svg_root && context.foreign.get() {
            svg_depth = 1;
            context.image.set(true);
        }
        let state = match result {
            TokenSinkResult::RawData(RawKind::Rcdata) => State::RcData,
            TokenSinkResult::RawData(RawKind::Rawtext) => State::RawText,
            TokenSinkResult::RawData(RawKind::ScriptData | RawKind::ScriptDataEscaped(_)) => {
                State::ScriptData
            }
            TokenSinkResult::Plaintext => State::PlainText,
            TokenSinkResult::Continue | TokenSinkResult::Script(_) => State::Data,
        };
        if state != State::Data {
            tokenizer.set_state(state);
        }
        context.raw.set(opaque_raw_text && state != State::Data);
    }
    budget.check()?;
    let _ = tree.process_token(TreeToken::EOFToken, 1);
    tree.end();
    let doc = tree.sink.finish();
    markup::project_tree(&doc, Some(&budget), stream_blocks, visit)
}

fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| anyhow::anyhow!(ENCODING_ERROR))
}

fn html_breakout(tag: &html5gum::StartTag<()>) -> bool {
    let name = tag.name.as_slice();
    matches!(
        name,
        b"b" | b"big"
            | b"blockquote"
            | b"body"
            | b"br"
            | b"center"
            | b"code"
            | b"dd"
            | b"div"
            | b"dl"
            | b"dt"
            | b"em"
            | b"embed"
            | b"h1"
            | b"h2"
            | b"h3"
            | b"h4"
            | b"h5"
            | b"h6"
            | b"head"
            | b"hr"
            | b"i"
            | b"img"
            | b"li"
            | b"listing"
            | b"menu"
            | b"meta"
            | b"nobr"
            | b"ol"
            | b"p"
            | b"pre"
            | b"ruby"
            | b"s"
            | b"small"
            | b"span"
            | b"strong"
            | b"strike"
            | b"sub"
            | b"sup"
            | b"table"
            | b"tt"
            | b"u"
            | b"ul"
            | b"var"
    ) || name == b"font"
        && [b"color".as_slice(), b"face", b"size"]
            .iter()
            .any(|n| tag.attributes.contains_key(*n))
}

#[derive(Default)]
struct Context {
    raw: Cell<bool>,
    image: Cell<bool>,
    foreign: Cell<bool>,
}

// Style values can themselves contain arbitrarily long embedded images. Only
// display/visibility affect our projection. Keep a short candidate declaration
// and let the same predicate used by Markdown decide whether it hides the node.
#[derive(Default)]
struct Visibility {
    candidate: Vec<u8>,
    ignored: bool,
    hidden: bool,
}
impl Visibility {
    fn push(&mut self, data: &[u8]) {
        for part in data.split_inclusive(|&byte| byte == b';') {
            let end = part.last() == Some(&b';');
            let part = if end { &part[..part.len() - 1] } else { part };
            if !self.ignored && !self.hidden {
                for &c in part {
                    if c == b'!' || self.candidate.len() == 32 {
                        self.ignored = true;
                        break;
                    }
                    if c.is_ascii_whitespace() {
                        if self.candidate.is_empty() || self.candidate.last() == Some(&b' ') {
                            continue;
                        }
                        self.candidate.push(b' ');
                    } else {
                        self.candidate.push(c);
                    }
                }
            }
            if end {
                self.finish_declaration();
            }
        }
    }
    fn finish_declaration(&mut self) {
        if let Ok(s) = std::str::from_utf8(&self.candidate) {
            self.hidden |= markup::hides(s);
        }
        self.ignored = false;
        self.candidate.clear();
    }
}

// Only values used by the tree builder or searchable projection need storage.
// Attribute names remain available for presence tests (including "hidden").
struct Filter<'a> {
    inner: DefaultEmitter,
    budget: &'a Budget,
    context: &'a Context,
    key: Vec<u8>,
    style: Visibility,
    token_bytes: usize,
}

impl<'a> Filter<'a> {
    fn new(budget: &'a Budget, context: &'a Context) -> Self {
        Self {
            inner: DefaultEmitter::default(),
            budget,
            context,
            key: Vec::new(),
            style: Visibility::default(),
            token_bytes: 0,
        }
    }
    fn stage(&mut self, bytes: usize) -> bool {
        self.token_bytes = self.token_bytes.saturating_add(bytes);
        self.budget.fits(self.token_bytes)
    }
    fn finish_style(&mut self) {
        if self.key == b"style" {
            self.style.finish_declaration();
            if self.style.hidden {
                Emitter::push_attribute_value(&mut self.inner, b"display:none");
            }
        }
    }
}

macro_rules! buffered_token_field {
    ($method:ident) => {
        fn $method(&mut self, value: &[u8]) {
            if self.stage(value.len().saturating_mul(TEXT_COPIES)) {
                Emitter::$method(&mut self.inner, value);
            }
        }
    };
}

impl html5gum::ForwardingEmitter for Filter<'_> {
    type Token = Token<()>;
    fn inner(&mut self) -> &mut impl Emitter<Token = Self::Token> {
        &mut self.inner
    }
    fn should_emit_errors(&mut self) -> bool {
        false
    }
    fn emit_error(&mut self, _: html5gum::Error) {}
    fn adjusted_current_node_present_but_not_in_html_namespace(&mut self) -> bool {
        self.context.foreign.get()
    }
    fn init_start_tag(&mut self) {
        self.token_bytes = NODE_RESERVATION;
        self.key.clear();
        Emitter::init_start_tag(&mut self.inner);
    }
    fn init_end_tag(&mut self) {
        self.token_bytes = 0;
        self.key.clear();
        Emitter::init_end_tag(&mut self.inner);
    }
    fn init_attribute(&mut self) {
        self.finish_style();
        self.key.clear();
        self.style = Visibility::default();
        if self.stage(ATTRIBUTE_RESERVATION) {
            Emitter::init_attribute(&mut self.inner);
        }
    }
    fn push_attribute_name(&mut self, data: &[u8]) {
        if self.stage(TEXT_COPIES * data.len()) {
            Emitter::push_attribute_name(&mut self.inner, data);
            self.key
                .extend_from_slice(&data[..data.len().min(16 - self.key.len())]);
        }
    }
    fn push_attribute_value(&mut self, data: &[u8]) {
        if self.key == b"style" {
            self.style.push(data);
        } else if matches!(
            &self.key[..],
            b"alt" | b"aria-hidden" | b"start" | b"value" | b"type" | b"encoding"
        ) && self.stage(TEXT_COPIES * data.len())
        {
            Emitter::push_attribute_value(&mut self.inner, data);
        }
    }
    fn emit_current_tag(&mut self) -> Option<State> {
        self.finish_style();
        Emitter::emit_current_tag(&mut self.inner)
    }
    fn emit_string(&mut self, data: &[u8]) {
        if !self.context.raw.get() && !self.context.image.get() {
            self.budget.charge(TEXT_COPIES * data.len());
            if !self.budget.exceeded.get() {
                Emitter::emit_string(&mut self.inner, data);
            }
        }
    }
    fn push_comment(&mut self, _: &[u8]) {}
    fn push_processing_instruction_data(&mut self, _: &[u8]) {}
    fn push_processing_instruction_target(&mut self, _: &[u8]) {}
    fn init_doctype(&mut self) {
        self.token_bytes = 128;
        Emitter::init_doctype(&mut self.inner);
    }
    fn emit_current_doctype(&mut self) {
        self.budget.charge(self.token_bytes);
        Emitter::emit_current_doctype(&mut self.inner);
    }
    buffered_token_field!(push_tag_name);
    buffered_token_field!(push_doctype_name);
    buffered_token_field!(push_doctype_public_identifier);
    buffered_token_field!(push_doctype_system_identifier);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraphs(html: &[u8], budget: usize) -> Vec<String> {
        read_with_budget(io::Cursor::new(html), budget)
            .unwrap()
            .blocks
            .into_iter()
            .map(|b| b.text)
            .collect()
    }

    #[test]
    fn embedded_resources_are_discarded_while_reading() {
        let data = "a".repeat(120_000);
        let html = format!(
            r#"<STYLE>body{{background:url(data:image/png;base64,{data})}}</STYLE>
            <p>first <picture><source SRCSET='data:image/webp;base64,{data} 1x'>
            <IMG SRC=data:image/png;base64,{data} ALT="the cow &amp; its attorney"></picture> last</p>
            <svg><g><path d='{data}'/><image href='data:image/png;base64,{data}'/><text>{data}</text></g></svg>
            <p style='background:url(data:image/png;base64,{data}); visibility: HIDDEN !important'>secret</p>
            <figcaption style='background:url(data:image/png;base64,{data})'>caption</figcaption>"#
        );
        assert_eq!(
            paragraphs(html.as_bytes(), 32 * 1024),
            ["first the cow & its attorney last", "caption"]
        );
    }

    #[test]
    fn preserves_existing_html5_text_structure_and_formatting() {
        for s in [
            "<ol start=4><li>one <em>two</em><li value=97>three</ol>",
            "<table>stray<tr><td><b>cell<p>second</table>last",
            "<p>literal data:image/png;base64,AAAA &lt;img src='x'&gt;</p><p hidden>never</p>",
            "<!-- <img alt='not text'> --><script>let t = \"<img alt='bad'>\";</script><p>visible</p>",
            "<p>a <img alt='&amp; b' src='x'> c</p><p style='display:none'>s</p>",
            "<svg><foreignObject><p>hidden</p></foreignObject></svg><p>outside</p>",
            "<p>before</p><svg><path/><p>HTML exits foreign content</p>",
            r#"<svg><foreignObject><textarea>literal </svg> text</textarea><p>inner</p></foreignObject></svg><p>end</p>"#,
            r#"<svg><foreignObject><script>const x = "</svg>";</script><p>inner</p></foreignObject></svg><p>end</p>"#,
            r#"<svg><foreignObject><style>p::after{content:"</svg>"}</style>inside</foreignObject></svg>end"#,
            "<p><strong>í 😀</strong><br>final</p>",
        ] {
            let baseline = serde_json::to_value(markup::project(s,false).unwrap()).unwrap();
            let streamed = serde_json::to_value(read(io::Cursor::new(s)).unwrap().blocks).unwrap();
            assert_eq!(baseline, streamed, "{s}");
        }
    }

    #[test]
    fn honors_boms_declared_encodings_and_bad_final_sequences() {
        let mut latin = b"<meta charset=windows-1252><p>ma".to_vec();
        latin.extend_from_slice(b"\xf1ana</p>");
        assert_eq!(paragraphs(&latin, 32_768), ["mañana"]);
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for c in "<p>café 😀</p>".encode_utf16() {
                bytes.extend_from_slice(&if little {
                    c.to_le_bytes()
                } else {
                    c.to_be_bytes()
                });
            }
            assert_eq!(paragraphs(&bytes, 32_768), ["café 😀"]);
            bytes.pop();
            assert!(read(io::Cursor::new(bytes.as_slice()))
                .unwrap_err()
                .to_string()
                .contains("unsupported_encoding"));
        }
        assert!(read(io::Cursor::new(b"<p>x\0y</p>")).is_err());
    }

    #[test]
    fn unmarked_legacy_html_replays_prior_text_but_marked_utf8_stays_strict() {
        let mut bytes = b"<p>\xc3\xa9</p><!--".to_vec();
        bytes.resize(100_000, b' ');
        bytes.extend_from_slice(b"--><p>\x93Caf\xe9\x94</p>");
        assert_eq!(paragraphs(&bytes, 32_768), ["Ã©", "“Café”"]);
        for prefix in [b"<meta charset=utf-8>".as_slice(), b"\xef\xbb\xbf"] {
            let marked = [prefix, &bytes].concat();
            assert!(read(io::Cursor::new(marked)).is_err());
        }
        assert!(read(io::Cursor::new(b"<p>\xff\x81</p>")).is_err());
        assert_eq!(
            paragraphs(b"<meta charset=windows-1252><p>\x81\xea</p>", 32_768),
            ["\u{81}ê"]
        );
    }

    #[test]
    fn accepts_control_characters_in_html_code_examples() {
        let html = "<meta charset=utf-8><pre>first\u{2}\u{6}\u{7}last</pre>";
        assert_eq!(
            paragraphs(html.as_bytes(), 32_768),
            ["first\u{2}\u{6}\u{7}last"]
        );
    }

    #[test]
    fn charges_retained_comment_and_empty_element_nodes() {
        for token in ["<!-- -->", "<i></i>", "</br>"] {
            let html = token.repeat(512);
            assert!(read_with_budget(io::Cursor::new(html), 32 * 1024)
                .unwrap_err()
                .to_string()
                .contains("too_large"));
        }
    }

    #[test]
    fn stops_reading_a_text_only_file_as_soon_as_the_budget_is_exceeded() {
        let mut input = io::repeat(b'x').take(1_000_000_000_000);
        let error = read_as(
            &mut input,
            HtmlCharset::Selected(UTF_8),
            256 * 1024,
            &Cell::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("too_large"));
        let bytes_read = 1_000_000_000_000 - input.limit();
        assert!((1..160 * 1024).contains(&bytes_read), "{bytes_read}");
        assert!(read_with_budget(io::Cursor::new("<p>text</p>".repeat(400)), 32 * 1024).is_err());
    }
}
