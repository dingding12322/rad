use super::helpers::*;

#[test]
fn dev_file_icons_carry_glyph_and_parsed_color() {
    // Markdown: the Nerd Font markdown glyph with a hex color.
    let (md_glyph, md_color) = dev_file_icon("README.md", true).unwrap();
    assert!(md_color != gpui::hsla(0., 0., 0., 1.));
    // TypeScript maps to a different glyph than markdown.
    let (ts_glyph, _) = dev_file_icon("src/lib.ts", true).unwrap();
    assert_ne!(md_glyph, ts_glyph);
    // Unknown extensions still resolve to a fallback glyph.
    assert!(dev_file_icon("data.unknownext123", true).is_some());
    // Dark and light palettes can differ per glyph color.
    let (_, dark) = dev_file_icon("Cargo.toml", true).unwrap();
    let (_, light) = dev_file_icon("Cargo.toml", false).unwrap();
    let _ = (dark, light);
}
