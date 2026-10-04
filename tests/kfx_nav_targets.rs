//! KFX navigation targets that name a section's page_template (rather than
//! any storyline element) must still resolve.
//!
//! Kindle-produced books point the cover's nav entry at the cover section's
//! template container id; no IR node carries that id.

mod common;

use boko::Format;
use boko::import::{ChapterId, Importer, KfxImporter};
use boko::kfx::container::{
    extract_doc_symbols, get_symbol_text, parse_container_header, parse_container_info,
    parse_index_table, skip_enty_header,
};
use boko::kfx::ion::{IonParser, IonValue};
use boko::kfx::symbols::KfxSymbol;
use boko::model::AnchorTarget;

fn get_field(fields: &[(u64, IonValue)], sym: KfxSymbol) -> Option<&IonValue> {
    fields
        .iter()
        .find_map(|(k, v)| (*k == sym as u64).then_some(v))
}

/// Two text chapters around an image-only one, which exports as a
/// scale-fit page-template section.
fn build_test_book() -> Vec<u8> {
    use common::{Doc, EpubBuilder, Nav};
    EpubBuilder::new("Nav Targets")
        .image("images/plate.png", common::tiny_png())
        .doc(Doc::new("text/ch1.xhtml", "One", "<p>First chapter.</p>"))
        .doc(Doc::new(
            "text/plate.xhtml",
            "Plate",
            "<img src=\"../images/plate.png\" alt=\"plate\"/>",
        ))
        .doc(Doc::new("text/ch2.xhtml", "Two", "<p>Second chapter.</p>"))
        .nav(vec![
            Nav::new("One", "text/ch1.xhtml"),
            Nav::new("Two", "text/ch2.xhtml"),
        ])
        .build()
}

/// (section_name, page_template id) for every section in a KFX container.
fn section_templates(kfx: &[u8]) -> Vec<(String, i64)> {
    let header = parse_container_header(&kfx[..18]).expect("header");
    let info_end = header.container_info_offset + header.container_info_length;
    let info = parse_container_info(&kfx[header.container_info_offset..info_end]).expect("info");
    let (index_offset, index_length) = info.index.expect("index");
    let doc_symbols = info
        .doc_symbols
        .map(|(offset, length)| extract_doc_symbols(&kfx[offset..offset + length]))
        .unwrap_or_default();
    let entities = parse_index_table(
        &kfx[index_offset..index_offset + index_length],
        header.header_len,
    );

    let mut out = Vec::new();
    for e in entities
        .iter()
        .filter(|e| e.type_id == KfxSymbol::Section as u32)
    {
        let entity = &kfx[e.offset..e.offset + e.length];
        let Ok(IonValue::Struct(fields)) = IonParser::new(skip_enty_header(entity)).parse() else {
            continue;
        };
        let name = get_field(&fields, KfxSymbol::SectionName)
            .and_then(|v| get_symbol_text(v, &doc_symbols))
            .expect("section name")
            .to_string();
        if let Some(IonValue::List(templates)) = get_field(&fields, KfxSymbol::PageTemplates) {
            for t in templates {
                if let IonValue::Struct(tf) = t
                    && let Some(id) = get_field(tf, KfxSymbol::Id).and_then(|v| v.as_int())
                {
                    out.push((name.clone(), id));
                }
            }
        }
    }
    out
}

#[test]
fn kfx_section_template_id_resolves_to_its_chapter() {
    let epub = build_test_book();
    let mut book = boko::Book::from_bytes(&epub, Format::Epub).expect("import epub");
    let kfx = common::export_to_bytes(&mut book, Format::Kfx);

    let path =
        std::env::temp_dir().join(format!("boko_kfx_nav_targets_{}.kfx", std::process::id()));
    std::fs::write(&path, &kfx).expect("write kfx");
    let importer = KfxImporter::open(&path).expect("open kfx");
    let _ = std::fs::remove_file(&path);

    let templates = section_templates(&kfx);
    assert!(!templates.is_empty(), "no page templates exported");
    for (section, template_id) in templates {
        let chapter = (0..importer.spine().len())
            .map(|i| ChapterId(i as u32))
            .find(|&id| importer.source_id(id) == Some(section.as_str()))
            .unwrap_or_else(|| panic!("section {section} not in spine"));

        // Bare, `#`-prefixed and `#id:offset` forms all land on the chapter
        // (or, if some element does carry the id, inside it)
        for href in [
            template_id.to_string(),
            format!("#{template_id}"),
            format!("#{template_id}:3"),
        ] {
            match importer.resolve_href(ChapterId(0), &href) {
                Some(AnchorTarget::Chapter(id)) => assert_eq!(id, chapter, "{href}"),
                Some(AnchorTarget::Internal(gid)) => assert_eq!(gid.chapter, chapter, "{href}"),
                other => panic!("{href} (section {section}) resolved to {other:?}"),
            }
        }
    }
}
