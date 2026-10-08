//! OOXML fixtures: a 1,000-page DOCX and a 500-slide PPTX.
//!
//! Hand-written minimal-but-complete packages. Zip entries use a fixed
//! timestamp so the output is byte-identical between runs. Media is stored
//! uncompressed, as Office does for JPEG and PNG.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::images::{chart_png, par_generate, photo_jpeg};
use crate::rng::{Rng, words_to};

const XML_DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

struct Package {
    zip: ZipWriter<BufWriter<File>>,
}

impl Package {
    fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            zip: ZipWriter::new(BufWriter::with_capacity(8 << 20, File::create(path)?)),
        })
    }

    fn options(method: CompressionMethod) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(DateTime::default())
            .large_file(false)
    }

    fn xml(&mut self, name: &str, body: &str) -> io::Result<()> {
        self.zip
            .start_file(name, Self::options(CompressionMethod::Deflated))?;
        self.zip.write_all(XML_DECL.as_bytes())?;
        self.zip.write_all(body.as_bytes())
    }

    fn stored(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.zip
            .start_file(name, Self::options(CompressionMethod::Stored))?;
        self.zip.write_all(bytes)
    }

    fn finish(self) -> io::Result<()> {
        self.zip.finish()?.flush()
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sentence(rng: &mut Rng, len: usize) -> String {
    let mut s = String::new();
    words_to(rng, &mut s, len);
    let mut chars = s.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
    format!("{}{}.", first.unwrap_or_default(), chars.as_str())
}

fn core_props(title: &str) -> String {
    format!(
        r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{title}</dc:title><dc:creator>quire-fixtures</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:modified></cp:coreProperties>"#
    )
}

fn app_props() -> String {
    r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>quire-fixtures</Application></Properties>"#.to_owned()
}

fn root_rels() -> String {
    format!(
        r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="{{MAIN}}"/><Relationship Id="rId2" Type="{PKG_REL}/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#
    )
}

// ---- DOCX ---------------------------------------------------------------------

const DOCX_PAGES: u64 = 1000;
const DOCX_IMAGES: u64 = 20;
const EMU_PER_INCH: u64 = 914_400;

/// A paragraph; `page_break_before` starts it on a new page. (A manual
/// `<w:br w:type="page"/>` at the end of the previous page was tried first:
/// Word 16 dropped 11 of 999 such breaks when they fell at the foot of a
/// nearly full page, giving 989 pages.)
fn w_para(style: Option<&str>, text: &str, page_break_before: bool) -> String {
    let style = style.map_or(String::new(), |s| format!(r#"<w:pStyle w:val="{s}"/>"#));
    let brk = if page_break_before {
        "<w:pageBreakBefore/>"
    } else {
        ""
    };
    let ppr = if style.is_empty() && brk.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{style}{brk}</w:pPr>")
    };
    format!(
        r#"<w:p>{ppr}<w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        esc(text)
    )
}

fn w_table(rng: &mut Rng, rows: usize, cols: usize) -> String {
    let mut t = String::from(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid>"#,
    );
    for _ in 0..cols {
        t.push_str(r#"<w:gridCol w:w="2254"/>"#);
    }
    t.push_str("</w:tblGrid>");
    for r in 0..rows {
        t.push_str("<w:tr>");
        for c in 0..cols {
            let text = if r == 0 {
                format!("Column {}", c + 1)
            } else {
                let mut s = String::new();
                words_to(rng, &mut s, 14);
                s
            };
            let _ = write!(
                t,
                r#"<w:tc><w:tcPr><w:tcW w:w="1250" w:type="pct"/></w:tcPr>{}</w:tc>"#,
                w_para(None, &text, false)
            );
        }
        t.push_str("</w:tr>");
    }
    t.push_str("</w:tbl>");
    t
}

fn w_image(drawing_id: u64, image: u64) -> String {
    let (cx, cy) = (EMU_PER_INCH * 7 / 2, EMU_PER_INCH * 7 / 3);
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{drawing_id}" name="Picture {drawing_id}"/><a:graphic><a:graphicData uri="{NS_PIC}"><pic:pic><pic:nvPicPr><pic:cNvPr id="{drawing_id}" name="image{image}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{image}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

const DOCX_STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Malgun Gothic" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="120" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#;

/// Every page starts with "page break before" and its content is sized to
/// fit one A4 page, so Word paginates to exactly 1,000 pages.
pub fn docx(path: &Path, seed: u64) -> io::Result<serde_json::Value> {
    let mut rng = Rng::new(seed);
    let images = par_generate(DOCX_IMAGES, |i| chart_png(seed, i, 600, 400));
    let mut pkg = Package::create(path)?;

    pkg.xml(
        "[Content_Types].xml",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#,
    )?;
    pkg.xml(
        "_rels/.rels",
        &root_rels().replace("{MAIN}", "word/document.xml"),
    )?;
    pkg.xml("docProps/core.xml", &core_props("1,000-page DOCX fixture"))?;
    pkg.xml("docProps/app.xml", &app_props())?;
    pkg.xml("word/styles.xml", DOCX_STYLES)?;
    pkg.xml(
        "word/footer1.xml",
        &format!(
            r#"<w:ftr xmlns:w="{NS_W}"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p></w:ftr>"#
        ),
    )?;

    let mut rels = format!(
        r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rIdStyles" Type="{REL}/styles" Target="styles.xml"/><Relationship Id="rIdFooter" Type="{REL}/footer" Target="footer1.xml"/>"#
    );
    for (i, png) in images.iter().enumerate() {
        pkg.stored(&format!("word/media/image{i}.png"), png)?;
        let _ = write!(
            rels,
            r#"<Relationship Id="rIdImg{i}" Type="{REL}/image" Target="media/image{i}.png"/>"#
        );
    }
    rels.push_str("</Relationships>");
    pkg.xml("word/_rels/document.xml.rels", &rels)?;

    let mut body = format!(
        r#"<w:document xmlns:w="{NS_W}" xmlns:r="{NS_R}" xmlns:wp="{NS_WP}" xmlns:a="{NS_A}" xmlns:pic="{NS_PIC}"><w:body>"#
    );
    let (mut tables, mut pictures) = (0u64, 0u64);
    for page in 1..=DOCX_PAGES {
        // The first paragraph of every page after the first starts a new page.
        let mut new_page = page > 1;
        if page % 50 == 1 {
            body.push_str(&w_para(
                Some("Heading1"),
                &format!("Chapter {}", page / 50 + 1),
                new_page,
            ));
            new_page = false;
        }
        let title = sentence(&mut rng, 30);
        body.push_str(&w_para(
            Some("Heading2"),
            &format!("Page {page}: {title}"),
            new_page,
        ));
        for _ in 0..2 {
            let len = rng.range(300, 420) as usize;
            body.push_str(&w_para(None, &sentence(&mut rng, len), false));
        }
        match page % 3 {
            0 => {
                body.push_str(&w_table(&mut rng, 5, 4));
                tables += 1;
            }
            1 => {
                pictures += 1;
                body.push_str(&w_image(pictures, page % DOCX_IMAGES));
            }
            _ => {
                for _ in 0..3 {
                    body.push_str(&w_para(None, &sentence(&mut rng, 80), false));
                }
            }
        }
        let closing = sentence(&mut rng, 200);
        body.push_str(&w_para(None, &closing, false));
    }
    body.push_str(r#"<w:sectPr><w:footerReference w:type="default" r:id="rIdFooter"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#);
    pkg.xml("word/document.xml", &body)?;
    pkg.finish()?;
    Ok(serde_json::json!({
        "pages": DOCX_PAGES, "tables": tables, "inlineImages": pictures,
        "distinctImages": DOCX_IMAGES, "pageSize": "A4", "documentXmlBytes": body.len()
    }))
}

// ---- PPTX -----------------------------------------------------------------------

const PPTX_SLIDES: u64 = 500;

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Fixture"><a:themeElements><a:clrScheme name="Fixture"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Fixture"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Fixture"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

fn group_props() -> &'static str {
    r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#
}

fn title_shape(text: &str, with_xfrm: bool) -> String {
    let sp_pr = if with_xfrm {
        r#"<p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>"#
    } else {
        "<p:spPr/>"
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>{sp_pr}<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>"#,
        esc(text)
    )
}

/// 500 slides, each with its own ~1 MB JPEG, ~500 MB of media in total.
pub fn pptx(path: &Path, seed: u64) -> io::Result<serde_json::Value> {
    let mut rng = Rng::new(seed);
    let media = par_generate(PPTX_SLIDES, |i| photo_jpeg(seed, i, 1600, 900, 58, 85));
    let media_bytes: usize = media.iter().map(Vec::len).sum();
    let mut pkg = Package::create(path)?;

    let mut types = String::from(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="jpeg" ContentType="image/jpeg"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/presProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#,
    );
    for n in 1..=PPTX_SLIDES {
        let _ = write!(
            types,
            r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        );
    }
    types.push_str("</Types>");
    pkg.xml("[Content_Types].xml", &types)?;
    pkg.xml(
        "_rels/.rels",
        &root_rels().replace("{MAIN}", "ppt/presentation.xml"),
    )?;
    pkg.xml("docProps/core.xml", &core_props("500-slide PPTX fixture"))?;
    pkg.xml("docProps/app.xml", &app_props())?;

    let mut pres = format!(
        r#"<p:presentation xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>"#
    );
    let mut pres_rels = format!(
        r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="theme/theme1.xml"/><Relationship Id="rId3" Type="{REL}/presProps" Target="presProps.xml"/><Relationship Id="rId4" Type="{REL}/viewProps" Target="viewProps.xml"/><Relationship Id="rId5" Type="{REL}/tableStyles" Target="tableStyles.xml"/>"#
    );
    for n in 1..=PPTX_SLIDES {
        let _ = write!(pres, r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + n, 100 + n);
        let _ = write!(
            pres_rels,
            r#"<Relationship Id="rId{}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#,
            100 + n
        );
    }
    pres.push_str(r#"</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#);
    pres_rels.push_str("</Relationships>");
    pkg.xml("ppt/presentation.xml", &pres)?;
    pkg.xml("ppt/_rels/presentation.xml.rels", &pres_rels)?;
    pkg.xml(
        "ppt/presProps.xml",
        &format!(r#"<p:presentationPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"/>"#),
    )?;
    pkg.xml(
        "ppt/viewProps.xml",
        &format!(
            r#"<p:viewPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#
        ),
    )?;
    pkg.xml(
        "ppt/tableStyles.xml",
        &format!(
            r#"<a:tblStyleLst xmlns:a="{NS_A}" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
        ),
    )?;
    pkg.xml("ppt/theme/theme1.xml", THEME)?;

    pkg.xml(
        "ppt/slideMasters/slideMaster1.xml",
        &format!(
            r#"<p:sldMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{}{}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr algn="l"><a:defRPr sz="4000"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="228600" indent="-228600"><a:buFont typeface="Arial"/><a:buChar char="&#8226;"/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
            group_props(),
            title_shape("Title", true)
        ),
    )?;
    pkg.xml(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        &format!(
            r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#
        ),
    )?;
    pkg.xml(
        "ppt/slideLayouts/slideLayout1.xml",
        &format!(
            r#"<p:sldLayout xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" type="titleOnly" preserve="1"><p:cSld name="Title Only"><p:spTree>{}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
            group_props(),
            title_shape("Title", false)
        ),
    )?;
    pkg.xml(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        &format!(
            r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#
        ),
    )?;

    for (i, jpeg) in media.iter().enumerate() {
        let n = i as u64 + 1;
        pkg.stored(&format!("ppt/media/image{n}.jpeg"), jpeg)?;
        let mut bullets = String::new();
        for _ in 0..rng.range(3, 6) {
            let _ = write!(
                bullets,
                r#"<a:p><a:pPr marL="285750" indent="-285750"><a:buFont typeface="Arial"/><a:buChar char="&#8226;"/></a:pPr><a:r><a:rPr lang="en-US" sz="2000" dirty="0"/><a:t>{}</a:t></a:r></a:p>"#,
                esc(&sentence(&mut rng, 60))
            );
        }
        let title = format!("Slide {n}: {}", sentence(&mut rng, 24));
        pkg.xml(
            &format!("ppt/slides/slide{n}.xml"),
            &format!(
                r#"<p:sld xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:spTree>{}{}<p:sp><p:nvSpPr><p:cNvPr id="3" name="TextBox 2"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="5181600" cy="4351338"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square"><a:normAutofit/></a:bodyPr><a:lstStyle/>{bullets}</p:txBody></p:sp><p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture 3"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="6172200" y="1825625"/><a:ext cx="5181600" cy="2914650"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
                group_props(),
                title_shape(&title, false)
            ),
        )?;
        pkg.xml(
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            &format!(
                r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/image" Target="../media/image{n}.jpeg"/></Relationships>"#
            ),
        )?;
    }
    pkg.finish()?;
    Ok(serde_json::json!({
        "slides": PPTX_SLIDES, "mediaFiles": PPTX_SLIDES, "mediaBytes": media_bytes,
        "slideSize": "16:9 (12192000 x 6858000 EMU)"
    }))
}
