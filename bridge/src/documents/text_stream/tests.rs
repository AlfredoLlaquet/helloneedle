use super::*;
use grep_regex::RegexMatcherBuilder;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Sample(PathBuf);

impl Sample {
    fn write(extension: &str, content: &[u8]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "helloneedle-text-stream-{}-{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        fs::write(&path, content).unwrap();
        Self(path)
    }

    fn results(&self, query: &str, regex: bool) -> Vec<(Value, usize, usize)> {
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(true)
            .fixed_strings(!regex)
            .build(query)
            .unwrap();
        let mut result = Vec::new();
        let supported = search(
            &self.0,
            "text",
            &matcher,
            query,
            regex,
            |loc, _, _, _, offset, length| {
                result.push((loc.clone(), offset, length));
                Ok(true)
            },
        )
        .unwrap();
        assert!(supported);
        result
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn large_literal_crossing_reads_and_clipped_preview() {
    let mut line = "a".repeat(BUFFER - 3);
    line.push_str("NeeDle");
    line.push_str(&"b".repeat(3 * BUFFER - 7));
    line.push_str("😀Needle");
    line.push_str(&"c".repeat(BUFFER));
    let file = Sample::write("json", line.as_bytes());
    let result = file.results("needle", false);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].1, BUFFER - 3);
    for (loc, focus, len) in result {
        let (preview, index, total) = preview(&file.0, "text", &loc, focus).unwrap();
        assert_eq!(total, None);
        let selected = &preview[index];
        assert!(selected.text.len() < 65_536);
        let start = focus - selected.location["textOffset"].as_u64().unwrap_or(0) as usize;
        let utf16: Vec<_> = selected.text.encode_utf16().skip(start).take(len).collect();
        assert_eq!(String::from_utf16(&utf16).unwrap().to_lowercase(), "needle");
    }
}

#[test]
fn legacy_tail_selects_one_encoding_and_preview_decodes_the_same_bytes() {
    // The ambiguous pair is valid UTF-8, but the final standalone é proves this file needs 1252.
    let mut bytes = "x".repeat(BUFFER + 7).into_bytes();
    bytes.extend_from_slice(b"\r\n\xc3\xa9\n");
    bytes.extend_from_slice(&vec![b'y'; BUFFER + 99]);
    bytes.extend_from_slice(b"\rCaf\xe9");
    let file = Sample::write("txt", &bytes);
    assert_eq!(file.results("Ã©", false).len(), 1);
    let hits = file.results("CAFÉ", false);
    assert_eq!(hits.len(), 1);
    let (loc, focus, len) = &hits[0];
    assert_eq!((*focus, *len), (0, 4));
    assert_eq!(loc["line"], 4);
    let (blocks, selected, total) = preview(&file.0, "text", loc, *focus).unwrap();
    assert_eq!(blocks[selected].text, "Café");
    assert_eq!(total, Some(4));
}

#[test]
fn utf16_surrogates_crlf_and_codepoints_cross_buffer_edges() {
    for little in [true, false] {
        let mut text = "a".repeat(BUFFER / 2 - 1);
        text.push_str("😀KEK");
        text.push_str(&"a".repeat(BUFFER / 2 - 6));
        text.push_str("\r\n\tNeedle\r\n");
        let mut bytes = if little {
            vec![255, 254]
        } else {
            vec![254, 255]
        };
        for c in text.encode_utf16() {
            let pair = if little {
                c.to_le_bytes()
            } else {
                c.to_be_bytes()
            };
            bytes.extend_from_slice(&pair);
        }
        let file = Sample::write("txt", &bytes);
        let hits = file.results("kek", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, BUFFER / 2 + 1);
        let hits = file.results("^\\sneedle$", true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0["line"], 2);
        let (blocks, selected, _) = preview(&file.0, "text", &hits[0].0, hits[0].1).unwrap();
        assert_eq!(blocks[selected].text, "\tNeedle");
    }
}

#[test]
fn regex_anchors_and_empty_lines_use_physical_lines() {
    let mut text = "x".repeat(BUFFER - 1);
    text.push_str("\r\n\rN");
    text.push_str(&"x".repeat(BUFFER + 15));
    text.push_str("Z\n");
    let file = Sample::write("txt", text.as_bytes());
    assert_eq!(file.results("^$", true)[0].0["line"], 2);
    let hits = file.results("^N.*Z$", true);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0["line"], 3);
    assert_eq!(hits[0].2, BUFFER + 17);
}

#[test]
fn late_literal_match_after_old_line_count_keeps_direct_bookmark_and_context() {
    let mut data = "generic text\n".repeat(200_005);
    data.push_str("selected line\nnext line");
    let file = Sample::write("log", data.as_bytes());
    let hits = file.results("selected", false);
    assert_eq!(hits[0].0["line"], 200_006);
    assert!(hits[0].0["sourceByte"].as_u64().unwrap() > 2_000_000);
    let (blocks, selected, total) = preview(&file.0, "text", &hits[0].0, 0).unwrap();
    assert_eq!(selected, 12);
    assert_eq!(blocks[selected + 1].text, "next line");
    assert_eq!(total, Some(200_007));
}

#[test]
fn html_source_preserves_controls_and_honors_declared_encoding() {
    let file = Sample::write(
        "html",
        b"<meta charset='windows-1252'>\n<p>Caf\xe9\x02\x06\x07\x81.</p>",
    );
    let matcher = RegexMatcherBuilder::new().build("Café").unwrap();
    let mut found = None;
    search(
        &file.0,
        "html",
        &matcher,
        "Café",
        false,
        |loc, _, _, _, _, _| {
            found = Some(loc.clone());
            Ok(true)
        },
    )
    .unwrap();
    let (blocks, index, _) = preview(&file.0, "html", &found.unwrap(), 3).unwrap();
    assert_eq!(blocks[index].text, "<p>Café\x02\x06\x07\u{81}.</p>");
}

#[test]
fn trimming_literal_windows_preserves_nonoverlapping_unicode_matches() {
    let query = "kakak";
    let mut text = "_".repeat(2 * BUFFER - 1600);
    text.push_str(&"Ka".repeat(1500));
    text.push_str(&"_".repeat(BUFFER));
    let file = Sample::write("json", text.as_bytes());
    let got: Vec<_> = file
        .results(query, false)
        .into_iter()
        .map(|(_, start, length)| (start, length))
        .collect();
    let expected: Vec<_> = (0..500).map(|n| (2 * BUFFER - 1600 + 6 * n, 5)).collect();
    assert_eq!(got, expected);
}

#[test]
fn unknown_binary_and_invalid_declared_text_are_distinguished() {
    let matcher = RegexMatcherBuilder::new().build("anything").unwrap();
    let check = |file: &Sample| {
        search(
            &file.0,
            "text",
            &matcher,
            "anything",
            false,
            |_, _, _, _, _, _| Ok(true),
        )
    };
    let binary = Sample::write("odd", b"\xff\x81\n\x00");
    assert!(!check(&binary).unwrap());
    let truncated = Sample::write("txt", &[0xff, 0xfe, 65]);
    assert!(check(&truncated)
        .unwrap_err()
        .to_string()
        .contains("unsupported_encoding"));
}

#[test]
fn word_export_hyphens_cross_buffers_and_preserve_highlight_positions() {
    for utf16 in [false, true] {
        let units_per_buffer = if utf16 { BUFFER / 2 } else { BUFFER };
        for shift in [0, 1] {
            let prefix = "x".repeat(units_per_buffer - 5 + shift);
            let full = format!("{prefix}part\x1e2 😀\r\nlast");
            let bytes = if utf16 {
                [
                    vec![0xff, 0xfe],
                    full.encode_utf16().flat_map(u16::to_le_bytes).collect(),
                ]
                .concat()
            } else {
                [vec![0xef, 0xbb, 0xbf], full.into_bytes()].concat()
            };
            let sample = Sample::write("TXT", &bytes);
            let hits = sample.results("part-2", false);
            assert_eq!(hits.len(), 1);
            let (location, focus, len) = &hits[0];
            assert_eq!((*focus, *len), (prefix.len(), 6));
            let (blocks, i, _) = preview(&sample.0, "text", location, *focus).unwrap();
            let offset = blocks[i].location["textOffset"].as_u64().unwrap_or(0) as usize;
            let selection: Vec<_> = blocks[i]
                .text
                .encode_utf16()
                .skip(focus - offset)
                .take(*len)
                .collect();
            assert_eq!(String::from_utf16(&selection).unwrap(), "part-2");
        }
    }
}

#[test]
fn word_hyphen_preserves_combining_marks_split_between_read_buffers() {
    for shift in [0, 1] {
        let prefix = "z".repeat(BUFFER - 6 + shift);
        let file = Sample::write("txt", format!("{prefix}cafe\u{301}\x1ecrème").as_bytes());
        let query = "cafe\u{301}-crème";
        let hits = file.results(query, false);
        assert_eq!(hits.len(), 1);
        let (loc, start, len) = &hits[0];
        assert_eq!((*start, *len), (prefix.len(), query.encode_utf16().count()));
        let (blocks, i, _) = preview(&file.0, "text", loc, *start).unwrap();
        assert!(blocks[i].text.ends_with(query));
    }
}

#[test]
fn ascii_del_prevents_late_legacy_fallback() {
    let mut bytes = vec![b'x'; BUFFER];
    bytes[1] = 0x7f;
    bytes.extend_from_slice(b"Caf\xe9");
    for (extension, html) in [("md", false), ("htm", true), ("txt", false)] {
        let file = Sample::write(extension, &bytes);
        let result = source::walk(&file.0, html, None, |_, _, _, _| Ok(true));
        assert!(result.unwrap_err().is::<source::NotText>());
    }
}

#[test]
fn record_separators_outside_word_joins_still_fail_validation() {
    let matcher = RegexMatcherBuilder::new().build("word").unwrap();
    for content in [
        b"\x1eword".as_slice(),
        b"word\x1e",
        b"word\x1e\x1eother",
        b"word\x1e ",
        b"word\x1e2\x00",
    ] {
        let sample = Sample::write("txt", content);
        assert!(search(
            &sample.0,
            "text",
            &matcher,
            "word",
            false,
            |_, _, _, _, _, _| Ok(true)
        )
        .is_err());
    }
    let sample = Sample::write("unknown", b"word\x1e2");
    assert!(!search(
        &sample.0,
        "text",
        &matcher,
        "word",
        false,
        |_, _, _, _, _, _| Ok(true)
    )
    .unwrap());
}
