// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use zip::{write::SimpleFileOptions, ZipWriter};

struct Fixture(PathBuf);
impl Fixture {
    fn new(extension: &str, bytes: &[u8]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "helloneedle-office-{}-{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn read(extension: &str, bytes: &[u8]) -> Result<Document> {
    let file = Fixture::new(extension, bytes);
    Ok(extract(&file.0, extension)?.unwrap())
}
fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, xml) in parts {
        zip.start_file(*path, SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
fn texts(doc: &Document) -> Vec<&str> {
    doc.blocks.iter().map(|b| b.text.as_str()).collect()
}
fn rels(items: &[(&str, &str, &str, bool)]) -> String {
    let mut s = String::from(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, path, external) in items {
        let mode = if *external {
            r#" TargetMode="External""#
        } else {
            ""
        };
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{path}"{mode}/>"#));
    }
    s.push_str("</Relationships>");
    s
}
fn odf(body: &str, styles: &str, flat: bool) -> String {
    let tag = if flat { "document" } else { "document-content" };
    format!(
        r#"<office:{tag}
 xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
 xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0">
 <office:automatic-styles>{styles}</office:automatic-styles><office:body>{body}</office:body></office:{tag}>"#
    )
}
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const PPT: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const DRAW: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const OFFICE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[test]
fn docx_joins_runs_and_retains_safe_rich_text_headings_and_table_coordinates() {
    let word = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
 <w:p><w:pPr><w:pStyle w:val="TitleTwo"/></w:pPr><w:r><w:t>Need</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>le &amp; &lt;needle&gt;</w:t></w:r></w:p>
 <w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>first</w:t></w:r></w:p></w:tc>
 <w:tc><w:p><w:del><w:r><w:delText>removed</w:delText></w:r></w:del><w:hyperlink><w:r><w:t>third</w:t><w:tab/><w:t>cell</w:t></w:r></w:hyperlink></w:p></w:tc></w:tr></w:tbl>
 </w:body></w:document>"#;
    let styles = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:styleId="TitleTwo"><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style></w:styles>"#;
    let root = rels(&[("a", "officeDocument", "dir/doc.xml", false)]);
    let doc_rel = rels(&[
        ("s", "styles", "styles.xml", false),
        ("ignored", "hyperlink", "https://example.invalid", true),
    ]);
    let doc = read(
        "docx",
        &zip(&[
            ("_rels/.rels", &root),
            ("dir/doc.xml", word),
            ("dir/styles.xml", styles),
            ("dir/_rels/doc.xml.rels", &doc_rel),
        ]),
    )
    .unwrap();
    assert_eq!(texts(&doc), ["Needle & <needle>", "first", "third\tcell"]);
    assert_eq!(doc.blocks[0].location["heading_level"], 2);
    assert_eq!(
        doc.blocks[2].location,
        json!({"index":2,"paragraph":3,"table":1,"row":1,"column":3})
    );
    let html = doc.blocks[0].html.as_deref().unwrap();
    assert!(html.starts_with("<h2") && html.contains("<b>le &amp; &lt;needle&gt;</b>"));
}

#[test]
fn odt_and_utf16_fodt_keep_headings_cell_and_whitespace_across_spans() {
    let body = r#"<office:text><text:h text:outline-level="3">Chapter</text:h>
    <text:p>Need<text:span text:style-name="emphasis">le</text:span><text:s text:c="2"/>here<text:line-break/>next</text:p>
    <table:table><table:table-header-rows><table:table-row><table:table-cell><text:p>In cell</text:p></table:table-cell></table:table-row></table:table-header-rows></table:table>
    </office:text>"#;
    let styles = r#"<style:style style:name="emphasis" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>"#;
    let src = odf(body, styles, false);
    let zipped = read("odt", &zip(&[("content.xml", &src)])).unwrap();
    let mut wide = vec![0xff, 0xfe];
    for unit in odf(body, styles, true).encode_utf16() {
        wide.extend(unit.to_le_bytes());
    }
    let flat = read("fodt", &wide).unwrap();
    for doc in [&zipped, &flat] {
        assert_eq!(texts(doc), ["Chapter", "Needle  here\nnext", "In cell"]);
        assert_eq!(doc.blocks[0].location["heading_level"], 3);
        assert!(doc.blocks[1].html.as_deref().unwrap().contains("<i>le</i>"));
        assert_eq!(
            doc.blocks[2].location,
            json!({"index":2,"paragraph":3,"table":1,"row":1,"column":1})
        );
    }
}

#[test]
fn xlsx_uses_relationship_and_sheet_order_and_only_cached_visible_values() {
    let book = format!(
        r#"<workbook xmlns="{SHEET}" xmlns:r="{OFFICE_REL}"><sheets><sheet name="Data" sheetId="6" r:id="last"/><sheet name="Archive" sheetId="2" r:id="first"/></sheets></workbook>"#
    );
    let rel = rels(&[
        ("s", "sharedStrings", "../common/strings.xml", false),
        ("last", "worksheet", "worksheets/data.xml", false),
        ("first", "worksheet", "worksheets/history.xml", false),
    ]);
    let strings = format!(
        r#"<sst xmlns="{SHEET}"><si><r><t>Need</t></r><r><t>le</t></r><rPh sb="0" eb="3"><t>phonetic reading</t></rPh></si></sst>"#
    );
    let data = format!(
        r#"<worksheet xmlns="{SHEET}"><sheetData><row r="3"><c r="B3" t="s"><v>0</v></c><c t="inlineStr"><is><r><t>inline</t></r><r><t> value</t></r></is></c><c t="b"><v>1</v></c><c><f>1+1</f><v>2</v></c><c><f>no-cache</f></c></row></sheetData></worksheet>"#
    );
    let history = format!(
        r#"<worksheet xmlns="{SHEET}"><sheetData><row><c t="d"><v>2026-05-01</v></c></row></sheetData></worksheet>"#
    );
    let bytes = zip(&[
        ("xl/workbook.xml", &book),
        ("xl/_rels/workbook.xml.rels", &rel),
        ("common/strings.xml", &strings),
        ("xl/worksheets/data.xml", &data),
        ("xl/worksheets/history.xml", &history),
    ]);
    let doc = read("xlsx", &bytes).unwrap();
    assert_eq!(
        texts(&doc),
        ["Needle", "inline value", "TRUE", "2", "2026-05-01"]
    );
    assert_eq!(
        doc.blocks[0].location,
        json!({"index":0,"sheet":"Data","cell":"B3","row":3,"column":2})
    );
    assert_eq!(doc.blocks[3].location["cell"], "E3");
    assert_eq!(doc.blocks[4].location["sheet"], "Archive");
}

#[test]
fn ods_keeps_sparse_coordinates_repeats_and_cached_values() {
    let content = odf(
        r#"<office:spreadsheet><table:table table:name="Budget">
    <table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="3"/></table:table-row>
    <table:table-row table:number-rows-repeated="2"><table:covered-table-cell/><table:table-cell office:value-type="float" office:value="42.5"><text:p>$42.50</text:p></table:table-cell><table:table-cell table:number-columns-repeated="2" office:value-type="string"><text:p>Need<text:span>le</text:span></text:p></table:table-cell></table:table-row>
    </table:table><table:table table:name="Dates"><table:table-row><table:table-cell office:value-type="date" office:date-value="2026-11-20"/></table:table-row></table:table></office:spreadsheet>"#,
        "",
        false,
    );
    let doc = read("ods", &zip(&[("content.xml", &content)])).unwrap();
    assert_eq!(
        texts(&doc),
        [
            "$42.50\n42.5",
            "Needle",
            "Needle",
            "$42.50\n42.5",
            "Needle",
            "Needle",
            "2026-11-20"
        ]
    );
    assert_eq!(doc.blocks[0].location["cell"], "B3");
    assert_eq!(doc.blocks[5].location["cell"], "D4");
    assert_eq!(doc.blocks[6].location["sheet"], "Dates");
}

#[test]
fn pptx_uses_presentation_order_and_odp_ignores_presenter_notes() {
    let book = format!(
        r#"<p:presentation xmlns:p="{PPT}" xmlns:r="{OFFICE_REL}"><p:sldIdLst><p:sldId id="257" r:id="second"/><p:sldId id="256" r:id="first"/></p:sldIdLst></p:presentation>"#
    );
    let rel = rels(&[
        ("first", "slide", "slides/slide1.xml", false),
        ("second", "slide", "slides/slide2.xml", false),
    ]);
    let slide = |s: &str| {
        format!(
            r#"<p:sld xmlns:p="{PPT}" xmlns:a="{DRAW}"><p:cSld><p:spTree><p:sp><p:txBody>{s}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
    };
    let first = slide("<a:p><a:r><a:t>End</a:t></a:r></a:p>");
    let second=slide("<a:p><a:r><a:t>Split</a:t></a:r><a:r><a:t> slide</a:t></a:r><a:br/><a:r><a:t>line</a:t></a:r></a:p><a:p><a:r><a:t>Next</a:t></a:r></a:p>");
    let ppt = read(
        "pptx",
        &zip(&[
            ("ppt/presentation.xml", &book),
            ("ppt/_rels/presentation.xml.rels", &rel),
            ("ppt/slides/slide1.xml", &first),
            ("ppt/slides/slide2.xml", &second),
        ]),
    )
    .unwrap();
    let content=odf("<office:presentation><draw:page><draw:frame><draw:text-box><text:p>Split<text:span> slide</text:span><text:line-break/>line</text:p><text:p>Next</text:p></draw:text-box></draw:frame><presentation:notes><text:p>Private</text:p></presentation:notes></draw:page><draw:page><draw:frame><text:p>End</text:p></draw:frame></draw:page></office:presentation>","",false);
    let odp = read("odp", &zip(&[("content.xml", &content)])).unwrap();
    for doc in [&ppt, &odp] {
        assert_eq!(texts(doc), ["Split slide\nline", "Next", "End"]);
        assert_eq!(
            doc.blocks[1].location,
            json!({"index":1,"slide":1,"paragraph":2})
        );
        assert_eq!(
            doc.blocks[2].location,
            json!({"index":2,"slide":2,"paragraph":1})
        );
    }
}

#[test]
fn rtf_combines_code_pages_unicode_fallback_and_surrogates_and_ignores_instruction_groups() {
    let data=br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fcharset0 Arial;}{\f1\fcharset204 Serif;}}
{\*\generator hidden}{\info{\title secret}}{\pict\bin3 {}x}Need{\b le} caf\'e9 \u241? \u-10179?\u-8704? \{yes\}\\
\par {\f1\'cf\'f0\'e8\'e2\'e5\'f2} {\field{\*\fldinst HYPERLINK secret}{\fldrslt visible}} {\upr{Wrong}{\*\ud Correct}} {\v hidden}{\i end}}";
    let doc = read("rtf", data).unwrap();
    assert_eq!(
        texts(&doc),
        ["Needle café ñ 😀 {yes}\\", "Привет visible Correct end"]
    );
    assert!(doc.blocks[0].html.as_deref().unwrap().contains("<b>le</b>"));
    assert_eq!(doc.blocks[1].location["paragraph"], 2);
    let double = read(
        "rtf",
        br"{\rtf1\ansi\ansicpg932 \'93\'fa\'96\'7b\uc2\u35486\'8c\'ea}",
    )
    .unwrap();
    assert_eq!(texts(&double), ["日本語"]);
    let oem = read("rtf", br"{\rtf1\pc \'82}").unwrap();
    assert_eq!(texts(&oem), ["é"]);
}

#[test]
fn recognized_errors_and_expansion_limits_are_distinct() {
    assert!(extract(Path::new("absent.unknown"), "unknown")
        .unwrap()
        .is_none());
    assert!(read("rtf", b"plain text")
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
    for text in [
        br"{\rtf1".as_slice(),
        br"{\rtf1\'zz}",
        br"{\rtf1\bin99 abc}",
    ] {
        assert!(read("rtf", text)
            .unwrap_err()
            .to_string()
            .starts_with("corrupt_document:"));
    }
    assert!(read("xlsx", b"not a zip")
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    let repeats = odf(
        r#"<office:spreadsheet><table:table table:name="bad"><table:table-row table:number-rows-repeated="1000000"><table:table-cell office:value-type="string" office:string-value="x"/></table:table-row></table:table></office:spreadsheet>"#,
        "",
        false,
    );
    assert!(read("ods", &zip(&[("content.xml", &repeats)]))
        .unwrap_err()
        .to_string()
        .starts_with("resource_limit:"));
    let manifest = r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry><manifest:encryption-data/></manifest:file-entry></manifest:manifest>"#;
    assert!(read("odt", &zip(&[("META-INF/manifest.xml", manifest)]))
        .unwrap_err()
        .to_string()
        .starts_with("encrypted_document:"));
    let book = format!(
        r#"<workbook xmlns="{SHEET}" xmlns:r="{OFFICE_REL}"><sheets><sheet name="External" r:id="s"/></sheets></workbook>"#
    );
    for target in ["https://example.invalid/other.xml", "../../outside.xml"] {
        let rel = rels(&[("s", "worksheet", target, target.starts_with("https:"))]);
        assert!(read(
            "xlsx",
            &zip(&[
                ("xl/workbook.xml", &book),
                ("xl/_rels/workbook.xml.rels", &rel)
            ])
        )
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
    }
    let bad = br#"<!DOCTYPE xml [<!ENTITY x SYSTEM "file:///unreachable">]><xml>&x;</xml>"#;
    assert!(read("fodt", bad)
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    assert!(read("doc", b"not a binary Word file")
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
}

#[test]
fn portable_word97_reads_regular_streams_and_unicode_native_paths() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/desktop/fixtures/sample.doc");
    let folder =
        std::env::temp_dir().join(format!("Hello Needle archivo 文字 {}", std::process::id()));
    fs::create_dir(&folder).unwrap();
    let spaced = folder.join("-escritura 日本語.doc");
    fs::copy(&path, &spaced).unwrap();
    let documents = [extract(&path, "doc"), extract(&spaced, "doc")];
    fs::remove_file(&spaced).unwrap();
    fs::remove_dir(&folder).unwrap();
    for result in documents {
        let doc = result.unwrap().unwrap();
        let source = include_str!("../../../tests/desktop/fixtures/sample.txt");
        assert_eq!(
            texts(&doc),
            source.lines().filter(|s| !s.is_empty()).collect::<Vec<_>>()
        );
        assert_eq!(doc.blocks[1].location["paragraph"], 2);
    }
}

#[test]
fn portable_word97_reads_real_mini_streams_and_skips_deleted_and_hidden_runs() {
    let short = read(
        "doc",
        include_bytes!("../../../tests/desktop/fixtures/short-word97.doc"),
    )
    .unwrap();
    let expected = include_str!("../../../tests/desktop/fixtures/short-word97.txt");
    assert_eq!(texts(&short), expected.lines().collect::<Vec<_>>());
    let changed = read(
        "doc",
        include_bytes!("../../../tests/desktop/fixtures/revisions-word97.doc"),
    )
    .unwrap();
    assert_eq!(texts(&changed), ["Visible revisión.", "Keep this ending."]);
}

#[test]
fn portable_word97_distinguishes_encryption_other_ole_files_and_broken_table_streams() {
    fn compound(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for (name, data) in entries {
            file.create_stream(*name).unwrap().write_all(data).unwrap();
        }
        file.into_inner().into_inner()
    }
    let mut header = [0u8; 32];
    header[..4].copy_from_slice(&[0xec, 0xa5, 0xc1, 0]);
    header[11] = 1;
    let encrypted = compound(&[("WordDocument", &header)]);
    assert!(read("doc", &encrypted)
        .unwrap_err()
        .to_string()
        .starts_with("encrypted_document:"));
    header[11] = 0;
    let broken = compound(&[("WordDocument", &header)]);
    assert!(read("doc", &broken)
        .unwrap_err()
        .to_string()
        .starts_with("corrupt_document:"));
    header[2..4].copy_from_slice(&0x1234u16.to_le_bytes());
    let unknown = compound(&[("WordDocument", &header)]);
    assert!(read("doc", &unknown)
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
    let excel = compound(&[("Workbook", &[])]);
    assert!(read("doc", &excel)
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
    let package = compound(&[("EncryptionInfo", &[]), ("EncryptedPackage", &[])]);
    assert!(read("doc", &package)
        .unwrap_err()
        .to_string()
        .starts_with("encrypted_document:"));
}

#[test]
fn bounded_zip64_and_encrypted_compound_containers_have_distinct_results() {
    let src = odf(
        "<office:text><text:p>ZIP64 paragraph</text:p></office:text>",
        "",
        false,
    );
    let mut data = zip(&[("content.xml", &src)]);
    let footer = data.split_off(data.len() - 22);
    let offset = data.len() as u64;
    let count = u16::from_le_bytes(footer[10..12].try_into().unwrap()) as u64;
    let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
    let directory = u32::from_le_bytes(footer[16..20].try_into().unwrap()) as u64;
    data.extend(b"PK\x06\x06");
    data.extend(44u64.to_le_bytes());
    data.extend([45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for field in [count, count, size, directory] {
        data.extend(field.to_le_bytes());
    }
    data.extend(b"PK\x06\x07");
    data.extend(0u32.to_le_bytes());
    data.extend(offset.to_le_bytes());
    data.extend(1u32.to_le_bytes());
    data.extend(&footer[..8]);
    data.extend([0xff; 12]);
    data.extend(&footer[20..]);
    assert_eq!(texts(&read("odt", &data).unwrap()), ["ZIP64 paragraph"]);

    let mut ole = vec![0; 1024];
    ole[..8].copy_from_slice(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1");
    assert!(read("docx", &ole)
        .unwrap_err()
        .to_string()
        .starts_with("unsupported_document:"));
    for (block, word) in [(4, "EncryptionInfo"), (5, "EncryptedPackage")] {
        let entry = &mut ole[block * 128..block * 128 + 128];
        for (i, ch) in word.encode_utf16().enumerate() {
            entry[i * 2..i * 2 + 2].copy_from_slice(&ch.to_le_bytes());
        }
        entry[64] = ((word.len() + 1) * 2) as u8;
        entry[66] = 2;
    }
    assert!(read("docx", &ole)
        .unwrap_err()
        .to_string()
        .starts_with("encrypted_document:"));
}
