//! 分包配置与归约（`docs/67` §5）：`bundles.json` → 分组 → 计划表。
//!
//! 放在 `rustlua-infra` 而不是 `assetc`，与 [`crate::bundle::plan`] 同一理由：
//! **构建工具与编辑器面板必须用同一份规则**。`game` 不能依赖 `assetc`（构建工具不进运行器），
//! 而"预览一套、打包另一套"是这类管线最常见的漂移（`docs/67` §8 第 4 项）。
//!
//! 三条归组来源（先匹配先赢）：
//! 1. `assign` 的显式路径/前缀（最长前缀优先）；
//! 2. `groups[].include`（按声明顺序）；
//! 3. `default_group`（缺省 `unowned`）。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::bundle::plan::{BundlePlan, PLAN_FORMAT, PlannedGroup};

/// 单个 bundle 的落盘上限：微信主包/普通分包口径（`docs/52`）。
pub const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;
/// 微信小游戏"主包 + 全部分包"的总量上限。
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// 分发单元的解析结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind<'a> {
    /// 启动即挂（主包 / 首屏）。
    Main,
    /// 分包；`Some(name)` 是具名分包（`sub:art`），`None` 是默认分包（`sub`）。
    Sub(Option<&'a str>),
    /// 不参与包内配额（CDN / 发布器另行放置）。
    Cdn,
}

/// 解析 `unit` 字段。语法错了就报错——不猜、不静默当成 `sub`。
pub fn parse_unit(unit: &str) -> Result<UnitKind<'_>, String> {
    match unit {
        "main" => Ok(UnitKind::Main),
        "cdn" => Ok(UnitKind::Cdn),
        "sub" => Ok(UnitKind::Sub(None)),
        other => match other.strip_prefix("sub:") {
            Some(name) if valid_sub_name(name) => Ok(UnitKind::Sub(Some(name))),
            Some(name) => Err(format!(
                "分包名 {name:?} 非法：只允许字母/数字/`-`/`_`，且不能为空（unit={other}）"
            )),
            None => Err(format!(
                "unit 只认 main | sub | sub:<name> | cdn，收到 {other:?}"
            )),
        },
    }
}

fn valid_sub_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// 是否启动单元（`main`）。
#[must_use]
pub fn is_main_unit(unit: &str) -> bool {
    matches!(parse_unit(unit), Ok(UnitKind::Main))
}

/// 具名分包名（`sub:art` → `Some("art")`；`sub` → `None`）。
#[must_use]
pub fn sub_name(unit: &str) -> Option<&str> {
    match parse_unit(unit) {
        Ok(UnitKind::Sub(name)) => name,
        _ => None,
    }
}

/// `bundles.json` 的根结构。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BundleConfig {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// 没被任何规则命中的分组。
    #[serde(default = "default_group")]
    pub default_group: String,
    /// 组的默认分发单元：`main` | `sub` | `sub:<name>` | `cdn`。
    #[serde(default = "default_unit")]
    pub default_unit: String,
    /// 允许 `default_group` 非空（发布期默认**拒绝**：未归组 = 首屏可能白图）。
    #[serde(default)]
    pub allow_unowned: bool,
    /// 分组声明（**顺序即优先级**）。
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
    /// 显式逐路径指派：`路径或前缀 → 组名`。以 `/` 结尾按前缀匹配。
    #[serde(default)]
    pub assign: BTreeMap<String, String>,
}

impl Default for BundleConfig {
    fn default() -> Self {
        // 引擎级默认：只依赖通用目录约定，不绑定任何具体游戏。
        Self::generic_domains()
    }
}

/// 一个组的声明。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GroupSpec {
    pub name: String,
    #[serde(default)]
    pub unit: Option<String>,
    /// 匹配项：`assets/art/ui` 或 `assets/art/ui/`（后者按前缀）或 `project.toml`（精确）。
    #[serde(default)]
    pub include: Vec<String>,
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

impl BundleConfig {
    /// 配置文件在工程根下的固定名字。
    pub const FILE: &'static str = "bundles.json";

    /// 解析 `bundles.json`；文件不存在时按 `docs/67` §5.3 的内容域默认规则生成。
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.is_file() {
            let text = std::fs::read_to_string(path)
                .map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
            let config: Self = serde_json::from_str(&text)
                .map_err(|error| anyhow!("解析 {} 失败：{error}", path.display()))?;
            if config.schema != default_schema() {
                bail!("{}: 不支持的 schema {}", path.display(), config.schema);
            }
            config.validate()?;
            return Ok(config);
        }
        Ok(Self::generic_domains())
    }

    /// 确定性 JSON（编辑器保存用）。
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
            .map_err(|error| anyhow!("序列化 bundles.json 失败：{error}"))
    }

    /// 语义校验：单元语法、组名唯一、include 形状、assign 指向存在的组。
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.default_group.trim().is_empty() {
            bail!("default_group 不能为空");
        }
        parse_unit(&self.default_unit).map_err(anyhow::Error::msg)?;
        let mut names = std::collections::BTreeSet::new();
        for group in &self.groups {
            if group.name.trim().is_empty() {
                bail!("组名不能为空");
            }
            if !names.insert(group.name.as_str()) {
                bail!("组名重复：{}", group.name);
            }
            if group.include.is_empty() {
                bail!("组 {} 没有 include", group.name);
            }
            if let Some(unit) = &group.unit {
                parse_unit(unit).map_err(anyhow::Error::msg)?;
            }
        }
        for (pattern, group) in &self.assign {
            if pattern.trim().is_empty() {
                bail!("assign 里出现空路径");
            }
            if group != &self.default_group && !names.contains(group.as_str()) {
                bail!(
                    "assign 指向未声明的组 {group}（路径 {pattern}）：先声明组，或改到 default_group"
                );
            }
        }
        Ok(())
    }

    /// **引擎级通用默认**：只依赖本引擎约定的目录（`project.toml` / `scripts/` / `scenes/` /
    /// `assets/` / `splash/` / `branding/`），不含任何具体游戏的路径。
    ///
    /// 任何 RustLua 工程不写 `bundles.json` 都能用这一份；有更细的内容域需求时，
    /// 由工程自己的 `bundles.json` 声明（`examples/game_jhyt` 就是那种工程级配置）。
    #[must_use]
    pub fn generic_domains() -> Self {
        let group = |name: &str, unit: &str, include: &[&str]| GroupSpec {
            name: name.to_owned(),
            unit: Some(unit.to_owned()),
            include: include.iter().map(|item| (*item).to_owned()).collect(),
        };
        Self {
            schema: default_schema(),
            default_group: default_group(),
            default_unit: default_unit(),
            allow_unowned: false,
            groups: vec![
                // 启动必需：清单、脚本、场景、预制体、字体、图集重定向表。
                group(
                    "core",
                    "main",
                    &[
                        "project.toml",
                        "chars.txt",
                        "dependencies.json",
                        "bundles.json",
                        "atlas_redirect.json",
                        "scripts/",
                        "scenes/",
                        "assets/prefabs/",
                        "assets/fonts/",
                    ],
                ),
                // 其余资源进默认分包；需要更细的加载边界时由工程 bundles.json 覆盖。
                group("assets", "sub", &["assets/"]),
                // 启动器品牌图不占游戏资源配额（发布器会另行放置）。
                group("brand", "cdn", &["splash/", "branding/"]),
            ],
            assign: BTreeMap::new(),
        }
    }

    /// 示例工程的内容域预设（`docs/67` §5.3 的实测分组表）。
    ///
    /// **不是**引擎默认：`art:ui` / `title_bg_pixel.png` 这类路径是具体游戏的约定，
    /// 引擎层只保留 [`Self::generic_domains`]；本函数供示例/测试复现实测分组。
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
            allow_unowned: false,
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
                        "assets/atlases/",
                        "assets/art/ui/",
                        "assets/art/items/",
                        "assets/art/portraits/",
                        "assets/art/title/title_bg_pixel.png",
                    ],
                ),
                // `unit` 的语义是"启动即需要"，不是"体积小"：BGM 在 boot 阶段就 play()。
                group("audio", "sub", &["assets/audio/"]),
                group("art:characters", "sub", &["assets/art/characters/"]),
                group("art:enemies", "sub", &["assets/art/enemies/"]),
                group("art:world", "sub", &["assets/"]),
                // 启动器品牌图不进游戏资源配额。
                group("brand", "cdn", &["splash/", "branding/"]),
            ],
            assign: BTreeMap::new(),
        }
    }

    /// 解析一个条目属于哪个组（三条来源的优先级在这里落地）。
    #[must_use]
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

    /// 组的单元（`main` / `sub` / `sub:<name>` / `cdn`）。
    #[must_use]
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

/// 计划需要工程清单里的几个键（启动自足性检查用）。
#[derive(Debug, Clone, Default)]
pub struct ProjectKeys {
    pub main_script: Option<String>,
    pub startup_scene: Option<String>,
    pub fonts: Vec<String>,
}

impl ProjectKeys {
    /// 从工程根的 `project.toml` 读取（与 `assetc` 同一套逐行 key=value 口径）。
    #[must_use]
    pub fn from_project(root: &Path) -> Self {
        let text = std::fs::read_to_string(root.join("project.toml")).unwrap_or_default();
        let mut keys = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim_matches('\'');
            match key.trim() {
                "main_script" => keys.main_script = Some(value.to_owned()),
                "startup_scene" => keys.startup_scene = Some(value.to_owned()),
                "font" | "font_fallback" if !value.is_empty() => {
                    keys.fonts.push(value.to_owned());
                }
                _ => {}
            }
        }
        keys
    }

    /// 启动必需项（`project.toml` + 主脚本 + 启动场景 + 字体）。
    #[must_use]
    pub fn required_paths(&self) -> Vec<String> {
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

/// 按配置把条目切成组（不改写条目字节，只做归属与统计）。
pub fn build_plan(
    config: &BundleConfig,
    entries: &[(String, u64)],
    block_size: u32,
) -> anyhow::Result<BundlePlan> {
    config.validate()?;
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
            bundle: format!("bundles/{}-{:08x}.rlb", sanitize(&name), 0),
            name,
            entries: members.len() as u64,
            raw_bytes,
            bundle_bytes: 0,
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
    Ok(BundlePlan {
        schema: 1,
        format: PLAN_FORMAT.to_owned(),
        block_size,
        groups,
        checksum: String::new(),
    }
    .sealed())
}

/// 一个分发根（微信分包/主包的目录边界）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistributionRoot {
    /// 单元名（`main` / `sub` / `sub:<name>`）。
    pub unit: String,
    /// 包内根目录：`pack`（主包首屏）、`pack-sub`（默认分包）、`pack-<name>`（具名分包）。
    pub root: String,
    /// 这个根里的容器（包内相对路径，升序）。
    pub bundles: Vec<String>,
    pub entries: u64,
    pub bytes: u64,
}

/// 把计划表折算成**分发布局**：每个单元一个根，`cdn` 不进包。
///
/// 这是发布器（`first-screen-pack.js` / `package.ps1`）与适配层之间的唯一契约：
/// - `main` → `pack`（随主包上传，启动即可读）；
/// - `sub` → `pack-sub`（微信默认分包根）；
/// - `sub:<name>` → `pack-<name>`（具名分包根，每根需要自己的 `game.js`）；
/// - `cdn` → 不出现在布局里（发布器另行放置）。
///
/// 校验两条硬门：主包 ≤ 4 MB、主包 + 全部分包 ≤ 20 MB（微信口径，`docs/52`）。
pub fn distribution_layout(plan: &BundlePlan) -> anyhow::Result<Vec<DistributionRoot>> {
    let mut roots: BTreeMap<(u8, String), DistributionRoot> = BTreeMap::new();
    for group in &plan.groups {
        let (rank, unit, root) = match parse_unit(&group.unit).map_err(anyhow::Error::msg)? {
            UnitKind::Main => (0u8, "main".to_owned(), "pack".to_owned()),
            UnitKind::Sub(None) => (1, "sub".to_owned(), "pack-sub".to_owned()),
            UnitKind::Sub(Some(name)) => (2, group.unit.clone(), format!("pack-{name}")),
            UnitKind::Cdn => continue,
        };
        let entry = roots
            .entry((rank, unit.clone()))
            .or_insert_with(|| DistributionRoot {
                unit,
                root,
                bundles: Vec::new(),
                entries: 0,
                bytes: 0,
            });
        entry.bundles.push(group.bundle.clone());
        entry.entries += group.entries;
        entry.bytes += group.bundle_bytes;
    }
    let mut out: Vec<DistributionRoot> = roots.into_values().collect();
    for root in &mut out {
        root.bundles.sort();
    }
    if let Some(main) = out.iter().find(|root| root.unit == "main")
        && main.bytes > MAX_BUNDLE_BYTES
    {
        anyhow::bail!(
            "主包（pack）落盘 {} KB 超过 4,096 KB：把非启动资源挪到分包",
            main.bytes / 1024
        );
    }
    let total: u64 = out.iter().map(|root| root.bytes).sum();
    if total > MAX_TOTAL_BYTES {
        anyhow::bail!(
            "主包 + 全部分包落盘 {} KB 超过总量上限 20,480 KB",
            total / 1024
        );
    }
    Ok(out)
}

/// 计划表自检：返回**全部**问题（编辑器面板要一次列全，而不是只报第一条）。
#[must_use]
pub fn check_plan(
    plan: &BundlePlan,
    entry_count: u64,
    project: &ProjectKeys,
    config: &BundleConfig,
    max_bundle_bytes: u64,
) -> Vec<String> {
    let mut issues = Vec::new();

    // ① 每个文件恰好属于一个组
    let planned: u64 = plan.groups.iter().map(|group| group.entries).sum();
    if planned != entry_count {
        issues.push(format!(
            "分组覆盖不全：计划 {planned} 条，实际 {entry_count} 条"
        ));
    }
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for group in &plan.groups {
        for path in &group.paths {
            if let Some(previous) = seen.insert(path.as_str(), group.name.as_str()) {
                issues.push(format!("条目 {path} 同时属于 {previous} 与 {}", group.name));
            }
        }
    }
    if seen.len() as u64 != entry_count {
        issues.push(format!(
            "分组覆盖不全：去重后 {} 条，实际 {entry_count} 条",
            seen.len()
        ));
    }

    // ② 单元语法 + 单组落盘上限（`cdn` 不参与微信配额）
    for group in &plan.groups {
        if let Err(error) = parse_unit(&group.unit) {
            issues.push(format!("组 {} 的 unit 非法：{error}", group.name));
        }
        if group.bundle_bytes > max_bundle_bytes
            && !matches!(parse_unit(&group.unit), Ok(UnitKind::Cdn))
        {
            issues.push(format!(
                "组 {} 落盘 {} KB 超过单包上限 {} KB",
                group.name,
                group.bundle_bytes / 1024,
                max_bundle_bytes / 1024
            ));
        }
    }

    // ③ 未归组：发布期默认拒绝（首屏读到 = 白图，且 async_loader 不重试）
    if !config.allow_unowned
        && let Some(group) = plan.group(&config.default_group)
        && group.entries > 0
    {
        issues.push(format!(
            "有 {} 个文件没被任何规则命中（组 {}）：发布期默认拒绝。给它们补 include/assign，\
             或显式打开 allow_unowned（不推荐）",
            group.entries, group.name
        ));
    }

    // ④ core 自足：启动必需的一切都必须在 main 单元里
    let main_groups: Vec<&PlannedGroup> = plan
        .groups
        .iter()
        .filter(|group| is_main_unit(&group.unit))
        .collect();
    let in_main = |path: &str| main_groups.iter().any(|group| group.contains(path));
    for required in project.required_paths() {
        if !in_main(&required) {
            issues.push(format!(
                "启动必需的 {required} 不在 unit = \"main\" 的组里：Web 上 require 同步，启动即失败"
            ));
        }
    }
    for path in seen.keys() {
        if path.starts_with("scripts/") && path.ends_with(".lua") && !in_main(path) {
            issues.push(format!(
                "Lua 脚本 {path} 不在 unit = \"main\" 的组里（require 同步）"
            ));
        }
    }

    // ⑤ 具名分包在当前发布器里会被并进同一个分包：明确说出来，别让人以为已经分开了
    let named: Vec<&str> = plan
        .groups
        .iter()
        .filter_map(|group| sub_name(&group.unit).map(|_| group.name.as_str()))
        .collect();
    if !named.is_empty() {
        issues.push(format!(
            "具名分包（{}）已进入计划表，但当前微信发布器只生成一个分包根：\
             它们会被合并进同一个 subpackage。真正的分包拆分见 docs/67 §9 待办",
            named.join(", ")
        ));
    }

    issues
}

/// 计划校验：有任何问题就报第一条（发布期用；编辑器用 [`check_plan`] 列全）。
pub fn validate_plan(
    plan: &BundlePlan,
    entry_count: u64,
    project: &ProjectKeys,
    config: &BundleConfig,
    max_bundle_bytes: u64,
) -> anyhow::Result<()> {
    match check_plan(plan, entry_count, project, config, max_bundle_bytes).first() {
        Some(issue) => Err(anyhow!("{issue}")),
        None => Ok(()),
    }
}

/// 计划序列化为确定性 JSON（组与路径都已排序，无时间戳）。
pub fn plan_to_json(plan: &BundlePlan) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec_pretty(plan).map_err(|error| anyhow!("序列化计划失败：{error}"))
}

/// 组名 → 文件名安全形式（`art:ui` → `art-ui`）。
#[must_use]
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

/// 扫描工程里"会被发布"的文件（**未 cook 预估**：不含图集页、字体子集、生成清单）。
///
/// 与 `assetc::package` 的输入集合保持一致：`project.toml` + `scenes/` + `scripts/` +
/// `assets/`，外加发布器声明的 `splash/` / `branding/`。以 `_` 开头的文件是生成物
/// （`assetc` 也按同一约定排除），不计入。
pub fn scan_project_entries(root: &Path) -> anyhow::Result<Vec<(String, u64)>> {
    let mut found = Vec::new();
    let root = root.to_path_buf();
    let top_file = |name: &str, out: &mut Vec<(String, u64)>| {
        let path = root.join(name);
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            out.push((name.to_owned(), meta.len()));
        }
    };
    top_file("project.toml", &mut found);
    top_file("settings.toml", &mut found);
    for dir in ["scenes", "scripts", "assets", "splash", "branding"] {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        let mut stack: Vec<PathBuf> = vec![base];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    if matches!(
                        name.as_ref(),
                        "builds" | ".cache" | ".data" | ".temp" | "target"
                    ) {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                if name.starts_with('_') {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|_| anyhow!("{} 不在工程根内", path.display()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, meta.len()));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = BundleConfig::default_domains();
        let plan = build_plan(&config, &entries, 1024).unwrap();
        validate_plan(
            &plan,
            entries.len() as u64,
            &keys(),
            &config,
            MAX_BUNDLE_BYTES,
        )
        .expect("计划必须自洽");
        assert_eq!(
            plan.groups.iter().map(|group| group.entries).sum::<u64>(),
            entries.len() as u64
        );
        assert_eq!(plan.groups[0].name, "core");
        assert_eq!(config.unit_of("core"), "main");
        assert_eq!(config.unit_of("audio"), "sub");
        assert_eq!(config.unit_of("art:world"), "sub");
    }

    #[test]
    fn scripts_outside_main_are_rejected() {
        let entries = entries();
        let config = BundleConfig::default_domains();
        let mut plan = build_plan(&config, &entries, 1024).unwrap();
        for group in &mut plan.groups {
            if group.name == "core" {
                group.unit = "sub".to_owned();
            }
        }
        let issues = check_plan(
            &plan,
            entries.len() as u64,
            &keys(),
            &config,
            MAX_BUNDLE_BYTES,
        );
        assert!(
            issues.iter().any(|issue| issue.contains("main")),
            "{issues:?}"
        );
    }

    #[test]
    fn unit_syntax_is_parsed_strictly() {
        assert_eq!(parse_unit("main"), Ok(UnitKind::Main));
        assert_eq!(parse_unit("sub"), Ok(UnitKind::Sub(None)));
        assert_eq!(parse_unit("sub:art"), Ok(UnitKind::Sub(Some("art"))));
        assert_eq!(parse_unit("cdn"), Ok(UnitKind::Cdn));
        assert!(parse_unit("sub:").is_err());
        assert!(parse_unit("sub:a/b").is_err());
        assert!(parse_unit("bundle").is_err());
        assert!(!is_main_unit("sub:art"));
        assert_eq!(sub_name("sub:art"), Some("art"));
    }

    #[test]
    fn named_sub_units_go_into_the_plan_and_are_reported_as_not_split_yet() {
        let entries = entries();
        let mut config = BundleConfig::default_domains();
        for group in &mut config.groups {
            if group.name == "audio" {
                group.unit = Some("sub:audio".to_owned());
            }
        }
        let plan = build_plan(&config, &entries, 1024).unwrap();
        let audio = plan.group("audio").expect("audio 组");
        assert_eq!(audio.unit, "sub:audio");
        let issues = check_plan(
            &plan,
            entries.len() as u64,
            &keys(),
            &config,
            MAX_BUNDLE_BYTES,
        );
        assert!(
            issues.iter().any(|issue| issue.contains("具名分包")),
            "必须提示当前发布器未拆分：{issues:?}"
        );
    }

    #[test]
    fn unowned_is_rejected_until_explicitly_allowed() {
        let entries = vec![
            ("project.toml".to_owned(), 10),
            ("scripts/main.lua".to_owned(), 10),
            ("scenes/main.scene".to_owned(), 10),
            ("assets/fonts/FusionPixel.ttf".to_owned(), 10),
            ("notes/readme.md".to_owned(), 10),
        ];
        let config = BundleConfig::default_domains();
        let plan = build_plan(&config, &entries, 1024).unwrap();
        let issues = check_plan(
            &plan,
            entries.len() as u64,
            &keys(),
            &config,
            MAX_BUNDLE_BYTES,
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("没被任何规则命中")),
            "{issues:?}"
        );

        let mut permissive = BundleConfig::default_domains();
        permissive.allow_unowned = true;
        let issues = check_plan(
            &plan,
            entries.len() as u64,
            &keys(),
            &permissive,
            MAX_BUNDLE_BYTES,
        );
        assert!(
            !issues
                .iter()
                .any(|issue| issue.contains("没被任何规则命中"))
        );
    }

    #[test]
    fn assign_wins_and_longest_prefix_first() {
        let mut config = BundleConfig::default_domains();
        config
            .assign
            .insert("assets/art/ui/".to_owned(), "art:ui-main".to_owned());
        config
            .assign
            .insert("assets/art/ui/hud/".to_owned(), "art:ui-hud".to_owned());
        config.groups.push(GroupSpec {
            name: "art:ui-main".to_owned(),
            unit: Some("main".to_owned()),
            include: vec!["assets/art/ui/".to_owned()],
        });
        config.groups.push(GroupSpec {
            name: "art:ui-hud".to_owned(),
            unit: Some("main".to_owned()),
            include: vec!["assets/art/ui/hud/".to_owned()],
        });
        config.validate().unwrap();
        assert_eq!(config.group_of("assets/art/ui/common/x.png"), "art:ui-main");
        assert_eq!(config.group_of("assets/art/ui/hud/x.png"), "art:ui-hud");
    }

    #[test]
    fn assign_to_unknown_group_is_rejected() {
        let mut config = BundleConfig::default_domains();
        config
            .assign
            .insert("assets/x.png".to_owned(), "typo-group".to_owned());
        let error = config
            .validate()
            .expect_err("assign 指向未声明的组必须被拒绝");
        assert!(error.to_string().contains("未声明的组"), "{error}");
    }

    #[test]
    fn plan_roundtrips_through_json_deterministically() {
        let entries = entries();
        let config = BundleConfig::default_domains();
        let plan = build_plan(&config, &entries, 1024).unwrap();
        let a = plan_to_json(&plan).unwrap();
        let b = plan_to_json(&plan).unwrap();
        assert_eq!(a, b);
        assert_eq!(BundlePlan::parse(&a).unwrap(), plan);
    }

    #[test]
    fn scan_project_entries_mirrors_package_inputs() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-bundle-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/art/ui")).unwrap();
        std::fs::create_dir_all(root.join("assets/art/ui/builds")).unwrap();
        std::fs::write(root.join("project.toml"), "name = \"t\"\n").unwrap();
        std::fs::write(root.join("scripts/main.lua"), "return 1\n").unwrap();
        std::fs::write(root.join("scenes/main.scene"), "scene_version=2\n").unwrap();
        std::fs::write(root.join("assets/art/ui/a.png"), b"png").unwrap();
        std::fs::write(root.join("assets/art/ui/_preview.png"), b"png").unwrap();
        std::fs::write(root.join("assets/art/ui/builds/x.png"), b"png").unwrap();

        let entries = scan_project_entries(&root).unwrap();
        let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"project.toml"));
        assert!(paths.contains(&"assets/art/ui/a.png"));
        assert!(!paths.iter().any(|path| path.contains("_preview")));
        assert!(!paths.iter().any(|path| path.contains("/builds/")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 引擎级默认只依赖通用目录约定：任何 RustLua 工程不写 `bundles.json` 都能用。
    #[test]
    fn generic_domains_depend_only_on_engine_conventions() {
        let config = BundleConfig::generic_domains();
        config.validate().unwrap();
        assert_eq!(config.group_of("scripts/main.lua"), "core");
        assert_eq!(config.group_of("assets/art/ui/hud/a.png"), "assets");
        assert_eq!(config.group_of("splash/splash.png"), "brand");
        let joined = serde_json::to_string(&config).unwrap();
        assert!(!joined.contains("art/ui"), "{joined}");
        assert!(!joined.contains("title_bg_pixel"), "{joined}");
        // `Default`（无配置文件时的兜底）必须是通用规则，而不是示例工程的预设。
        assert_eq!(BundleConfig::default().groups, config.groups);
    }

    fn layout_plan() -> BundlePlan {
        let mut plan = build_plan(
            &BundleConfig::default_domains(),
            &[
                ("project.toml".to_owned(), 10),
                ("scripts/main.lua".to_owned(), 10),
                ("scenes/main.scene".to_owned(), 10),
                ("assets/audio/bgm.mp3".to_owned(), 100),
                ("assets/art/ui/a.png".to_owned(), 100),
                ("assets/art/buildings/b.png".to_owned(), 100),
                ("splash/splash.png".to_owned(), 100),
            ],
            1024,
        )
        .unwrap();
        for group in &mut plan.groups {
            group.bundle_bytes = group.raw_bytes;
        }
        plan
    }

    #[test]
    fn distribution_layout_groups_units_into_roots() {
        let mut plan = layout_plan();
        // 让 audio 成为具名分包，验证第三种根。
        for group in &mut plan.groups {
            if group.name == "audio" {
                group.unit = "sub:audio".to_owned();
            }
        }
        let roots = distribution_layout(&plan).unwrap();
        let names: Vec<&str> = roots.iter().map(|root| root.root.as_str()).collect();
        assert!(names.contains(&"pack"));
        assert!(names.contains(&"pack-sub"));
        assert!(names.contains(&"pack-audio"));
        assert!(
            !names.iter().any(|root| root.contains("brand")),
            "cdn 不进包"
        );
        let main = roots.iter().find(|root| root.unit == "main").unwrap();
        assert!(main.bytes > 0);
        assert!(
            main.bundles
                .iter()
                .all(|bundle| bundle.contains("core") || bundle.contains("art-ui"))
        );
        let audio = roots.iter().find(|root| root.unit == "sub:audio").unwrap();
        assert_eq!(audio.root, "pack-audio");
        assert_eq!(audio.entries, 1);
    }

    #[test]
    fn distribution_layout_rejects_oversized_main() {
        let mut plan = layout_plan();
        for group in &mut plan.groups {
            group.bundle_bytes = MAX_BUNDLE_BYTES + 1;
        }
        let error = distribution_layout(&plan).expect_err("主包超限必须被拒绝");
        assert!(error.to_string().contains("主包"), "{error}");
    }
}
