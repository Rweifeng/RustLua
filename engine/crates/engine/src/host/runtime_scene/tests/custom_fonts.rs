//! 原 `mod sdf_tests` 里 `custom_fonts` 这一域（WP12 阶段 1 纯搬运；源行段 9812–10020）。
//!
//! - `scene_text_custom_font_surfaces_in_mesh`
//! - `text_render_mode_forces_vector_or_sdf_for_custom_fonts`
//! - `text_render_mode_parsing_uses_auto_fallback`
//! - `sourcehan_sdf_mode_emits_default_font_draws`
//! - `pixel_font_stays_out_of_the_vector_channel_even_in_forced_vector_mode`

use super::*;
use rustlua_scene::scene::Scene;
use std::fs;

/// 场景 Text 指定自定义字体时：该字体 key 出现在 SceneMesh 的自定义图集
/// 上报与纹理路径里；默认文本在短文本场景改走矢量路径。用临时项目根存放
/// 两个字体文件（同源，仅验证管线/命名空间，不验证字形差异）。
#[test]
fn scene_text_custom_font_surfaces_in_mesh() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-custom-font-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    let _ = fs::create_dir_all(&fonts_dir);
    fs::copy(&source, fonts_dir.join("NotoSansSC-VF.ttf")).expect("copy default font");
    fs::copy(&source, fonts_dir.join("Custom.ttf")).expect("copy custom font");

    let mut scene = Scene::new("custom-fonts");
    scene
        .add_entity(SceneEntityId(1), "DefaultText", None)
        .unwrap();
    scene
        .add_entity(SceneEntityId(2), "CustomText", None)
        .unwrap();
    let layout = "anchors=0,0,0,0;offsets=0,0,64,16".to_owned();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("UiLayout".into(), layout.clone());
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Text".into(), "text=Default;font_size=24".into());
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("UiLayout".into(), layout);
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Text".into(),
            "text=Custom;font_size=24;font=assets/fonts/Custom.ttf".into(),
        );

    let result = RuntimeScene::from_scene_edit(Some(&root), scene)
        .and_then(|mut runtime| runtime.vertices(320, 240));
    let _ = fs::remove_dir_all(&root);
    let mesh = result.expect("vertices");
    assert!(
        mesh.custom_font_atlases
            .iter()
            .any(|font| font.key == "font://assets/fonts/Custom.ttf"),
        "custom font key should be reported to the renderer"
    );
    let paths: Vec<&str> = mesh.paths.iter().map(|path| path.as_str()).collect();
    assert!(
        paths.contains(&"font://assets/fonts/Custom.ttf"),
        "custom font path should be in mesh paths: {paths:?}"
    );
    assert!(
        !paths.contains(&FONT_ATLAS_TEXTURE_PATH),
        "default short text should use the vector path: {paths:?}"
    );
    assert!(
        mesh.vector_vertices.len() >= 4,
        "default short text should emit vector vertices"
    );
}

#[test]
fn text_render_mode_forces_vector_or_sdf_for_custom_fonts() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-text-render-mode-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::copy(&source, fonts_dir.join("Custom.ttf")).unwrap();

    let mut scene = Scene::new("render-mode");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=20,20,240,80".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Text".into(),
            "text=Mode;font_size=48;font=assets/fonts/Custom.ttf".into(),
        );

    let project = |mode: &str| {
        fs::write(
                root.join("project.toml"),
                format!(
                    "[project]\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\ntext_render_mode = \"{mode}\"\n"
                ),
            )
            .unwrap();
    };
    project("vector");
    let vector = RuntimeScene::from_scene_edit(Some(&root), scene.clone())
        .and_then(|mut runtime| runtime.vertices(320, 240))
        .expect("vector vertices");
    assert!(!vector.vector_vertices.is_empty());
    assert!(
        vector.custom_font_atlases.is_empty(),
        "forced vector mode should not build custom SDF atlases"
    );

    project("sdf");
    let sdf = RuntimeScene::from_scene_edit(Some(&root), scene)
        .and_then(|mut runtime| runtime.vertices(320, 240))
        .expect("sdf vertices");
    assert!(sdf.vector_vertices.is_empty());
    assert!(
        sdf.custom_font_atlases
            .iter()
            .any(|font| font.key == "font://assets/fonts/Custom.ttf")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn text_render_mode_parsing_uses_auto_fallback() {
    assert_eq!(TextRenderMode::parse("auto"), TextRenderMode::Auto);
    assert_eq!(TextRenderMode::parse(" Vector "), TextRenderMode::Vector);
    assert_eq!(TextRenderMode::parse("MSDF"), TextRenderMode::Sdf);
    assert_eq!(TextRenderMode::parse("hinted"), TextRenderMode::Hinted);
    assert_eq!(TextRenderMode::parse(" TTF "), TextRenderMode::Hinted);
    assert_eq!(TextRenderMode::parse("bad"), TextRenderMode::Auto);
}

#[test]
fn sourcehan_sdf_mode_emits_default_font_draws() {
    let root = rustlua_infra::games_dir()
        .join("examples")
        .join("sourcehan_test");
    if !root.join("scenes/font_check.scene").is_file() {
        return;
    }
    let path = root.join("scenes/font_check.scene");
    let scene = Scene::load(&path).unwrap();
    let font_atlas = load_project_font_atlas(Some(&root)).expect("project font atlas");
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    runtime.set_font_atlas(Some(font_atlas));
    runtime.set_text_render_mode(TextRenderMode::Sdf);
    runtime.set_render_ui_overlay(false);
    let hidden = runtime.vertices(1280, 720).unwrap();
    runtime.set_render_ui_overlay(true);
    let mesh = runtime.vertices(1280, 720).unwrap();
    let sdf_draws = mesh.draws.iter().filter(|draw| draw.sdf).count();
    let font_paths = mesh
        .paths
        .iter()
        .filter(|path| path.as_str() == FONT_ATLAS_TEXTURE_PATH)
        .count();
    assert!(
        sdf_draws > 0 && font_paths > 0,
        "sdf draws={sdf_draws}, font paths={font_paths}, all={:?}",
        mesh.paths
    );
    // 空网格帧（overlay=false）不得消费版本号：首个可绘制帧必须携带全量
    // 像素（否则新渲染器会以空数据 write_texture 整张图集 → wgpu 崩溃）。
    assert!(
        hidden
            .font_atlas
            .as_ref()
            .is_some_and(|font| font.pixels.is_empty()),
        "empty-mesh frame must not consume the atlas version (deferred to drawable frame)"
    );
    assert!(
        mesh.font_atlas
            .as_ref()
            .is_some_and(|font| !font.pixels.is_empty()),
        "first drawable frame must carry full atlas pixels"
    );
}

/// M72-03：点阵字体的判定必须**压过** `text_render_mode`——即使强行写 `vector`，
/// 一句中文也只能进阈值档（`sdf == 2`），轮廓通道一个顶点都不许有。
///
/// 这条同时钉住一个真实回归：`push_ui_text` 与场景 UI 文本排版都曾**无条件**调用
/// `push_vector_text_run`（只在其后才判 `use_vector`），于是被挡在矢量档之外的文字
/// 依然往 `vector_vertices` 里攒整份轮廓顶点（实测一帧 510 个），渲染器把同一句字
/// 再画一遍——点阵层与轮廓层错位叠字，而日志一个字都没有。世界文字那条一直是
/// `if use_vector && push_…` 的正确写法，这两处是漏网的。
#[test]
fn pixel_font_stays_out_of_the_vector_channel_even_in_forced_vector_mode() {
    let project = rustlua_infra::games_dir()
        .join("examples")
        .join("game_jhyt");
    let source = project.join("assets/fonts/FusionPixel-12px-Monospaced-zhHans.ttf");
    // 只在"整个示例工程没被 checkout"时让路。工程在盘上、`project.toml` 声明的主字体却不在，
    // 就是字体被误删或漏提交——那时静默 `return` 等于把这条挡板变成零断言
    // （与 `docs/49` 说的"画了但没登记"同一类零报错坏行为）。
    if !project.join("project.toml").is_file() {
        return;
    }
    assert!(
        source.is_file(),
        "示例工程在盘上却找不到它声明的主字体：{}（这条挡板没有字体就是零断言）",
        source.display()
    );
    let root = std::env::temp_dir().join(format!(
        "rustlua-pixel-vector-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).expect("create fonts dir");
    fs::copy(&source, fonts_dir.join("Pixel.ttf")).expect("copy pixel font");
    fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/Pixel.ttf\"\n",
    )
    .expect("write project.toml");

    let mut scene = Scene::new("pixel-vector");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=0,0,120,24".to_owned(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Text".into(), "text=山庄炼药;font_size=12".into());

    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
    runtime.set_text_render_mode(TextRenderMode::Vector);
    // 挡板本身：forced vector 模式下，字体判定仍然把它按回点阵档。
    assert!(
        !runtime.should_render_vector(None, 1.0, 4),
        "pixel font must not take the vector path even in forced vector mode"
    );
    let mesh = runtime.vertices(320, 240).expect("vertices");
    let _ = fs::remove_dir_all(&root);

    assert!(
        mesh.vector_vertices.is_empty(),
        "矢量通道本帧有 {} 个顶点：被判定挡掉的文字不许生成轮廓顶点",
        mesh.vector_vertices.len()
    );
    let sdf_values: Vec<u32> = mesh.vertices.iter().map(|v| v.sdf).collect();
    assert!(
        !sdf_values.contains(&1),
        "点阵字体不许出现平滑 MSDF 档（sdf=1）：{sdf_values:?}"
    );
    assert!(
        sdf_values.iter().filter(|value| **value == 2).count() >= 4 * 6,
        "至少 4 个字形 ×6 顶点应落在阈值档（sdf=2），实际 {sdf_values:?}"
    );
    let paths: Vec<&str> = mesh.paths.iter().map(|path| path.as_str()).collect();
    assert!(
        paths.contains(&FONT_ATLAS_TEXTURE_PATH),
        "点阵字形应走默认字体图集纹理，实际 {paths:?}"
    );
    let sdf_draws: Vec<(bool, bool)> = mesh
        .draws
        .iter()
        .filter(|draw| draw.sdf)
        .map(|draw| (draw.bitmap, draw.vector))
        .collect();
    assert!(
        !sdf_draws.is_empty() && sdf_draws.iter().all(|&(bitmap, vector)| bitmap && !vector),
        "点阵 draw 必须带 bitmap 标记（渲染器据此绑 Nearest 采样）且不得是矢量 draw，实际 (bitmap,vector)={sdf_draws:?}"
    );
}

/// 带 TrueType 指令表的测试字体（Sarasa 等 OFL 字体不入库）。没设环境变量时返回
/// `None`——hinting 图集只能在真有指令表的字体上验证，拿 Noto 跑等于零断言。
fn hinted_test_font() -> Option<(String, Vec<u8>)> {
    let path = std::env::var("RUSTLUA_HINTED_TEST_FONT").ok()?;
    fs::read(&path).ok().map(|bytes| (path, bytes))
}

/// hinting 档遇到没有 `fpgm`/`prep`/`cvt ` 的字体时必须**回退 SDF**：
/// 画出来的字形等于未 hinting，却会让人以为这条档生效了。
#[test]
fn hinted_mode_without_hinting_tables_falls_back_to_sdf() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    if !source.is_file() {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-hinted-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::copy(&source, fonts_dir.join("NotoSansSC-VF.ttf")).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\ntext_render_mode = \"hinted\"\n",
    )
    .unwrap();

    let mut scene = Scene::new("hinted-fallback");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=0,0,160,24".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Text".into(), "text=山庄;font_size=12".into());

    let mesh = RuntimeScene::from_scene_edit(Some(&root), scene)
        .and_then(|mut runtime| runtime.vertices(320, 240))
        .expect("vertices");
    let _ = fs::remove_dir_all(&root);

    let hinted_paths: Vec<&str> = mesh
        .paths
        .iter()
        .map(String::as_str)
        .filter(|path| path.contains("/hinted/"))
        .collect();
    assert!(
        hinted_paths.is_empty(),
        "没有指令表的字体不该产出 hinting 图集：{hinted_paths:?}"
    );
    let sdf_values: Vec<u32> = mesh.vertices.iter().map(|vertex| vertex.sdf).collect();
    assert!(
        !sdf_values.contains(&3),
        "回退帧不许出现覆盖率档顶点（sdf=3）：{sdf_values:?}"
    );
    assert!(
        mesh.draws.iter().any(|draw| draw.sdf),
        "回退后仍应画出 SDF 文字"
    );
}

/// hinting 档的正常路径：字形落在覆盖率档（`sdf=3` + Nearest 绑定），图集按
/// `font://hinted/<ppem>/...` 上报，ppem 跟随 `render_scale` 取物理像素。
///
/// 需要真带指令表的字体：`RUSTLUA_HINTED_TEST_FONT=<Sarasa 等 ttf 路径>`。
#[test]
fn hinted_mode_emits_coverage_draws_and_follows_render_scale() {
    let Some((_, bytes)) = hinted_test_font() else {
        eprintln!("跳过：未设置 RUSTLUA_HINTED_TEST_FONT（需要一个带 fpgm/prep/cvt 的字体）");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "rustlua-hinted-draw-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::write(fonts_dir.join("Hinted.ttf"), &bytes).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/Hinted.ttf\"\ntext_render_mode = \"hinted\"\n",
    )
    .unwrap();

    let mut scene = Scene::new("hinted-draw");
    scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=8,8,200,40".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Text".into(), "text=山庄;font_size=12".into());

    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime");
    let first = runtime.vertices(320, 240).expect("design-scale vertices");
    let key_12 = "font://hinted/12/__default__";
    assert!(
        first.paths.iter().any(|path| path == key_12),
        "12px（scale=1）应上报 ppem=12 的 hinting 图集：{:?}",
        first.paths
    );
    let atlas = first
        .custom_font_atlases
        .iter()
        .find(|font| font.key == key_12)
        .expect("hinting atlas snapshot");
    assert!(
        !atlas.pixels.pixels.is_empty(),
        "首帧必须携带全量覆盖率像素（否则渲染器绑不出图集）"
    );
    let coverage_vertices = first
        .vertices
        .iter()
        .filter(|vertex| vertex.sdf == 3)
        .count();
    assert!(
        coverage_vertices >= 2 * 6,
        "两个字形至少 12 个覆盖率顶点，实际 {coverage_vertices}"
    );
    assert!(
        first
            .draws
            .iter()
            .any(|draw| draw.sdf && draw.bitmap && !draw.vector),
        "hinting draw 必须是 sdf+bitmap（渲染器据此绑 Nearest）"
    );
    assert!(
        first.vector_vertices.is_empty(),
        "hinted 模式不该同时产出矢量顶点"
    );
    assert!(
        !first.vertices.iter().any(|vertex| vertex.sdf == 2),
        "hinted 模式不该出现阈值档（sdf=2）顶点"
    );

    // 设计盒被放大 2 倍：ppem 必须按物理像素走（12 → 24），旧图集 key 不再命中。
    runtime.set_render_scale(2.0);
    let second = runtime.vertices(320, 240).expect("physical-scale vertices");
    let _ = fs::remove_dir_all(&root);
    let key_24 = "font://hinted/24/__default__";
    assert!(
        second.paths.iter().any(|path| path == key_24),
        "render_scale=2 应上报 ppem=24 的 hinting 图集：{:?}",
        second.paths
    );
    assert!(
        !second.paths.iter().any(|path| path == key_12),
        "ppem 变化后旧图集不该再被引用"
    );
}

/// `ui.measure_text` 必须跟着渲染侧的整数物理 ppem 走：hinted 档把字号量化成
/// `round(字号 × render_scale)`，度量若不量化就比实画窄 3~4%，调用方按更窄的宽度
/// 居中，整行向右溢（实测打包窗口里菜单底部提示被右边缘裁掉）。
///
/// 这条门走完整链路：Lua 绑定 → `ScriptState::text_hint_scale` →
/// `measure_text_with_hint_scale`，并验证切回非 hinted 档后度量恢复设计字号口径。
#[test]
fn measure_text_quantizes_to_hinted_ppem() {
    let Some((_, bytes)) = hinted_test_font() else {
        eprintln!("跳过：未设置 RUSTLUA_HINTED_TEST_FONT（需要一个带 fpgm/prep/cvt 的字体）");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "rustlua-hinted-measure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let fonts_dir = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts_dir).unwrap();
    fs::write(fonts_dir.join("Hinted.ttf"), &bytes).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nfont = \"assets/fonts/Hinted.ttf\"\ntext_render_mode = \"hinted\"\n",
    )
    .unwrap();

    let mut runtime =
        RuntimeScene::from_scene_edit(Some(&root), Scene::new("hinted-measure")).expect("runtime");
    let width = |runtime: &mut RuntimeScene| -> f32 {
        runtime
            .script
            .execute_console("return ui.measure_text('开始游戏', 18).width")
            .expect("ui.measure_text")
            .parse()
            .expect("width 数字")
    };
    // 初始 render_scale=1.0：ppem=18 与设计字号一致，即未量化口径。
    let design = width(&mut runtime);
    // 0.751 ≈ 打包窗口客户区 541 / 设计宽 720：ppem 量化成 14，设计等效 18.64px/em。
    runtime.set_render_scale(0.751);
    let physical = width(&mut runtime);
    // 切回非 hinted 档：量化关闭，度量必须逐位回到设计口径。
    runtime.set_text_render_mode(TextRenderMode::Vector);
    let restored = width(&mut runtime);
    let _ = fs::remove_dir_all(&root);

    let ratio = physical / design;
    assert!(
        (1.02..1.05).contains(&ratio),
        "hinted 度量应比设计口径宽 3% 左右（ppem 18→14），实际 {ratio}（design={design}, physical={physical}）"
    );
    assert!(
        (restored - design).abs() < 0.01,
        "非 hinted 档度量必须不量化：restored={restored} design={design}"
    );
}
