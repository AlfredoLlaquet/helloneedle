// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn store16(buffer: &mut [u8], at: usize, v: u16) {
    buffer[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn store32(buffer: &mut [u8], at: usize, v: u32) {
    buffer[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

// Physical order differs from document order. Data outside the piece table is deliberate fast-save debris.
fn mixed_pieces() -> (Vec<u8>, Vec<u8>) {
    let mut word = vec![0; 4096];
    store16(&mut word, 32, 14);
    store16(&mut word, 62, 22);
    store16(&mut word, 152, 34);
    word[720..728].copy_from_slice(b"obsolete");
    let mut clx = vec![1, 3, 0, 0, 8, 1]; // CLX property group zero sets sprmCFRMarkDel.
    let pieces: [(usize, bool, u16, &[u8]); 6] = [
        (920, true, 0, b"First \x93"),
        (800, false, 0, &[0x3e, 0xd8]), // U+1F9F5 split across two pieces
        (1100, false, 0, &[0xf5, 0xdd]),
        (850, true, 1, b"DELETED"),
        (
            1000,
            true,
            0,
            b"\x94\r\x13HYPERLINK private\x14caf\xe9\x15\x0bnext\r",
        ),
        (740, true, 0, b"HEADER SECRET\r"),
    ];
    let mut cps = Vec::new();
    let mut pcds = Vec::new();
    let mut cp = 0u32;
    for (i, (offset, compressed, prm, data)) in pieces.iter().enumerate() {
        if i == pieces.len() - 1 {
            store32(&mut word, 76, cp);
        }
        word[*offset..*offset + data.len()].copy_from_slice(data);
        cps.extend(cp.to_le_bytes());
        cp += (data.len() / if *compressed { 1 } else { 2 }) as u32;
        pcds.extend(0u16.to_le_bytes());
        pcds.extend(
            (if *compressed {
                0x4000_0000 + 2 * (*offset as u32)
            } else {
                *offset as u32
            })
            .to_le_bytes(),
        );
        pcds.extend(prm.to_le_bytes());
    }
    cps.extend(cp.to_le_bytes());
    cps.extend(pcds);
    clx.push(2);
    clx.extend((cps.len() as u32).to_le_bytes());
    clx.extend(cps);
    store32(&mut word, 154 + 33 * 8 + 4, clx.len() as u32);
    (word, clx)
}

#[test]
fn reordered_unicode_and_compressed_pieces_fields_and_current_main_story() {
    let (word, table) = mixed_pieces();
    let mut out = Output::default();
    project(&word, &table, &mut out).unwrap();
    assert_eq!(
        out.blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>(),
        ["First “🧵”", "café\nnext"]
    );
}

#[test]
fn bad_piece_ranges_incomplete_unicode_and_impossible_cp_counts_have_explicit_errors() {
    let (word, table) = mixed_pieces();
    let mut bad = table.clone();
    let first_fc = 6 + 5 + 7 * 4 + 2;
    store32(&mut bad, first_fc, 0x3fff_fffe);
    assert!(project(&word, &bad, &mut Output::default())
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    let mut word_bad = word.clone();
    word_bad[1101] = b'X';
    assert!(project(&word_bad, &table, &mut Output::default())
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    store32(&mut word_bad, 76, u32::MAX);
    assert!(project(&word_bad, &table, &mut Output::default())
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    assert!(grpprl(&[0x00, 0x08, 0x82])
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
}

fn style_record(kind: u16, parent: u16, visibility: &[u8]) -> Vec<u8> {
    let mut std = vec![0; 10];
    store16(&mut std, 2, parent << 4 | kind);
    store16(&mut std, 4, if kind == 1 { 2 } else { 1 });
    std.extend([0; 4]); // empty Xstz name
    if kind == 1 {
        std.extend([2, 0, 0, 0]); // paragraph UPX with no direct paragraph properties
    }
    std.extend((visibility.len() as u16).to_le_bytes());
    std.extend(visibility);
    if visibility.len() % 2 != 0 {
        std.push(0);
    }
    let mut prefixed = (std.len() as u16).to_le_bytes().to_vec();
    prefixed.extend(std);
    prefixed
}

fn visibility_styles() -> Vec<u8> {
    let mut table = vec![0; 20];
    store16(&mut table, 0, 18);
    store16(&mut table, 2, 15);
    store16(&mut table, 4, 10);
    table.extend(style_record(1, 0xfff, &[])); // Normal
    table.extend(style_record(1, 0, &[0x3c, 8, 1])); // hidden paragraph style
    table.extend(style_record(1, 1, &[])); // inherited hidden paragraph style
    table.extend(style_record(2, 0xfff, &[0x3c, 8, 0x81])); // opposite of paragraph
    table.extend(style_record(2, 3, &[])); // inherits that same character style
    table.extend([0; 20]); // remaining fixed slots are unused
    table
}

fn append_table_part(word: &mut [u8], table: &mut Vec<u8>, index: usize, data: &[u8]) {
    store32(word, 154 + index * 8, table.len() as u32);
    store32(word, 158 + index * 8, data.len() as u32);
    table.extend(data);
}

#[test]
fn inherited_visibility_follows_the_logical_paragraph_and_the_active_character_style() {
    let mut word = vec![0; 4096];
    store16(&mut word, 32, 14);
    store16(&mut word, 62, 22);
    store16(&mut word, 152, 34);
    let mut table = Vec::new();
    append_table_part(&mut word, &mut table, 1, &visibility_styles());

    // The initial paragraph ends in an earlier physical piece. Both paragraphs use
    // inherited hidden formatting; character style 4 reveals the text of the second.
    let runs: [(usize, &[u8], u16); 5] = [
        (1200, b"omitted", 0),
        (1000, b" ending\r", 0),
        (1300, b"visible", 0),
        (1400, b"concealed", 1),
        (1450, b"\r", 0),
    ];
    let mut clx = vec![1, 3, 0, 0x3c, 8, 0x81]; // piece-only override on "concealed"
    let mut cps = Vec::new();
    let mut records = Vec::new();
    let mut cp = 0u32;
    for (start, data, prm) in runs {
        word[start..start + data.len()].copy_from_slice(data);
        cps.extend(cp.to_le_bytes());
        cp += data.len() as u32;
        records.extend([0, 0]);
        records.extend((0x4000_0000 + 2 * start as u32).to_le_bytes());
        records.extend(prm.to_le_bytes());
    }
    store32(&mut word, 76, cp);
    cps.extend(cp.to_le_bytes());
    cps.extend(records);
    clx.push(2);
    clx.extend((cps.len() as u32).to_le_bytes());
    clx.extend(cps);
    append_table_part(&mut word, &mut table, 33, &clx);

    let pap = &mut word[2048..2560];
    for (index, fc) in [900, 1008, 1451].into_iter().enumerate() {
        store32(pap, 4 * index, fc);
    }
    pap[12] = 240;
    pap[25] = 240;
    pap[480..484].copy_from_slice(&[0, 1, 2, 0]); // PAPX style 2
    pap[511] = 2;
    let chp = &mut word[2560..3072];
    store32(chp, 0, 1300);
    store32(chp, 4, 1451);
    chp[8] = 240;
    chp[480..488].copy_from_slice(&[7, 0x30, 0x4a, 4, 0, 0x3c, 8, 0x80]);
    chp[511] = 1;
    let bte =
        |a: u32, b: u32, pn: u32| [a.to_le_bytes(), b.to_le_bytes(), pn.to_le_bytes()].concat();
    append_table_part(&mut word, &mut table, 12, &bte(1300, 1451, 5));
    append_table_part(&mut word, &mut table, 13, &bte(900, 1451, 4));
    let mut out = Output::default();
    project(&word, &table, &mut out).unwrap();
    assert_eq!(
        out.blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>(),
        ["visible"]
    );
}

#[test]
fn character_style_changes_preserve_field_and_revision_flags() {
    let styles = Styles::read(&visibility_styles()).unwrap();
    let paragraph = styles.flags(2, 0).unwrap();
    let mut state = Visibility::from_style(paragraph);
    grpprl(&[2, 8, 1, 0, 8, 0x81, 0x30, 0x4a, 4, 0])
        .unwrap()
        .apply(&styles, paragraph, &mut state)
        .unwrap();
    assert!(state.hidden() && state.deleted());
    // Reset the field and revision flags relative to their baseline. The current
    // character style reverses hidden, so everything in the final run is visible.
    grpprl(&[2, 8, 0x80, 0, 8, 0, 0x3c, 8, 0x80])
        .unwrap()
        .apply(&styles, paragraph, &mut state)
        .unwrap();
    assert!(!state.hidden() && !state.deleted());
}

#[test]
fn circular_style_ancestry_is_reported() {
    let mut styles = visibility_styles();
    store16(&mut styles, 24, 1 << 4 | 1); // Normal based on style 1, which is based on Normal
    assert!(Styles::read(&styles)
        .err()
        .unwrap()
        .to_string()
        .contains("circular"));
}

#[test]
fn remapping_character_styles_reapplies_unstyled_direct_formatting() {
    let styles = Styles::read(&visibility_styles()).unwrap();
    let mut state = Visibility::from_style(0);
    // Default style 10 does not prescribe hidden. A directly hidden word stays
    // hidden when its style is remapped to the empty style at slot 11.
    grpprl(&[0x3c, 8, 1, 0x31, 0xca, 7, 0, 10, 0, 10, 0, 11, 0])
        .unwrap()
        .apply(&styles, 0, &mut state)
        .unwrap();
    assert!(state.hidden());
    piece_properties(0x53 << 1, &[])
        .unwrap()
        .apply(&styles, 0, &mut state)
        .unwrap();
    assert!(!state.hidden());
}

#[test]
fn random_access_reads_a_word_piece_past_64_mib_and_stops_mid_story() {
    // The text starts after a sparse, unreferenced portion of the stream.
    // Describing 40 MiB of story in the piece table must not cause a 40 MiB
    // allocation or read when the consumer already has its target.
    use std::fs::OpenOptions;
    use std::io::{Cursor, Write};
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hn-word-nr-{}-{stamp}", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let offset = 70 * 1024 * 1024usize;
    let chars = 40 * 1024 * 1024u32;
    let mut word = vec![0; 512];
    store16(&mut word, 32, 14);
    store16(&mut word, 62, 22);
    store16(&mut word, 152, 34);
    store32(&mut word, 76, chars);
    let mut clx = vec![2, 16, 0, 0, 0];
    clx.extend(0u32.to_le_bytes());
    clx.extend(chars.to_le_bytes());
    clx.extend([0, 0]);
    clx.extend((0x4000_0000 + 2 * (offset as u32)).to_le_bytes());
    clx.extend([0, 0]);
    store32(&mut word, 154 + 33 * 8 + 4, clx.len() as u32);
    file.write_all(&word).unwrap();
    file.seek(SeekFrom::Start(offset as u64)).unwrap();
    file.write_all(b"far target\r").unwrap();
    file.set_len(offset as u64 + u64::from(chars)).unwrap();
    let mut found = String::new();
    let mut visitor = |block: crate::Block| {
        found = block.text;
        Ok(false)
    };
    let mut out = Output {
        visitor: Some(&mut visitor),
        ..Default::default()
    };
    let result = project_streams(
        &mut Indexed::new(&mut file).unwrap(),
        &mut Indexed::new(&mut Cursor::new(clx)).unwrap(),
        &mut out,
    );
    drop(file);
    std::fs::remove_file(path).unwrap();
    assert!(result.unwrap_err().is::<crate::office::EndVisit>());
    assert_eq!(found, "far target");
}
