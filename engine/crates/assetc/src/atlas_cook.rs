//! 图集 cook（`docs/67` §6）：把"共绘集"在构建期打成 `.rltex`/`.png` 页 + `.atlas`，
//! 并产出 `atlas_redirect.json` 让运行时把原路径重定向到 `图集#区域`。
//!
//! 配置解析、目录聚合、排除规则与装箱都在 `rustlua_assets::atlas_plan`
//! （编辑器面板用同一份；两边各写一份必然漂移，`docs/67` §8 第 4 项的教训）。
//! 本模块只负责**改产物**：写页、写定义、登记重定向、删源图、自检。
//!
//! 三条边界（都是"看起来能省、实际会炸"的地方）：
//! 1. **只打配置命中的图**，构建器不猜"同类集"；
//! 2. **瓦片集 / 动画表 / 九宫格 / Lua 拼出来的图绝不进图集**：判定在 `atlas_plan`，
//!    与装配期"只有 `Sprite` 与 UI `Image` 会被重定向"这条能力边界一致；
//! 3. **删除源图与登记重定向在同一个循环里**：只做一半 = 运行期白兜底。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail};
use rustlua_assets::atlas_plan;

/// 配置类型与产物目录对上层（CLI/编辑器）保持一致。
pub use rustlua_assets::atlas_plan::{ATLAS_DIR, AtlasConfig, AtlasSpec, AutoAtlas};
/// 重定向表的类型与解析在 infra（构建期写、运行期读，必须同一份定义）。
pub use rustlua_infra::atlas_redirect::{AtlasRedirect, REDIRECT_FILE, RedirectEntry};

/// 执行 cook：读 `out/` 里已复制好的资源，产出页与 `.atlas`、写重定向表、删源图。
///
/// 输入是**打包产物**而不是源工程：字体子集/安全边界等步骤都在 `package()` 里做过，
/// cook 只碰 `out/assets/**`，于是"包内条目 = 目录形态产物"这条等价性在 cook 之后依然成立。
pub fn cook(_project: &Path, out: &Path, config: &AtlasConfig) -> anyhow::Result<AtlasRedirect> {
    let files = atlas_plan::collect_png_files(out, "assets")?;
    let excluded = atlas_plan::collect_excluded(out)?;
    let set = atlas_plan::plan_pages(config, &files, &excluded)?;
    for warning in &set.warnings {
        eprintln!("atlas_cook: {warning}");
    }

    let mut redirect = AtlasRedirect::empty();
    for planned in &set.pages {
        let spec_name = &planned.spec_name;
        if planned.page.placements.is_empty() {
            continue;
        }
        let atlas_dir = out.join(ATLAS_DIR);
        fs::create_dir_all(&atlas_dir)?;
        let stem = format!("{}-{}", atlas_plan::sanitize(spec_name), planned.index);
        let (extension, bytes) = encode_page(planned)?;
        let texture_rel = format!("{ATLAS_DIR}/{stem}.{extension}");
        let atlas_rel = format!("{ATLAS_DIR}/{stem}.atlas");
        let texture_path = out.join(&texture_rel);
        let atlas_path = out.join(&atlas_rel);
        fs::write(&texture_path, bytes)?;
        // `.atlas` 里的 `texture=` 必须是**工程相对路径**：写绝对路径会让产物换机器就失效，
        // 而且容器条目名是相对的——两者对不上，运行时永远找不到页。
        rustlua_assets::atlas_pack::write_atlas_definition(
            &planned.page,
            Path::new(&texture_rel),
            &atlas_path,
        )
        .map_err(|error| anyhow!("写 .atlas 失败：{error}"))?;
        // 页内每条区域 → 一条重定向（原图路径不变，只是不再作为文件存在）
        for (region, _rect) in &planned.page.placements {
            let source_rel = planned
                .by_name
                .get(region)
                .ok_or_else(|| anyhow!("区域 {region} 找不到来源路径"))?;
            redirect.entries.insert(
                source_rel.clone(),
                RedirectEntry {
                    atlas: atlas_rel.clone(),
                    region: region.clone(),
                },
            );
        }
    }

    // 自检：登记完再删源图。任何一个"删了却没有重定向"的源都是运行期白兜底。
    let mut removed = 0usize;
    for source in redirect.sources().map(str::to_owned).collect::<Vec<_>>() {
        let path: PathBuf = out.join(&source);
        if !path.is_file() {
            continue;
        }
        if !redirect.entries.contains_key(&source) {
            bail!("源图 {source} 要被删除但没有重定向登记（拒绝产出白贴图）");
        }
        fs::remove_file(&path)?;
        removed += 1;
    }
    if removed > 0 {
        prune_empty_dirs(&out.join("assets"))?;
    }
    Ok(redirect)
}

/// 按配置的编码形态产出页字节，返回 `(扩展名, 字节)`。
///
/// `auto` 的判据是**进容器块之后的落盘长度**，不是原始长度（`docs/67` P1-1 的第二版）：
/// 稀疏页的裸 RGBA 有 3.7 MB、PNG 只有 80 KB，但裸像素进 zstd 块后只剩十几 KB——
/// 按原始字节比会选 PNG，把 art:ui 从 151 KB 抬到 184 KB（2026-10-18 实测）。
/// 所以两边都过一遍与写包同源的 zstd 估算，再取小的那一个。
fn encode_page(
    planned: &rustlua_assets::atlas_plan::PlannedPage,
) -> anyhow::Result<(&'static str, Vec<u8>)> {
    use rustlua_assets::atlas_plan::{AtlasFormat, encode_page_png, encode_page_rltex};
    match planned.format {
        AtlasFormat::Rltex => Ok(("rltex", encode_page_rltex(planned)?)),
        AtlasFormat::Png => Ok(("png", encode_page_png(planned)?)),
        AtlasFormat::Auto => {
            let raw = encode_page_rltex(planned)?;
            let png = encode_page_png(planned)?;
            // 估算用 L9，而写包对 `.rltex`/大块用 L19——裸像素这一侧被系统性低估。
            // 所以 PNG 要**明显**更小才换（10% 余量），否则维持 `.rltex`。
            let stored_raw = rustlua_infra::bundle::compressed_len_estimate(&raw, 9);
            let stored_png = rustlua_infra::bundle::compressed_len_estimate(&png, 9);
            if stored_png * 10 < stored_raw * 9 {
                Ok(("png", png))
            } else {
                Ok(("rltex", raw))
            }
        }
    }
}

fn prune_empty_dirs(root: &Path) -> anyhow::Result<()> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_empty_dirs(&path)?;
            children.push(path);
        }
    }
    for path in children {
        if fs::read_dir(&path)?.next().is_none() {
            fs::remove_dir(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-cook-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets/art/ui/hud")).unwrap();
        fs::create_dir_all(root.join("assets/art/ui/title")).unwrap();
        fs::create_dir_all(root.join("scenes")).unwrap();
        root
    }

    fn write_png(path: &Path, w: u32, h: u32, color: [u8; 4]) {
        let pixels: Vec<u8> = color
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        rustlua_render::texture::save_png_rgba8(path, w, h, &pixels).unwrap();
    }

    fn hud_config() -> AtlasConfig {
        AtlasConfig {
            schema: 1,
            atlases: vec![AtlasSpec {
                name: "ui_hud".to_owned(),
                include: vec!["assets/art/ui/hud/".to_owned()],
                max_size: 64,
                padding: 1,
                pot: false,
                format: "rltex".to_owned(),
                oversize: None,
            }],
            ..Default::default()
        }
    }

    fn auto_config() -> AtlasConfig {
        AtlasConfig {
            schema: 1,
            atlases: Vec::new(),
            auto: Some(AutoAtlas {
                enabled: true,
                root: "assets/art".to_owned(),
                max_size: 1024,
                padding: 1,
                format: "rltex".to_owned(),
                single_min_pixels: 256 * 256,
            }),
            separate: Vec::new(),
        }
    }

    #[test]
    fn cook_writes_rltex_pages_and_redirect_and_removes_sources() {
        let root = temp_root("basic");
        write_png(
            &root.join("assets/art/ui/hud/a.png"),
            8,
            8,
            [255, 0, 0, 255],
        );
        write_png(
            &root.join("assets/art/ui/hud/b.png"),
            8,
            8,
            [0, 255, 0, 255],
        );

        let redirect = cook(&root, &root, &hud_config()).unwrap();
        assert_eq!(redirect.entries.len(), 2, "{redirect:?}");
        let entry = redirect
            .entries
            .get("assets/art/ui/hud/a.png")
            .expect("a.png 必须有重定向");
        assert!(entry.atlas.starts_with(ATLAS_DIR));

        let page = root.join(&entry.atlas).with_extension("rltex");
        assert!(page.is_file(), "页必须存在：{}", page.display());
        assert!(
            !root.join("assets/art/ui/hud/a.png").exists(),
            "源图必须被删"
        );
        assert!(
            !root.join("assets/art/ui/hud/b.png").exists(),
            "源图必须被删"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// `format` 省略时用 `rltex`（P0-2：曾经默认成 `rtex`，省略即构建失败）。
    #[test]
    fn format_defaults_to_rltex() {
        let config: AtlasConfig = serde_json::from_str(
            r#"{"schema":1,"atlases":[{"name":"ui","include":["assets/art/ui/hud/"]}]}"#,
        )
        .unwrap();
        let root = temp_root("format-default");
        write_png(&root.join("assets/art/ui/hud/a.png"), 8, 8, [1, 2, 3, 255]);
        let redirect = cook(&root, &root, &config).unwrap();
        let atlas = redirect
            .entries
            .get("assets/art/ui/hud/a.png")
            .unwrap()
            .atlas
            .clone();
        let definition =
            rustlua_assets::atlas_format::AtlasDefinition::load(root.join(atlas)).unwrap();
        assert!(
            definition.texture.ends_with(".rltex"),
            "{}",
            definition.texture
        );
        fs::remove_dir_all(&root).ok();
    }

    /// `format=auto`：稀疏页必须选裸像素（进块后远小于 PNG）。
    ///
    /// 这条刻意不写"稠密图必须选 PNG"：判据是"进块之后谁更小"，不是图像学直觉。
    /// 第一版按原始字节比，把 art:ui 从 151 KB 抬到 184 KB，这里钉住不回归。
    #[test]
    fn auto_format_picks_the_smaller_container_cost() {
        let root = temp_root("auto-format");
        // 稀疏页：64×64 只放得下一张 8×8 的不透明图，其余全透明。
        write_png(&root.join("assets/art/ui/hud/a.png"), 8, 8, [7, 9, 11, 255]);
        write_png(&root.join("assets/art/ui/hud/b.png"), 8, 8, [7, 9, 11, 255]);
        let mut config = hud_config();
        config.atlases[0].max_size = 64;
        config.atlases[0].format = "auto".to_owned();
        let redirect = cook(&root, &root, &config).unwrap();
        let atlas = redirect
            .entries
            .get("assets/art/ui/hud/a.png")
            .unwrap()
            .atlas
            .clone();
        let definition =
            rustlua_assets::atlas_format::AtlasDefinition::load(root.join(atlas)).unwrap();
        assert!(
            definition.texture.ends_with(".rltex"),
            "稀疏页必须选裸像素（zstd 后远小于 PNG），实际 {}",
            definition.texture
        );
        assert!(root.join(&definition.texture).is_file());
        fs::remove_dir_all(&root).ok();
    }

    /// 目录默认聚合：同目录的小图并进 `<目录>_default`。
    #[test]
    fn auto_merges_small_images_in_one_directory() {
        let root = temp_root("auto-merge");
        write_png(
            &root.join("assets/art/ui/title/a.png"),
            8,
            8,
            [1, 1, 1, 255],
        );
        write_png(
            &root.join("assets/art/ui/title/b.png"),
            8,
            8,
            [2, 2, 2, 255],
        );
        let redirect = cook(&root, &root, &auto_config()).unwrap();
        let a = redirect.entries.get("assets/art/ui/title/a.png").unwrap();
        let b = redirect.entries.get("assets/art/ui/title/b.png").unwrap();
        assert_eq!(a.atlas, b.atlas, "同一目录的小图必须共用一页");
        assert!(a.atlas.contains("title_default"), "{}", a.atlas);
        assert_ne!(a.region, b.region);
        fs::remove_dir_all(&root).ok();
    }

    /// `separate` 与"面积超阈值"都必须让那张图单独成集。
    #[test]
    fn auto_honours_separate_and_large_images() {
        let root = temp_root("auto-single");
        write_png(
            &root.join("assets/art/ui/title/ui1.png"),
            8,
            8,
            [1, 0, 0, 255],
        );
        write_png(
            &root.join("assets/art/ui/title/ui2.png"),
            8,
            8,
            [2, 0, 0, 255],
        );
        write_png(
            &root.join("assets/art/ui/title/big.png"),
            512,
            512,
            [3, 0, 0, 255],
        );
        let mut config = auto_config();
        config.auto.as_mut().unwrap().root = "assets/art/ui".to_owned();
        config.separate = vec!["assets/art/ui/title/ui2.png".to_owned()];

        let redirect = cook(&root, &root, &config).unwrap();
        let atlas_of = |rel: &str| {
            redirect
                .entries
                .get(rel)
                .unwrap_or_else(|| panic!("{rel} 必须有重定向"))
                .atlas
                .clone()
        };
        assert!(atlas_of("assets/art/ui/title/ui2.png").contains("title_ui2"));
        assert!(atlas_of("assets/art/ui/title/big.png").contains("title_big"));
        // ui1 是目录里唯一可合并的图 → 没有合并收益，按单图集处理
        assert!(atlas_of("assets/art/ui/title/ui1.png").contains("title_ui1"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tilemap_texture_is_refused() {
        let root = temp_root("tilemap");
        write_png(
            &root.join("assets/art/ui/hud/ground.png"),
            8,
            8,
            [0, 0, 255, 255],
        );
        fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\ncomponent\t1\tTilemap\ttexture=assets/art/ui/hud/ground.png;atlas_cols=2\n",
        )
        .unwrap();
        let error = cook(&root, &root, &hud_config()).expect_err("瓦片纹理必须被拒绝");
        assert!(error.to_string().contains("Tilemap"), "{error}");
        assert!(
            root.join("assets/art/ui/hud/ground.png").is_file(),
            "拒绝时不许删源图"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// 九宫格：按组件字段判（不是只按文件名 `_9s`）。
    #[test]
    fn nine_slice_component_is_refused() {
        let root = temp_root("nine-slice");
        fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        write_png(
            &root.join("assets/art/ui/hud/panel.png"),
            64,
            64,
            [9, 9, 9, 255],
        );
        fs::write(
            root.join("assets/prefabs/hud.prefab"),
            "prefab_version=1\ncomponent\t2\tImage\ttexture=assets/art/ui/hud/panel.png;nine_slice=4,4,4,4\n",
        )
        .unwrap();
        let error = cook(&root, &root, &hud_config()).expect_err("九宫格必须被拒绝");
        assert!(error.to_string().contains("九宫格"), "{error}");
        assert!(
            root.join("assets/art/ui/hud/panel.png").is_file(),
            "拒绝时不许删源图"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversize_can_be_skipped() {
        let root = temp_root("oversize");
        write_png(
            &root.join("assets/art/ui/hud/small.png"),
            8,
            8,
            [1, 2, 3, 255],
        );
        write_png(
            &root.join("assets/art/ui/hud/huge.png"),
            200,
            8,
            [4, 5, 6, 255],
        );
        let mut config = hud_config();
        config.atlases[0].oversize = Some("skip".to_owned());

        let redirect = cook(&root, &root, &config).unwrap();
        assert!(redirect.entries.contains_key("assets/art/ui/hud/small.png"));
        assert!(
            !redirect.entries.contains_key("assets/art/ui/hud/huge.png"),
            "超限源不该进图集"
        );
        assert!(
            root.join("assets/art/ui/hud/huge.png").is_file(),
            "被跳过的源必须原样保留"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// 复核 P1-1 的白名单门：**除 `Sprite`/`Image` 之外的任何组件**引用纹理，都不许被图集吃掉。
    #[test]
    fn non_sprite_component_texture_is_refused() {
        let root = temp_root("non-sprite");
        write_png(
            &root.join("assets/art/ui/hud/backdrop.png"),
            8,
            8,
            [9, 9, 9, 255],
        );
        fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\ncomponent\t1\tBackdrop\ttexture=assets/art/ui/hud/backdrop.png;layer=0\n",
        )
        .unwrap();
        let error = cook(&root, &root, &hud_config()).expect_err("非 Sprite 引用必须被拒绝");
        assert!(
            error.to_string().contains("自取纹理的消费方"),
            "错误要说清原因：{error}"
        );
        assert!(
            root.join("assets/art/ui/hud/backdrop.png").is_file(),
            "拒绝时不许删源图"
        );
        fs::remove_dir_all(&root).ok();
    }
}
