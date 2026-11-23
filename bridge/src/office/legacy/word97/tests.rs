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
fn bad_piece_ranges_incomplete_unicode_and_oversized_cp_counts_have_explicit_errors() {
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
        .starts_with("resource_limit:"));
    assert!(grpprl(&[0x00, 0x08, 0x81])
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
}
