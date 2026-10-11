//! 字体管道的原生回归：**工程根只在宿主资源表里**（Web/微信的形态）时，
//! `project.toml` 的字体配置与字体字节都要能读到，探针要能按真字形量出宽度。
//!
//! 这是上一轮"字体进 wasm"那条修复的原生复现：当时只有浏览器侧证据，因为 `vfs::set_provider`
//! 是进程级一次安装的，单元测试里装它会波及其他用例。Cargo 给每个集成测试文件单独一个进程，
//! 这里因此可以安全地装 provider——本文件也只放一个 `#[test]`。

use std::path::Path;

/// 仓库里的真字体（与 `runtime_scene/tests/custom_fonts.rs`、`font.rs` 的单元测试同源）。
fn repo_font_bytes() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    std::fs::read(&path).unwrap_or_else(|error| panic!("测试字体缺失 {}: {error}", path.display()))
}

#[test]
fn font_probe_loads_when_the_project_lives_in_the_resource_provider() {
    let root = std::env::temp_dir().join(format!("rustlua-font-vfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    let toml_path = root.join("project.toml");
    let font_path = root.join("assets/fonts/Pack.ttf");
    let toml_bytes =
        b"name = \"Web\"\nfont = \"assets/fonts/Pack.ttf\"\nfont_weight = 500\n".to_vec();
    let font_bytes = repo_font_bytes();
    let table = [
        (toml_path.clone(), toml_bytes),
        (font_path.clone(), font_bytes),
    ];
    rustlua_infra::vfs::set_provider(Box::new(move |path: &Path| {
        table
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, bytes)| bytes.clone())
    }))
    .expect("本进程首个 provider");

    // 前提：这个工程根在磁盘上**不存在**——能读到就只可能来自 provider（= wasm 上的情形）。
    assert!(
        !toml_path.is_file() && !font_path.is_file(),
        "前提：工程根不落在文件系统里"
    );

    // 配置键 → 字体路径 → 字重，一条链全走 provider。
    assert_eq!(
        rustlua_render::font::project_font::primary_path(&root),
        "assets/fonts/Pack.ttf"
    );
    assert_eq!(rustlua_render::font::project_font::weight(&root), 500.0);

    // 度量探针（`ui.measure_text` 的后端）必须能加载真字体。
    let probe = rustlua_render::font::project_font::load_probe(&root)
        .expect("provider 提供字体后探针应能加载");

    // 中文必须按**真字形**度量：宽度 > 0、四字四字形、无缺字。
    let measured = rustlua_render::text::measure_text(
        &probe,
        "江湖有田",
        32.0,
        rustlua_render::text::TextStyle::default().spacing,
    );
    assert!(
        measured.width > 0.0,
        "宽度必须是真字形算出来的：{}",
        measured.width
    );
    assert_eq!(measured.glyphs, 4, "四个汉字四个字形");
    assert!(
        measured.missing.is_empty(),
        "仓库自带字体不该缺字：{:?}",
        measured.missing_text()
    );

    let _ = std::fs::remove_dir_all(&root);
}
