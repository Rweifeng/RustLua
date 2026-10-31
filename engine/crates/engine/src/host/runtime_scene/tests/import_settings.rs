//! 导入设置在**运行期**的两个接缝：字体字重覆盖与逐音效音量。
//!
//! 这两条都落在同一个文件上（工程根的 `resources.meta`），但生效时机不同：
//! 字重在字体图集**构建时**读一次，音量在**每次播放**时乘进去。

use super::*;
use rustlua_assets::import_settings::{
    AudioImportSettings, FontImportSettings, RESOURCE_META_FILE, ResourceMeta,
};
use std::fs;
use std::path::Path;

/// 不认的表头（版本更高的那份文件）：读它必须失败，而不是当成空表。
const FOREIGN_HEADER: &str = "# rustlua-resource-meta-v9";

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-import-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// 导入设置里的**字重覆盖**优先于工程级 `font_weight`。
///
/// 观测点是**图集里的字形度量**：可变字体的字宽随字重变，所以"按 100 建的图集"与
/// "按 700 建的图集"量出来必然不同。断言覆盖后的图集落在 100 那一档，
/// 而不是工程级写的 700。
#[test]
fn the_import_settings_font_weight_overrides_the_project_weight() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = temp_root("font-weight");
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::copy(&source, fonts_dir.join("Main.ttf")).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/Main.ttf\"\nfont_weight = 700\n",
    )
    .unwrap();

    let mut meta = ResourceMeta::open(&root).expect("meta");
    meta.set_font_settings("assets/fonts/Main.ttf", FontImportSettings::new(Some(100)))
        .expect("font");
    meta.save().expect("save");

    // 决策函数：覆盖 = 100，没登记别的字体 = 工程级 700。
    assert!(
        (super::fonts::font_weight_for(&root, "assets/fonts/Main.ttf") - 100.0).abs() < 1e-6,
        "登记了的字重必须压过工程级"
    );
    assert!(
        (super::fonts::font_weight_for(&root, "assets/fonts/Other.ttf") - 700.0).abs() < 1e-6,
        "没登记的字体必须沿用工程级"
    );

    // 端到端：真正建出来的图集按覆盖那一档。
    let atlas = load_project_font_atlas(Some(&root)).expect("project font atlas");
    let mut atlas = atlas.lock().unwrap();
    let advance = atlas.try_glyph_metrics('I').expect("glyph metrics").advance;
    let bytes = fs::read(&source).unwrap();
    let mut thin = FontAtlas::load_with_fallback_and_weight(&bytes, None, 100.0).unwrap();
    let mut bold = FontAtlas::load_with_fallback_and_weight(&bytes, None, 700.0).unwrap();
    let thin_advance = thin.try_glyph_metrics('I').unwrap().advance;
    let bold_advance = bold.try_glyph_metrics('I').unwrap().advance;
    let _ = fs::remove_dir_all(&root);

    assert!(
        (advance - thin_advance).abs() < 0.001,
        "覆盖字重没生效：advance={advance} thin={thin_advance} bold={bold_advance}"
    );
    assert!(
        (advance - bold_advance).abs() > 0.001,
        "覆盖字重被工程级压过去了：advance={advance} bold={bold_advance}"
    );
}

/// 字重覆盖没登记时，字重就是工程级那一个（不是"默认 400"）。
#[test]
fn a_font_without_an_override_keeps_the_project_weight() {
    let root = temp_root("font-no-override");
    fs::write(root.join("project.toml"), "[project]\nfont_weight = 650\n").unwrap();
    assert!(
        (super::fonts::font_weight_for(&root, "assets/fonts/Main.ttf") - 650.0).abs() < 1e-6,
        "没登记覆盖就必须沿用工程级字重"
    );
    let _ = fs::remove_dir_all(&root);
}

/// `resources.meta` 读不动时沿用工程级字重——读不动 ≠ 字重 0。
#[test]
fn an_unreadable_meta_file_keeps_the_project_weight() {
    let root = temp_root("font-broken");
    fs::write(root.join("project.toml"), "[project]\nfont_weight = 650\n").unwrap();
    fs::write(
        root.join(RESOURCE_META_FILE),
        format!("{FOREIGN_HEADER}\nfont\ta.ttf\t700\n"),
    )
    .unwrap();
    assert!(
        (super::fonts::font_weight_for(&root, "assets/fonts/Main.ttf") - 650.0).abs() < 1e-6,
        "读不动那份文件时必须沿用工程级字重"
    );
    let _ = fs::remove_dir_all(&root);
}

/// 逐音效的音量表从导入设置建出来，**键是音效名**（与 `play("hit")` 同口径）。
#[test]
fn the_audio_gain_table_is_built_from_the_import_settings() {
    let root = temp_root("audio-gains");
    let mut meta = ResourceMeta::open(&root).expect("meta");
    meta.set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::new(0.25))
        .expect("hit");
    meta.set_audio_settings("assets/audio/bgm.wav", AudioImportSettings::new(2.0))
        .expect("bgm");
    meta.save().expect("save");

    let gains = super::scene::audio_asset_gains(&root);
    let _ = fs::remove_dir_all(&root);
    assert_eq!(gains.get("hit"), Some(&0.25), "键必须是音效名：{gains:?}");
    assert_eq!(gains.get("bgm"), Some(&2.0), "键必须是音效名：{gains:?}");
    assert!(
        !gains.contains_key("assets/audio/hit.ogg"),
        "键不能是路径（`play(\"hit\")` 按名字找文件）：{gains:?}"
    );
}

/// 那份文件读不动时：整张表退化成"全部原样"，而不是把整场音频拖垮。
#[test]
fn an_unreadable_meta_file_leaves_the_audio_gains_untouched() {
    let root = temp_root("audio-broken");
    fs::write(
        root.join(RESOURCE_META_FILE),
        format!("{FOREIGN_HEADER}\naudio\ta.ogg\t0.5\n"),
    )
    .unwrap();
    let gains = super::scene::audio_asset_gains(&root);
    let _ = fs::remove_dir_all(&root);
    assert!(gains.is_empty(), "读不动就不该有音量表：{gains:?}");
}
