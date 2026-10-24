//! Document Word du compte rendu, mis en forme avec la charte graphique :
//! logo en en-tête, titres aux couleurs de la charte, police, pied de page et
//! numéro de page. Écrit à la main (quelques fichiers XML dans une archive
//! zip) : le Markdown du compte rendu n'utilise qu'un petit sous-ensemble
//! (titres, paragraphes, listes, tableaux, gras, italique).

use std::io::Write;

use zip::write::SimpleFileOptions;

use crate::meeting::GraphicCharter;

/// Logo prêt à insérer : PNG (l'interface convertit SVG, WebP… avant envoi).
pub struct DocxLogo {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct DocxInput<'a> {
    pub markdown: &'a str,
    pub title: &'a str,
    pub charter: &'a GraphicCharter,
    pub logo: Option<DocxLogo>,
}

/// Charte par défaut, quand aucune n'a été enregistrée.
pub fn default_charter() -> GraphicCharter {
    GraphicCharter {
        primary_color: "#6D3BF5".into(),
        secondary_color: "#1F2937".into(),
        font: "Arial".into(),
        footer: String::new(),
    }
}

/// Largeur utile d'une page A4 avec 2 cm de marge, en vingtièmes de point.
const TEXT_WIDTH_TWIPS: u32 = 9_638;
/// Hauteur du logo dans l'en-tête : 1 cm, en EMU.
const LOGO_HEIGHT_EMU: u64 = 360_000;
/// Largeur maximale du logo : 6 cm.
const LOGO_MAX_WIDTH_EMU: u64 = 2_160_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    Title(String),
    Heading(u8, String),
    Paragraph(String),
    Bullet(String),
    Numbered(String, String),
    Table(Vec<Vec<String>>),
}

fn table_cells(line: &str) -> Option<Vec<String>> {
    let inner = line.trim().strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    Some(inner.split('|').map(|c| c.trim().to_string()).collect())
}

fn is_table_separator(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

/// Découpe le Markdown en blocs. Chaque ligne de texte est un paragraphe :
/// dans un compte rendu, « Date : » et « Participants : » se suivent sans
/// ligne vide mais restent deux lignes.
fn parse_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    for line in markdown.lines() {
        if let Some(cells) = table_cells(line) {
            if !is_table_separator(&cells) {
                table.push(cells);
            }
            continue;
        }
        if !table.is_empty() {
            blocks.push(Block::Table(std::mem::take(&mut table)));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if level > 0 && trimmed[level..].starts_with(' ') {
            let text = trimmed[level..].trim().to_string();
            blocks.push(if level == 1 {
                Block::Title(text)
            } else {
                Block::Heading(level.min(3) as u8, text)
            });
            continue;
        }
        if let Some(rest) = ["- ", "* ", "• "]
            .iter()
            .find_map(|marker| trimmed.strip_prefix(marker))
        {
            blocks.push(Block::Bullet(rest.trim().to_string()));
            continue;
        }
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            let rest = &trimmed[digits..];
            if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
                blocks.push(Block::Numbered(
                    format!("{}.", &trimmed[..digits]),
                    text.trim().to_string(),
                ));
                continue;
            }
        }
        blocks.push(Block::Paragraph(trimmed.to_string()));
    }
    if !table.is_empty() {
        blocks.push(Block::Table(table));
    }
    blocks
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Caractères de contrôle interdits en XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Morceaux de texte avec leur mise en forme : `**gras**`, `__gras__`,
/// `*italique*`. Un marqueur sans fermeture reste du texte.
fn inline_runs(text: &str) -> Vec<(String, bool, bool)> {
    let mut runs: Vec<(String, bool, bool)> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut current = String::new();
    let mut rest = text;
    let flush = |current: &mut String, runs: &mut Vec<(String, bool, bool)>, b, i| {
        if !current.is_empty() {
            runs.push((std::mem::take(current), b, i));
        }
    };
    while !rest.is_empty() {
        let marker = ["**", "__"].into_iter().find(|m| rest.starts_with(m));
        if let Some(marker) = marker {
            let closes = bold || rest[2..].contains(marker);
            if closes {
                flush(&mut current, &mut runs, bold, italic);
                bold = !bold;
                rest = &rest[2..];
                continue;
            }
        } else if rest.starts_with('*') && (italic || rest[1..].contains('*')) {
            flush(&mut current, &mut runs, bold, italic);
            italic = !italic;
            rest = &rest[1..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        current.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut current, &mut runs, bold, italic);
    runs
}

fn runs_xml(text: &str, extra_props: &str) -> String {
    inline_runs(text)
        .into_iter()
        .map(|(text, bold, italic)| {
            let mut props = String::from(extra_props);
            if bold {
                props.push_str("<w:b/><w:bCs/>");
            }
            if italic {
                props.push_str("<w:i/><w:iCs/>");
            }
            let props = if props.is_empty() {
                String::new()
            } else {
                format!("<w:rPr>{props}</w:rPr>")
            };
            format!(
                "<w:r>{props}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                escape(&text)
            )
        })
        .collect()
}

fn paragraph(style: Option<&str>, content: &str) -> String {
    let props = style.map_or(String::new(), |s| {
        format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>")
    });
    format!("<w:p>{props}{content}</w:p>")
}

fn list_item(marker: &str, text: &str) -> String {
    paragraph(
        Some("Liste"),
        &format!(
            "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r><w:r><w:tab/></w:r>{}",
            escape(marker),
            runs_xml(text, "")
        ),
    )
}

fn color(value: &str) -> String {
    value.trim_start_matches('#').to_ascii_uppercase()
}

/// Largeur des colonnes, proportionnelle à leur plus long texte (bornée) :
/// « Quoi » prend la place, « Qui » et « Échéance » restent étroites.
fn column_widths(rows: &[Vec<String>], columns: usize) -> Vec<u32> {
    let weights: Vec<u32> = (0..columns)
        .map(|column| {
            let longest = rows
                .iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.replace(['*', '_'], "").chars().count())
                .max()
                .unwrap_or(0);
            longest.clamp(18, 55) as u32
        })
        .collect();
    let total: u32 = weights.iter().sum();
    weights
        .iter()
        .map(|weight| TEXT_WIDTH_TWIPS * weight / total)
        .collect()
}

fn table_xml(rows: &[Vec<String>], primary: &str) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let widths = column_widths(rows, columns);
    let border = |side: &str| {
        format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D1D5DB\"/>")
    };
    let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
        .iter()
        .map(|side| border(side))
        .collect();
    let mut xml = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"{TEXT_WIDTH_TWIPS}\" w:type=\"dxa\"/><w:tblBorders>{borders}</w:tblBorders><w:tblCellMar><w:top w:w=\"60\" w:type=\"dxa\"/><w:left w:w=\"100\" w:type=\"dxa\"/><w:bottom w:w=\"60\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr><w:tblGrid>"
    );
    for width in &widths {
        xml.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
    }
    xml.push_str("</w:tblGrid>");
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        // Une ligne n'est jamais coupée entre deux pages ; l'en-tête se
        // répète en haut de chaque page.
        xml.push_str("<w:tr><w:trPr><w:cantSplit/>");
        if header {
            xml.push_str("<w:tblHeader/>");
        }
        xml.push_str("</w:trPr>");
        for (column, width) in widths.iter().enumerate() {
            let text = row.get(column).map(String::as_str).unwrap_or("");
            let (shading, props) = if header {
                (
                    format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{primary}\"/>"),
                    "<w:b/><w:bCs/><w:color w:val=\"FFFFFF\"/>",
                )
            } else {
                (String::new(), "")
            };
            xml.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{shading}</w:tcPr><w:p><w:pPr><w:spacing w:after=\"0\"/></w:pPr>{}</w:p></w:tc>",
                runs_xml(text, props)
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl>");
    // Un tableau ne doit pas coller au paragraphe suivant.
    xml.push_str("<w:p/>");
    xml
}

const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn document_xml(blocks: &[Block], primary: &str) -> String {
    let mut body = String::new();
    for block in blocks {
        body.push_str(&match block {
            Block::Title(text) => paragraph(Some("Title"), &runs_xml(text, "")),
            Block::Heading(2, text) => paragraph(Some("Heading1"), &runs_xml(text, "")),
            Block::Heading(_, text) => paragraph(Some("Heading2"), &runs_xml(text, "")),
            Block::Paragraph(text) => paragraph(None, &runs_xml(text, "")),
            Block::Bullet(text) => list_item("•", text),
            Block::Numbered(marker, text) => list_item(marker, text),
            Block::Table(rows) => table_xml(rows, primary),
        });
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\"><w:body>{body}<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdHeader\"/><w:footerReference w:type=\"default\" r:id=\"rIdFooter\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1418\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\" w:header=\"567\" w:footer=\"567\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    )
}

fn styles_xml(charter: &GraphicCharter) -> String {
    let font = escape(&charter.font);
    let primary = color(&charter.primary_color);
    let secondary = color(&charter.secondary_color);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"{NS_W}\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\" w:eastAsia=\"{font}\"/><w:color w:val=\"1F2937\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/><w:lang w:val=\"fr-FR\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"12\" w:space=\"4\" w:color=\"{primary}\"/></w:pBdr><w:spacing w:after=\"240\"/></w:pPr><w:rPr><w:b/><w:bCs/><w:color w:val=\"{primary}\"/><w:sz w:val=\"40\"/><w:szCs w:val=\"40\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"320\" w:after=\"120\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:bCs/><w:color w:val=\"{primary}\"/><w:sz w:val=\"30\"/><w:szCs w:val=\"30\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"240\" w:after=\"80\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/><w:bCs/><w:color w:val=\"{secondary}\"/><w:sz w:val=\"26\"/><w:szCs w:val=\"26\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Liste\"><w:name w:val=\"Liste du compte rendu\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:tabs><w:tab w:val=\"left\" w:pos=\"567\"/></w:tabs><w:spacing w:after=\"60\"/><w:ind w:left=\"567\" w:hanging=\"283\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Header\"><w:name w:val=\"header\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"0\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Footer\"><w:name w:val=\"footer\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"0\"/></w:pPr><w:rPr><w:color w:val=\"6B7280\"/><w:sz w:val=\"16\"/><w:szCs w:val=\"16\"/></w:rPr></w:style>\
</w:styles>"
    )
}

/// Taille du logo dans l'en-tête (EMU) : 1 cm de haut, 6 cm de large au plus.
fn logo_extent(logo: &DocxLogo) -> (u64, u64) {
    let ratio = logo.width.max(1) as f64 / logo.height.max(1) as f64;
    let mut height = LOGO_HEIGHT_EMU;
    let mut width = (height as f64 * ratio).round() as u64;
    if width > LOGO_MAX_WIDTH_EMU {
        width = LOGO_MAX_WIDTH_EMU;
        height = (width as f64 / ratio).round() as u64;
    }
    (width.max(1), height.max(1))
}

fn header_xml(logo: Option<&DocxLogo>) -> String {
    let content = match logo {
        Some(logo) => {
            let (cx, cy) = logo_extent(logo);
            format!(
                "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"1\" name=\"Logo\"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"1\" name=\"logo.png\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"rIdLogo\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
            )
        }
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:hdr xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><w:p><w:pPr><w:pStyle w:val=\"Header\"/></w:pPr>{content}</w:p></w:hdr>"
    )
}

fn footer_xml(text: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:ftr xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\"><w:p><w:pPr><w:pStyle w:val=\"Footer\"/><w:tabs><w:tab w:val=\"right\" w:pos=\"{TEXT_WIDTH_TWIPS}\"/></w:tabs></w:pPr><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t xml:space=\"preserve\">Page </w:t></w:r><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p></w:ftr>",
        escape(text)
    )
}

fn content_types(with_logo: bool) -> String {
    let png = if with_logo {
        "<Default Extension=\"png\" ContentType=\"image/png\"/>"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>{png}<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/><Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/><Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/></Types>"
    )
}

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";

const DOCUMENT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rIdHeader\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/><Relationship Id=\"rIdFooter\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/></Relationships>";

const HEADER_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdLogo\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/logo.png\"/></Relationships>";

fn core_xml(title: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>{}</dc:title><dc:creator>OCADE Dictée</dc:creator><dc:language>fr-FR</dc:language></cp:coreProperties>",
        escape(title)
    )
}

/// Construit le fichier .docx.
pub fn build_docx(input: &DocxInput<'_>) -> Result<Vec<u8>, String> {
    let blocks = parse_blocks(input.markdown);
    let primary = color(&input.charter.primary_color);
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
            zip.start_file(name, options).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())
        };
        add(
            "[Content_Types].xml",
            content_types(input.logo.is_some()).as_bytes(),
        )?;
        add("_rels/.rels", ROOT_RELS.as_bytes())?;
        add("docProps/core.xml", core_xml(input.title).as_bytes())?;
        add("word/_rels/document.xml.rels", DOCUMENT_RELS.as_bytes())?;
        add(
            "word/document.xml",
            document_xml(&blocks, &primary).as_bytes(),
        )?;
        add("word/styles.xml", styles_xml(input.charter).as_bytes())?;
        add(
            "word/header1.xml",
            header_xml(input.logo.as_ref()).as_bytes(),
        )?;
        add(
            "word/footer1.xml",
            footer_xml(&input.charter.footer).as_bytes(),
        )?;
        if let Some(logo) = &input.logo {
            add("word/_rels/header1.xml.rels", HEADER_RELS.as_bytes())?;
            add("word/media/logo.png", &logo.png)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const REPORT: &str = "# Compte rendu – Site & boutique\n\n**Date :** jeudi 22 octobre 2026\n**Participants :** **Claire Moreau**, Thomas\n\n## Ordre du jour\n1. Maquettes\n2. Séance *photo*\n\n## Plan d'actions\n| Qui | Quoi | Échéance |\n|---|---|---|\n| **Claire** | Rappeler <le> photographe | lundi |\n\n## Décisions\n- Budget validé";

    fn read(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn markdown_becomes_titles_lists_and_tables() {
        let blocks = parse_blocks(REPORT);
        assert_eq!(
            blocks[0],
            Block::Title("Compte rendu – Site & boutique".into())
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph("**Date :** jeudi 22 octobre 2026".into())
        );
        assert!(matches!(&blocks[2], Block::Paragraph(p) if p.starts_with("**Participants")));
        assert_eq!(blocks[3], Block::Heading(2, "Ordre du jour".into()));
        assert_eq!(blocks[4], Block::Numbered("1.".into(), "Maquettes".into()));
        assert_eq!(
            blocks[7],
            Block::Table(vec![
                vec!["Qui".into(), "Quoi".into(), "Échéance".into()],
                vec![
                    "**Claire**".into(),
                    "Rappeler <le> photographe".into(),
                    "lundi".into()
                ],
            ])
        );
        assert_eq!(blocks.last(), Some(&Block::Bullet("Budget validé".into())));
    }

    #[test]
    fn inline_bold_and_italic_are_runs() {
        assert_eq!(
            inline_runs("**Date :** jeudi *22* 2*3"),
            vec![
                ("Date :".into(), true, false),
                (" jeudi ".into(), false, false),
                ("22".into(), false, true),
                (" 2*3".into(), false, false),
            ]
        );
    }

    #[test]
    fn the_document_carries_the_charter_and_escapes_text() {
        let charter = GraphicCharter {
            primary_color: "#fe414c".into(),
            secondary_color: "#385623".into(),
            font: "Poppins".into(),
            footer: "Maison & Cie — confidentiel".into(),
        };
        let bytes = build_docx(&DocxInput {
            markdown: REPORT,
            title: "Réunion",
            charter: &charter,
            logo: Some(DocxLogo {
                png: vec![1, 2, 3],
                width: 400,
                height: 100,
            }),
        })
        .unwrap();
        let document = read(&bytes, "word/document.xml");
        assert!(document.contains("Site &amp; boutique"));
        assert!(document.contains("Rappeler &lt;le&gt; photographe"));
        assert!(document.contains("w:fill=\"FE414C\""));
        assert!(document.contains("<w:pStyle w:val=\"Title\"/>"));
        let styles = read(&bytes, "word/styles.xml");
        assert!(styles.contains("w:ascii=\"Poppins\""));
        assert!(styles.contains("w:color w:val=\"385623\""));
        assert!(read(&bytes, "word/footer1.xml").contains("Maison &amp; Cie"));
        assert!(read(&bytes, "word/header1.xml").contains("r:embed=\"rIdLogo\""));
        assert!(read(&bytes, "[Content_Types].xml").contains("image/png"));
        // 4:1 → 1 cm de haut, 4 cm de large.
        assert!(read(&bytes, "word/header1.xml").contains("cx=\"1440000\" cy=\"360000\""));
    }

    #[test]
    fn without_logo_there_is_no_image_part() {
        let bytes = build_docx(&DocxInput {
            markdown: "# T",
            title: "T",
            charter: &default_charter(),
            logo: None,
        })
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert!(archive.by_name("word/media/logo.png").is_err());
        assert!(!read(&bytes, "word/header1.xml").contains("<w:drawing>"));
    }

    #[test]
    fn columns_follow_their_content_and_rows_never_split() {
        let rows = vec![
            vec!["Qui".into(), "Quoi".into(), "Échéance".into()],
            vec![
                "**Claire Moreau**".into(),
                "Rappeler le photographe pour confirmer la date de la séance".into(),
                "lundi 26 octobre".into(),
            ],
        ];
        let widths = column_widths(&rows, 3);
        assert!(widths[1] > widths[0] && widths[1] > widths[2], "{widths:?}");
        assert!(widths.iter().sum::<u32>() <= TEXT_WIDTH_TWIPS);
        let xml = table_xml(&rows, "FE414C");
        assert_eq!(xml.matches("<w:cantSplit/>").count(), 2);
        assert_eq!(xml.matches("<w:tblHeader/>").count(), 1);
    }

    #[test]
    fn wide_logos_are_capped() {
        let (cx, cy) = logo_extent(&DocxLogo {
            png: vec![],
            width: 2000,
            height: 100,
        });
        assert_eq!(cx, LOGO_MAX_WIDTH_EMU);
        assert_eq!(cy, 108_000);
    }
}
