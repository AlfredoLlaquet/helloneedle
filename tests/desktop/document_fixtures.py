"""Write original, deterministic search documents using only the Python standard library."""

from pathlib import Path
from shutil import copyfile
from zipfile import ZIP_DEFLATED, ZIP_STORED, ZipFile

ODF = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "draw": "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0",
    "presentation": "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0",
}
OOXML = "http://schemas.openxmlformats.org"
REL = OOXML + "/officeDocument/2006/relationships/"
PACKAGE = OOXML + "/package/2006/relationships"
DOC_NS = OOXML + "/wordprocessingml/2006/main"
SHEET_NS = OOXML + "/spreadsheetml/2006/main"
SLIDE_NS = OOXML + "/presentationml/2006/main"
DRAW_NS = OOXML + "/drawingml/2006/main"


def relationships(entries):
    records = "".join(
        f'<Relationship Id="{rid}" Type="{REL}{kind}" Target="{target}"/>'
        for rid, kind, target in entries
    )
    return f'<Relationships xmlns="{PACKAGE}">{records}</Relationships>'


def write_ooxml(path, main, parts, types):
    with ZipFile(path, "w", compression=ZIP_DEFLATED) as archive:
        overrides = "".join(
            f'<Override PartName="/{name}" ContentType="application/vnd.openxmlformats-officedocument.{kind}+xml"/>'
            for name, kind in types.items()
        )
        archive.writestr("[Content_Types].xml",
                         f'<Types xmlns="{OOXML}/package/2006/content-types">'
                         '<Default Extension="xml" ContentType="application/xml"/>'
                         '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                         f'{overrides}</Types>')
        archive.writestr("_rels/.rels", relationships([("rId1", "officeDocument", main)]))
        for name, content in parts.items():
            archive.writestr(name, content)


def write_odf(path, kind, body, flat=False):
    declarations = " ".join(f'xmlns:{name}="{uri}"' for name, uri in ODF.items())
    mime = "application/vnd.oasis.opendocument." + kind
    if flat:
        xml = (f'<office:document {declarations} office:version="1.2" office:mimetype="{mime}">'
               f'<office:body>{body}</office:body></office:document>')
        path.write_text(xml, encoding="utf-8")
        return
    with ZipFile(path, "w", compression=ZIP_DEFLATED) as archive:
        archive.writestr("mimetype", mime, compress_type=ZIP_STORED)
        archive.writestr("META-INF/manifest.xml",
                         '<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">'
                         f'<manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/>'
                         '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>'
                         '</manifest:manifest>')
        archive.writestr("content.xml", f'<office:document-content {declarations} office:version="1.2">'
                         f'<office:body>{body}</office:body></office:document-content>')


def write_pdf(path):
    lines = ["Cover page", "Needle on page two. Needle again.", "Closing page"]
    children = " ".join(f"{4 + 2*i} 0 R" for i in range(len(lines)))
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               f"<< /Type /Pages /Count {len(lines)} /Kids [{children}] >>".encode("ascii"),
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for i, line in enumerate(lines):
        objects.append((f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 320 240] "
                        f"/Resources << /Font << /F1 3 0 R >> >> /Contents {5 + i*2} 0 R >>").encode("ascii"))
        content = f"BT /F1 14 Tf 32 168 Td ({line}) Tj"
        if i == 1:
            content += " 0 -28 Td (This is the second line on this page.) Tj"
        stream = (content + " ET\n").encode("ascii")
        objects.append(f"<< /Length {len(stream)} >>\nstream\n".encode("ascii") + stream + b"endstream")
    data = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{i} 0 obj\n".encode("ascii") + obj + b"\nendobj\n")
    xref = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode("ascii"))
    for offset in offsets[1:]:
        data.extend(f"{offset:010d} 00000 n \n".encode("ascii"))
    data.extend(f"trailer\n<< /Root 1 0 R /Size {len(offsets)} >>\nstartxref\n{xref}\n%%EOF\n".encode("ascii"))
    path.write_bytes(data)


def write_documents(folder):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "notes.txt").write_text("First line\nSecond line\n🧵 Needle then Needle, café.\nLast line\n", encoding="utf-8")
    (folder / "page.html").write_text(
        '<!doctype html><html><head><title>Document fixture</title><script>forbiddenScriptMarker()</script></head>'
        '<body><h1>Newsletter</h1><p>The <strong>Latest</strong> Batch</p><p>Needle &amp; thread.</p></body></html>',
        encoding="utf-8")
    (folder / "readme.md").write_text("# Newsletter\n\nThe **Latest** Batch\n\nNeedle & thread.\n", encoding="utf-8")
    write_pdf(folder / "report.pdf")
    document = (f'<w:document xmlns:w="{DOC_NS}"><w:body>'
                '<w:p><w:r><w:t>Before the passage.</w:t></w:r></w:p>'
                '<w:p><w:r><w:t>Nee</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>dle in the margin.</w:t></w:r></w:p>'
                '<w:p><w:r><w:t>After the passage.</w:t></w:r></w:p><w:sectPr/></w:body></w:document>')
    write_ooxml(folder / "letter.docx", "word/document.xml", {"word/document.xml": document},
                {"word/document.xml": "wordprocessingml.document.main"})

    odtext = ('<office:text><text:p>Before the passage.</text:p>'
              '<text:p>Nee<text:span>dle</text:span> in the margin.</text:p>'
              '<text:p>After the passage.</text:p></office:text>')
    write_odf(folder / "letter.odt", "text", odtext)
    write_odf(folder / "letter.fodt", "text", odtext, flat=True)

    sheets = {"xl/workbook.xml": (f'<workbook xmlns="{SHEET_NS}" xmlns:r="{REL[:-1]}">'
                                  '<sheets><sheet name="Quarterly" sheetId="1" r:id="rId1"/></sheets></workbook>'),
              "xl/_rels/workbook.xml.rels": relationships([("rId1", "worksheet", "worksheets/sheet1.xml"),
                                                           ("rId2", "sharedStrings", "sharedStrings.xml")]),
              "xl/worksheets/sheet1.xml": (f'<worksheet xmlns="{SHEET_NS}"><sheetData><row r="7">'
                                           '<c r="C7" t="s"><v>0</v></c><c r="D7"><v>1942</v></c>'
                                           '</row></sheetData></worksheet>'),
              "xl/sharedStrings.xml": (f'<sst xmlns="{SHEET_NS}" count="1" uniqueCount="1"><si>'
                                       '<r><t>Nee</t></r><r><t>dle budget</t></r></si></sst>')}
    write_ooxml(folder / "sheet.xlsx", "xl/workbook.xml", sheets, {
        "xl/workbook.xml": "spreadsheetml.sheet.main", "xl/worksheets/sheet1.xml": "spreadsheetml.worksheet",
        "xl/sharedStrings.xml": "spreadsheetml.sharedStrings"})
    write_odf(folder / "sheet.ods", "spreadsheet",
              '<office:spreadsheet><table:table table:name="Quarterly">'
              '<table:table-row table:number-rows-repeated="6"><table:table-cell/></table:table-row>'
              '<table:table-row><table:table-cell table:number-columns-repeated="2"/>'
              '<table:table-cell office:value-type="string"><text:p>Needle budget</text:p></table:table-cell>'
              '<table:table-cell office:value-type="float" office:value="1942"><text:p>1942</text:p></table:table-cell>'
              '</table:table-row></table:table></office:spreadsheet>')

    slides = [("Cover", "Cover slide"), ("Details", "Needle on stage.")]
    presentation = (f'<p:presentation xmlns:p="{SLIDE_NS}" xmlns:r="{REL[:-1]}"><p:sldIdLst>'
                    + "".join(f'<p:sldId id="{255+i}" r:id="rId{i}"/>' for i in (1, 2))
                    + '</p:sldIdLst></p:presentation>')
    parts = {"ppt/presentation.xml": presentation,
             "ppt/_rels/presentation.xml.rels": relationships([(f"rId{i}", "slide", f"slides/slide{i}.xml") for i in (1, 2)])}
    types = {"ppt/presentation.xml": "presentationml.presentation.main"}
    for i, (name, text) in enumerate(slides, 1):
        member = f"ppt/slides/slide{i}.xml"
        parts[member] = (f'<p:sld xmlns:p="{SLIDE_NS}" xmlns:a="{DRAW_NS}"><p:cSld name="{name}">'
                         '<p:spTree><p:sp><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r>'
                         f'<a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>')
        types[member] = "presentationml.slide"
    write_ooxml(folder / "talk.pptx", "ppt/presentation.xml", parts, types)
    write_odf(folder / "talk.odp", "presentation", "<office:presentation>" + "".join(
        f'<draw:page draw:name="{name}"><draw:frame><draw:text-box><text:p>{text}</text:p>'
        '</draw:text-box></draw:frame></draw:page>' for name, text in slides) + "</office:presentation>")
    (folder / "letter.rtf").write_bytes(
        br"{\rtf1\ansi\ansicpg1252\uc1 Before the passage.\par Nee{\b dle} in the margin. Caf\'e9.\par After the passage.}")
    copyfile(Path(__file__).with_name("fixtures") / "sample.doc", folder / "sample.doc")
