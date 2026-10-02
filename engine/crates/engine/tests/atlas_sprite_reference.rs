//! 批 B 门：场景 Sprite 的 `图集路径#区域名` 引用必须在装配期解析为
//! "纹理工程根路径 + 区域 UV"，运行时抽取/合批路径零改动。
//!
//! 路径形状取**真实项目里的写法**：场景组件串与 `.atlas` 的 `texture=` 都是工程根相对
//! 路径，由运行时的 project_root 解析（与渲染器读纹理同一规则）。

use engine::core::scene::{Scene, SceneEntityId};
use engine::core::script::LuaRuntime;
use engine::core::sprite::{Sprite, SpriteAsset};
use engine::core::transform::Vec2;
use std::fs;
use std::path::{Path, PathBuf};

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-atlas-scene-ref-{tag}-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// 在 `root/assets/atlases/` 下写 4x2 真实 PNG（左半红右半绿）+ 引用它的 `hero.atlas`，
/// 返回两者的**工程根相对**路径（`.atlas` 里的 `texture=` 也按这份相对路径写）。
fn write_atlas_project(root: &Path) -> (String, String) {
    let dir = root.join("assets").join("atlases");
    fs::create_dir_all(&dir).unwrap();
    let mut img = image::RgbaImage::new(4, 2);
    for (x, _y, pixel) in img.enumerate_pixels_mut() {
        *pixel = if x < 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 255, 0, 255])
        };
    }
    img.save(dir.join("hero-0.png")).unwrap();
    fs::write(
        dir.join("hero.atlas"),
        "atlas_version=1\ntexture=assets/atlases/hero-0.png\nregion\twalk\t2\t0\t2\t2\n",
    )
    .unwrap();
    (
        "assets/atlases/hero-0.png".to_owned(),
        "assets/atlases/hero.atlas".to_owned(),
    )
}

fn scene_with_sprite(texture: &str) -> Scene {
    let mut scene = Scene::new("atlas-ref");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), format!("texture={texture};size=1x1"));
    scene
}

fn spawned_sprite(runtime: &mut LuaRuntime) -> (String, Sprite) {
    runtime.with_world(|world| {
        let id = *world
            .entities_with::<SpriteAsset>()
            .first()
            .expect("SpriteAsset 应随图集引用落进世界");
        let asset = world.get::<SpriteAsset>(id).unwrap().unwrap();
        let sprite = *world.get::<Sprite>(id).unwrap().unwrap();
        (asset.0.clone(), sprite)
    })
}

#[test]
fn scene_atlas_reference_resolves_to_sheet_uv_at_spawn() {
    let root = temp_root("ok");
    let (sheet, atlas) = write_atlas_project(&root);

    let scene = scene_with_sprite(&format!("{atlas}#walk"));
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

    let (asset_path, sprite) = spawned_sprite(&mut runtime);
    assert_eq!(
        asset_path, sheet,
        "解析后的 SpriteAsset 必须是 .atlas 里写的工程根相对纹理路径"
    );
    assert_eq!(sprite.uv_min, Vec2::new(0.5, 0.0));
    assert_eq!(sprite.uv_max, Vec2::new(1.0, 1.0));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn scene_plain_texture_reference_still_passes_through() {
    let root = temp_root("plain");
    let scene = scene_with_sprite("art/idle.png");
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);
    let (asset_path, _) = spawned_sprite(&mut runtime);
    assert_eq!(asset_path, "art/idle.png");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn scene_broken_atlas_reference_fails_loudly_not_silently() {
    let root = temp_root("broken");
    let (_, atlas) = write_atlas_project(&root);

    // 区域不存在：装配必须报错，而不是回退成整图。
    let scene = scene_with_sprite(&format!("{atlas}#fly"));
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let error = runtime
        .instantiate_scene(&scene)
        .expect_err("未知区域必须显式失败");
    assert!(error.to_string().contains("fly"), "诊断需含区域名: {error}");

    // 图集文件不存在：同样显式失败。
    let scene = scene_with_sprite("assets/atlases/missing.atlas#walk");
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    assert!(runtime.instantiate_scene(&scene).is_err());
    let _ = fs::remove_dir_all(&root);
}

/// 图集引用要靠 project_root 才解析得到：没设根时不能静默退化成整张图。
#[test]
fn scene_atlas_reference_without_project_root_is_an_error() {
    let root = temp_root("no-root");
    let (_, atlas) = write_atlas_project(&root);
    let scene = scene_with_sprite(&format!("{atlas}#walk"));
    let mut runtime = LuaRuntime::new().unwrap();
    let error = runtime
        .instantiate_scene(&scene)
        .expect_err("相对图集路径在没设工程根时必须失败");
    assert!(
        error.to_string().contains(&atlas),
        "诊断需含图集路径: {error}"
    );
    let _ = fs::remove_dir_all(&root);
}
