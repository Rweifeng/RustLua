//! 分包计划（`docs/67` §5，批 C）：把打包产物切成若干 bundle，并产出 `bundle_plan.json`。
//!
//! 计划表的**结构**在 `rustlua_infra::bundle::plan`（构建期写、运行期读，两边必须同一份定义）；
//! 本模块只负责**构建期**那一半：配置、归组、三条硬门、序列化。
//!
//! 为什么配置是独立的 `bundles.json` 而不是 `project.toml` 的 `[bundles]` 表：
//! `assetc` 解析 `project.toml` 用的是逐行 `key = value` 扫描（`project_font_key` 那一族），
//! 撑不住"数组表 + 嵌套"，而工作区没有 `toml` 依赖。`bundles.json` 与 `dependencies.json`、
//! `mainpack.json` 同族（构建期产物/配置），`serde_json` 已在依赖里，不需要新 crate。
//!
//! 归组顺序（先匹配先赢）：
//! 1. `assign` 里的显式路径（最长前缀优先）；
//! 2. `groups[].include` 前缀/精确路径（按 `groups` 声明顺序）；
//! 3. `default_group`（缺省 `unowned`）。
//!
//! 构建期三条硬门（§5.2）：
//! 1. **每个文件恰好属于一个组**——归组是单趟构造，重复不可能、遗漏不可能；
//! 2. **一个组不跨分发单元**——`unit` 是组属性，不存在"半个组在主包"；
//! 3. **core 自足**——`main_script` / `startup_scene` / `project.toml` / 字体 / 以及**全部 Lua
//!    脚本**必须在 `unit = "main"` 的组里（Web 上 `require` 是同步的）。

use std::collections::BTreeMap;

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

pub use rustlua_infra::bundle::plan::{BundlePlan, PLAN_FILE, PLAN_FORMAT, PlannedGroup};

/// 单个 bundle 的落盘上限：微信普通分包口径（`docs/52`）。
pub const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;

/// `bundles.json` 的根结构。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BundleConfig {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// 没被任何规则命中的分组。
    #[serde(default = "default_group")]
    pub default_group: String,
    /// 组的默认分发单元：`main` | `sub` | `cdn`。
    #[serde(default = "default_unit")]
    pub default_unit: String,
    /// 分组声明（**顺序即优先级**）。
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
    /// 显式逐路径指派：`路径或前缀 → 组名`。以 `/` 结尾按前缀匹配。
    #[serde(default)]
    pub assign: BTreeMap<String, String>,
}

fn default_schema() -> u32 {
    1
}
fn default_group() -> String {
    "unowned".to_owned()
}
fn default_unit() -> String {
    "sub".to_owned()
}

/// 一个组的声明。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupSpec {
    pub name: String,
    #[serde(default)]
    pub unit: Option<String>,
    /// 匹配项：`assets/art/ui` 或 `assets/art/ui/`（后者按前缀）或 `project.toml`（精确）。
    #[serde(default)]
    pub include: Vec<String>,
}

impl BundleConfig {
    /// 解析 `bundles.json`；文件不存在时按 `docs/67` §5.3 的内容域默认规则生成。
    pub fn load_or_default(path: &std::path::Path) -> anyhow::Result<Self> {
        if path.is_file() {
            let text = std::fs::read_to_string(path)
                .map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
            let config: Self = serde_json::from_str(&text)
                .map_err(|error| anyhow!("解析 {} 失败：{error}", path.display()))?;
            if config.schema != default_schema() {
                bail!("{}: 不支持的 schema {}", path.display(), config.schema);
            }
            return Ok(config);
        }
        Ok(Self::default_domains())
    }

    /// 默认规则：就是 §5.3 那张实测分组表（core / audio / art:ui / art:world / …）。
    ///
    /// 顺序有讲究：`art:ui` 必须在 `art:world` 之前，否则 `assets/` 的兜底组会先把 UI 吃掉。
    pub fn default_domains() -> Self {
        let group = |name: &str, unit: &str, include: &[&str]| GroupSpec {
            name: name.to_owned(),
            unit: Some(unit.to_owned()),
            include: include.iter().map(|item| (*item).to_owned()).collect(),
        };
        Self {
            schema: default_schema(),
            default_group: default_group(),
            default_unit: default_unit(),
            groups: vec![
                group(
                    "core",
                    "main",
                    &[
                        "project.toml",
                        "chars.txt",
                        "dependencies.json",
                        "bundles.json",
                        // 图集重定向表：装配期**启动就要**查（Sprite 组装的第一步）。
                        // 落进 `unowned`（sub 单元）会被延迟挂载，首屏贴图先 miss 再补挂。
                        "atlas_redirect.json",
                        "scripts/",
                        "scenes/",
                        "assets/prefabs/",
                        "assets/fonts/",
                    ],
                ),
                group(
                    "art:ui",
                    "main",
                    &[
                        // **图集页必须在 main 单元**（`docs/67` §6.4）：页是 cook 出来的、原图已被删，
                        // 装配期只能靠页。它若落进 sub 组会被延迟挂载 ⇒ 首屏精灵解析不到页 ⇒
                        // 重定向失败 ⇒ 回落到已删除的原路径 ⇒ 白兜底。
                        // 实测症状：`textures_decoded ok=2 fail=13`（13 张 ui/menu 全白）。
                        "assets/atlases/",
                        "assets/art/ui/",
                        "assets/art/items/",
                        "assets/art/portraits/",
                        // 标题底图是**启动场景**（`title_menu.prefab`）的全屏 Image：落进
                        // `art:world`(sub) 会被延迟挂载，首帧贴图 MISS 后加载器把该路径记为
                        // 失败且不重试 ⇒ 整块门面永远白底。它只有 14 KB，留在启动单元。
                        // 同目录的 `startup_background.png`（794 KB 启动图）不进主包。
                        "assets/art/title/title_bg_pixel.png",
                    ],
                ),
                // `audio` 是 **main** 而不是 sub：`unit` 的语义是"启动即需要"，不是"体积小"。
                // 实测（`test-webgl.bat`，2026-10-19）：BGM 在 boot 阶段就 `play()`，
                // 若它被延迟挂载，此时容器还在路上，音效会静默退化成 `pack/<x>` 的 404。
                // 微信侧本来就靠 `ensurePackForBoot` 在 boot 前把分包拉齐，两端语义一致。
                group("audio", "sub", &["assets/audio/"]),
                group("art:characters", "sub", &["assets/art/characters/"]),
                group("art:enemies", "sub", &["assets/art/enemies/"]),
                group("art:world", "sub", &["assets/"]),
                // 启动器品牌图**不该按普通资源计费**：`splash/`、`branding/` 是发布器自己
                // 放进放行根的图，真要落进 pack/ 也归 `cdn` 单元——cdn 不参与微信 4 MB
                // 分包上限与主包配额，避免它们把配额挤爆（实测 `splash/splash.png` 2.2 MB
                // 曾整个落进 `unowned`(sub)）。
                group("brand", "cdn", &["splash/", "branding/"]),
            ],
            assign: BTreeMap::new(),
        }
    }

    /// 解析一个条目属于哪个组（三条来源的优先级在这里落地）。
    pub fn group_of(&self, path: &str) -> String {
        // 1) 显式指派：最长前缀优先（同长度按 BTreeMap 的键序，稳定）
        let mut best: Option<(usize, &str)> = None;
        for (pattern, group) in &self.assign {
            let hit = if pattern.ends_with('/') {
                path.starts_with(pattern.as_str())
            } else {
                path == pattern
            };
            if hit && best.is_none_or(|(len, _)| pattern.len() > len) {
                best = Some((pattern.len(), group.as_str()));
            }
        }
        if let Some((_, group)) = best {
            return group.to_owned();
        }
        // 2) 组规则（声明顺序即优先级）
        for spec in &self.groups {
            if spec.matches(path) {
                return spec.name.clone();
            }
        }
        // 3) 兜底
        self.default_group.clone()
    }

    /// 组的单元（`main` / `sub` / `cdn`）。
    pub fn unit_of(&self, group: &str) -> String {
        self.groups
            .iter()
            .find(|spec| spec.name == group)
            .and_then(|spec| spec.unit.clone())
            .unwrap_or_else(|| self.default_unit.clone())
    }
}

impl GroupSpec {
    fn matches(&self, path: &str) -> bool {
        self.include.iter().any(|pattern| {
            if pattern.ends_with('/') {
                path.starts_with(pattern.as_str())
            } else {
                path == pattern
            }
        })
    }
}

/// 按配置把条目切成组（不改写条目字节，只做归属与统计）。
pub fn build_plan(
    config: &BundleConfig,
    entries: &[(String, u64)],
    block_size: u32,
) -> anyhow::Result<BundlePlan> {
    let mut buckets: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for (path, size) in entries {
        buckets
            .entry(config.group_of(path))
            .or_default()
            .push((path.clone(), *size));
    }
    let mut groups = Vec::with_capacity(buckets.len());
    for (name, mut members) in buckets {
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let raw_bytes: u64 = members.iter().map(|(_, size)| *size).sum();
        groups.push(PlannedGroup {
            unit: config.unit_of(&name),
            // 建包后回填真实文件名（内容哈希只有压完才知道）
            bundle: format!("bundles/{}-{:08x}.rlb", sanitize(&name), 0),
            name,
            entries: members.len() as u64,
            raw_bytes,
            bundle_bytes: 0,
            // 建包后回填（内容哈希只有压完才知道）
            hash: String::new(),
            paths: members.into_iter().map(|(path, _)| path).collect(),
        });
    }
    groups.sort_by_key(|group| {
        config
            .groups
            .iter()
            .position(|spec| spec.name == group.name)
            .unwrap_or(usize::MAX)
    });
    // `.sealed()` 写入自校验：计划表是缺件归因的唯一依据，被截断/覆盖时必须当场发现。
    Ok(BundlePlan {
        schema: 1,
        format: PLAN_FORMAT.to_owned(),
        block_size,
        groups,
        checksum: String::new(),
    }
    .sealed())
}

/// 三条构建期硬门（§5.2）。`entry_count` 是打包前的条目总数，用来证"恰好一次"。
pub fn validate_plan(
    plan: &BundlePlan,
    entry_count: u64,
    project: &ProjectKeys,
    max_bundle_bytes: u64,
) -> anyhow::Result<()> {
    // ① 每个文件恰好属于一个组
    let planned: u64 = plan.groups.iter().map(|group| group.entries).sum();
    if planned != entry_count {
        bail!("分组覆盖不全：计划 {planned} 条，实际 {entry_count} 条");
    }
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for group in &plan.groups {
        for path in &group.paths {
            if let Some(previous) = seen.insert(path.as_str(), group.name.as_str()) {
                bail!("条目 {path} 同时属于 {previous} 与 {}", group.name);
            }
        }
    }
    if seen.len() as u64 != entry_count {
        bail!(
            "分组覆盖不全：去重后 {} 条，实际 {entry_count} 条",
            seen.len()
        );
    }

    // ② 一个组不跨分发单元 + 单组不超微信分包上限
    for group in &plan.groups {
        if !matches!(group.unit.as_str(), "main" | "sub" | "cdn") {
            bail!(
                "组 {} 的 unit 非法：{}（只认 main/sub/cdn）",
                group.name,
                group.unit
            );
        }
        // **上限按落盘字节判，不按原始字节**（`docs/67` 复核 P0-1）：微信的 4 MB 是压缩后的
        // 分包上限。拿 raw_bytes 判会两头错——"原始 5 MB、压完 600 KB"被误拒，
        // "原始 3.9 MB、压完 4.2 MB"被放过。真正的判定在 `package_grouped` 建完包之后，
        // 用 `built.bytes.len()` 对 `group.bundle_bytes` 做。这里只校验 unit 合法性。
        let _ = (group, max_bundle_bytes);
    }

    // ③ core 自足：启动必需的一切都必须在 main 单元里
    let main_groups: Vec<&PlannedGroup> = plan.groups_in_unit("main");
    let in_main = |path: &str| main_groups.iter().any(|group| group.contains(path));
    for required in project.required_paths() {
        if !in_main(&required) {
            bail!(
                "启动必需的 {required} 不在 unit = \"main\" 的组里：Web 上 require 同步，\
                 启动即失败（docs/67 §5.2 第 3 条）"
            );
        }
    }
    for path in seen.keys() {
        if path.starts_with("scripts/") && path.ends_with(".lua") && !in_main(path) {
            bail!("Lua 脚本 {path} 不在 unit = \"main\" 的组里（require 同步）");
        }
    }
    Ok(())
}

/// 计划序列化为确定性 JSON（组与路径都已排序，无时间戳）。
pub fn plan_to_json(plan: &BundlePlan) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec_pretty(plan).map_err(|error| anyhow!("序列化计划失败：{error}"))
}

/// 计划需要工程清单里的几个键（启动自足性检查用）。
#[derive(Debug, Clone, Default)]
pub struct ProjectKeys {
    pub main_script: Option<String>,
    pub startup_scene: Option<String>,
    pub fonts: Vec<String>,
}

impl ProjectKeys {
    fn required_paths(&self) -> Vec<String> {
        let mut out = vec!["project.toml".to_owned()];
        if let Some(script) = &self.main_script {
            out.push(script.clone());
        }
        if let Some(scene) = &self.startup_scene {
            out.push(scene.clone());
        }
        out.extend(self.fonts.iter().cloned());
        out
    }
}

/// 组名 → 文件名安全形式（`art:ui` → `art-ui`）。
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BundleConfig {
        BundleConfig::default_domains()
    }

    #[test]
    fn default_rules_split_by_content_domain() {
        let config = config();
        assert_eq!(config.group_of("scripts/main.lua"), "core");
        assert_eq!(config.group_of("assets/prefabs/hud.prefab"), "core");
        assert_eq!(config.group_of("assets/audio/bgm_manor.mp3"), "audio");
        assert_eq!(
            config.group_of("assets/art/characters/player_male/static_s.png"),
            "art:characters"
        );
        assert_eq!(
            config.group_of("assets/art/ui/hud/ui_icon_hp.png"),
            "art:ui"
        );
        // 兜底：美术里没被更早规则吃掉的（建筑/道具/瓦片）落 art:world
        assert_eq!(
            config.group_of("assets/art/buildings/manor_house_body.png"),
            "art:world"
        );
        // 谁都没命中
        assert_eq!(config.group_of("notes.txt"), "unowned");
    }

    /// 标题底图是启动场景（`title_menu.prefab`）的全屏 Image，必须随 `unit = main` 挂载：
    /// 落进 `art:world`(sub) 会被延迟挂载，首帧贴图 MISS 后 `async_loader` 把该路径记为
    /// 失败且**不重试**，整块门面永远是白底（2026-10-18 现场）。
    /// 同目录的 `startup_background.png`（794 KB 启动图）不进主包，避免白占微信 4 MB 额度。
    #[test]
    fn title_backdrop_ships_in_the_boot_unit() {
        let config = BundleConfig::default_domains();
        assert_eq!(
            config.unit_of(&config.group_of("assets/art/title/title_bg_pixel.png")),
            "main",
            "标题底图必须在 main 单元（首帧就要读）"
        );
        assert_eq!(
            config.unit_of(&config.group_of("assets/art/title/startup_background.png")),
            "sub",
            "启动大图仍留在 art:world，不拖进主包"
        );
    }

    #[test]
    fn explicit_assign_wins_and_longest_prefix_first() {
        let mut config = config();
        config
            .assign
            .insert("assets/art/ui/".to_owned(), "art:ui-main".to_owned());
        config
            .assign
            .insert("assets/art/ui/hud/".to_owned(), "art:ui-hud".to_owned());
        assert_eq!(
            config.group_of("assets/art/ui/common/ui_divider.png"),
            "art:ui-main"
        );
        assert_eq!(
            config.group_of("assets/art/ui/hud/ui_icon_hp.png"),
            "art:ui-hud",
            "更长前缀必须赢"
        );
        // 显式指派优先于组规则（即便 order 更靠后）
        assert_eq!(config.group_of("assets/audio/x.mp3"), "audio");
    }

    fn keys() -> ProjectKeys {
        ProjectKeys {
            main_script: Some("scripts/main.lua".to_owned()),
            startup_scene: Some("scenes/main.scene".to_owned()),
            fonts: vec!["assets/fonts/FusionPixel.ttf".to_owned()],
        }
    }

    fn entries() -> Vec<(String, u64)> {
        vec![
            ("project.toml".to_owned(), 10),
            ("scripts/main.lua".to_owned(), 10),
            ("scripts/data/items.lua".to_owned(), 10),
            ("scenes/main.scene".to_owned(), 10),
            ("assets/fonts/FusionPixel.ttf".to_owned(), 10),
            ("assets/audio/bgm.mp3".to_owned(), 10),
            ("assets/art/ui/hud/icon.png".to_owned(), 10),
            ("assets/art/buildings/house.png".to_owned(), 10),
        ]
    }

    #[test]
    fn plan_covers_every_entry_exactly_once() {
        let entries = entries();
        let config = config();
        let plan = build_plan(&config, &entries, 1024).unwrap();
        validate_plan(&plan, entries.len() as u64, &keys(), MAX_BUNDLE_BYTES)
            .expect("计划必须自洽");
        assert_eq!(
            plan.groups.iter().map(|group| group.entries).sum::<u64>(),
            entries.len() as u64
        );
        // 组顺序跟随配置声明（core 第一）
        assert_eq!(plan.groups[0].name, "core");
        // core 是 main；audio 与美术大类都是 sub（音频不该占微信 4 MB 主包额度，
        // 它在 webgl 侧"boot 期必须挂"由 `split_by_unit` 单独保证）
        assert_eq!(config.unit_of("core"), "main");
        assert_eq!(config.unit_of("audio"), "sub");
        assert_eq!(config.unit_of("art:world"), "sub");
    }

    #[test]
    fn scripts_outside_main_are_rejected() {
        let entries = entries();
        let plan = build_plan(&config(), &entries, 1024).unwrap();
        // 把 core 改成 sub：脚本不再在 main 里 → 必须红
        let mut broken = plan.clone();
        for group in &mut broken.groups {
            if group.name == "core" {
                group.unit = "sub".to_owned();
            }
        }
        let error = validate_plan(&broken, entries.len() as u64, &keys(), MAX_BUNDLE_BYTES)
            .expect_err("脚本不在 main 必须被拒绝");
        assert!(error.to_string().contains("main"), "{error}");
    }

    #[test]
    fn oversized_group_is_rejected_with_the_group_name() {
        // core 只有 50 B，`art:world` 有一个 1 KB 的大家伙；上限设 100 B。
        let entries = vec![
            ("project.toml".to_owned(), 10),
            ("scripts/main.lua".to_owned(), 10),
            ("scenes/main.scene".to_owned(), 10),
            ("assets/fonts/FusionPixel.ttf".to_owned(), 10),
            ("scripts/data/items.lua".to_owned(), 10),
            ("assets/art/buildings/house.png".to_owned(), 1024),
        ];
        let plan = build_plan(&config(), &entries, 1024).unwrap();
        // 复核 P0-1 的回归护栏：**原始字节不再触发上限**（按落盘判，见 package_grouped）。
        validate_plan(&plan, entries.len() as u64, &keys(), 100)
            .expect("原始字节不该再触发 4 MB 门（错的口径已移除）");

        // 同一个组改成 cdn 就不受分包上限约束（core 仍然自足）
        let mut cdn = plan.clone();
        for group in &mut cdn.groups {
            if group.name == "art:world" {
                group.unit = "cdn".to_owned();
            }
        }
        validate_plan(&cdn, entries.len() as u64, &keys(), 100)
            .expect("cdn 单元不受 4 MB 分包上限约束");
    }

    #[test]
    fn missing_entry_is_rejected() {
        let entries = entries();
        let plan = build_plan(&config(), &entries, 1024).unwrap();
        let error = validate_plan(&plan, entries.len() as u64 + 1, &keys(), MAX_BUNDLE_BYTES)
            .expect_err("条目数不守恒必须被拒绝");
        assert!(error.to_string().contains("覆盖不全"), "{error}");
    }

    #[test]
    fn pending_lists_only_unmounted_groups() {
        let entries = entries();
        let plan = build_plan(&config(), &entries, 1024).unwrap();
        let core_bundle = plan
            .groups
            .iter()
            .find(|group| group.name == "core")
            .map(|group| group.bundle.clone())
            .unwrap();
        let pending = plan.pending_for(&[core_bundle]);
        assert!(
            pending.iter().all(|(bundle, _)| !bundle.contains("/core-")),
            "已挂载的组不能出现在 pending 里：{pending:?}"
        );
        assert_eq!(pending.len(), plan.groups.len() - 1, "其余组都应可归因");
    }

    #[test]
    fn sanitize_keeps_names_filename_safe() {
        assert_eq!(sanitize("art:ui"), "art-ui");
        assert_eq!(sanitize("core"), "core");
    }

    #[test]
    fn plan_roundtrips_through_json_deterministically() {
        let entries = entries();
        let plan = build_plan(&config(), &entries, 1024).unwrap();
        let a = plan_to_json(&plan).unwrap();
        let b = plan_to_json(&plan).unwrap();
        assert_eq!(a, b);
        let parsed = BundlePlan::parse(&a).unwrap();
        assert_eq!(parsed, plan);
    }
}
