//! 图集配置与"配置 → 具体打包任务"的归约（`docs/67` §6.2.1、`docs/63`）。
//!
//! 这一层放在 `rustlua-assets` 而不是 `assetc`，理由与 `atlas_redirect` 相同：
//! **构建工具与编辑器面板必须用同一份规则**。两边各写一份归约逻辑，迟早会
//! 出现"编辑器预览打出来的页，和发布包里打出来的不是同一张"（`docs/67` §8 第 4 项
//! 记的"两套提取器必然漂移"就是这个病）。
//!
//! 本模块只做三件事：
//! 1. 解析/保存 `atlas.json`；
//! 2. 把配置归约成"图集名 + 具体源文件"的任务清单（目录聚合、显式声明、排除规则）；
//! 3. 按任务装箱（shelf），返回页与区域位置。
//!
//! **不碰磁盘上的产物**：写 `.rltex`/`.atlas`、删源图、写重定向表都在 `assetc::atlas_cook`；
//! 编辑器只调用这里的预览路径，绝不删源图。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::atlas_pack::{AtlasPackerOptions, pack_atlas, source_dimensions};
pub use crate::atlas_pack::{PackSource, PackedPage};

/// 图集页的输出目录（相对工程根）。构建产物一律落这里，不进 `assets/art/**`。
pub const ATLAS_DIR: &str = "assets/atlases";

/// 瓦片集目录：`Tilemap` 的 UV 按整张纹理算，换页会让地形整体错位，
/// 因此该目录下的图**永远不参与**图集（`docs/67` §6.2 的三条边界之一）。
///
/// 用目录规则兜底（而不是只靠场景里的 `Tilemap` 引用）：四季地表这类图是
/// Lua 用 `string.format` 拼出来的，静态扫描只能看见"模式"，看不见具体文件。
pub const TILESET_DIR: &str = "assets/art/tilesets/";

/// 页的编码形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasFormat {
    /// 裸像素（`docs/67` §6.3）：压缩交给容器块。
    Rltex,
    /// 常规 PNG（开发形态/美术手工图集，或稠密图更小的场合）。
    Png,
    /// 逐页比较两种编码的落盘字节，取小者（`docs/67` P1-1）。
    Auto,
}

impl AtlasFormat {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text {
            "rltex" => Ok(Self::Rltex),
            "png" => Ok(Self::Png),
            "auto" => Ok(Self::Auto),
            other => bail!("图集 format 只认 rltex|png|auto，收到 {other}"),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rltex => "rltex",
            Self::Png => "png",
            Self::Auto => "auto",
        }
    }
}

/// 单张源图放不进一页时怎么办。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oversize {
    /// 直接报错（显式配置的默认值：作者点名要打的图打不进去，必须让人看见）。
    Error,
    /// 跳过并告警，源图原样保留。
    Skip,
}

impl Oversize {
    fn parse(text: Option<&str>) -> anyhow::Result<Self> {
        match text {
            None | Some("error") => Ok(Self::Error),
            Some("skip") => Ok(Self::Skip),
            Some(other) => bail!("oversize 只认 error|skip，收到 {other}"),
        }
    }
}

/// `atlas.json` 根结构。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AtlasConfig {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// 显式图集声明（老写法，优先级高于 `auto`）。
    #[serde(default)]
    pub atlases: Vec<AtlasSpec>,
    /// 目录默认聚合（推荐用法）。
    #[serde(default)]
    pub auto: Option<AutoAtlas>,
    /// 显式"不并入目录默认图集"的原图路径（编辑器属性面板勾选后写到这里）。
    #[serde(default)]
    pub separate: Vec<String>,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            atlases: Vec::new(),
            auto: None,
            separate: Vec::new(),
        }
    }
}

/// 一个显式图集的声明。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AtlasSpec {
    /// 图集名（决定文件名 `<name>-<page>.<ext>`）。
    pub name: String,
    /// 参与打包的路径前缀（相对工程根，以 `/` 结尾）。
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default = "default_max_size")]
    pub max_size: u32,
    #[serde(default = "default_padding")]
    pub padding: u32,
    /// 页尺寸向上取整到 2 的幂（默认关：POT 与 padding 耦合会把页面积抬到理论下界的两倍）。
    #[serde(default)]
    pub pot: bool,
    /// `rltex` | `png` | `auto`。
    ///
    /// **默认 `rltex`，拼写带 `l`**：这里曾经写成 `"rtex"`，导致"省略 format 的
    /// 显式图集"直接构建失败（`docs/67` 复核 P0-2）。取值由 [`AtlasFormat::parse`]
    /// 统一校验，不再等到写页时才炸。
    #[serde(default = "default_format")]
    pub format: String,
    /// 单张源图超过 `max_size` 时怎么办：`error`（默认）/ `skip`（跳过并告警）。
    #[serde(default)]
    pub oversize: Option<String>,
}

/// 目录聚合策略。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AutoAtlas {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 从哪个目录往下聚合（相对工程根）。
    #[serde(default = "default_auto_root")]
    pub root: String,
    #[serde(default = "default_auto_max_size")]
    pub max_size: u32,
    #[serde(default = "default_auto_padding")]
    pub padding: u32,
    /// `rltex` | `png` | `auto`；目录聚合默认 `auto`（逐页取小，避免"稠密图换编码反而变大"）。
    #[serde(default = "default_auto_format")]
    pub format: String,
    /// 超过这个**像素面积**的图不并入默认图集，单独成集（"大图成为单图集"）。
    #[serde(default = "default_single_min_pixels")]
    pub single_min_pixels: u32,
}

impl AutoAtlas {
    /// 编辑器"启用目录聚合"时的初值：`assets/art/ui`，1024 上限，padding 1。
    #[must_use]
    pub fn for_ui_root() -> Self {
        Self {
            enabled: true,
            root: "assets/art/ui".to_owned(),
            max_size: 1024,
            padding: 1,
            format: default_auto_format(),
            single_min_pixels: default_single_min_pixels(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_auto_root() -> String {
    // 默认仍是 `assets`（与 docs/67 一致），但排除规则会把瓦片集/动画表/九宫格/
    // Lua 拼出来的图全部挡在外面，所以"打开就炸"的那几条已经被堵住。
    "assets".to_owned()
}
fn default_auto_max_size() -> u32 {
    1024
}
fn default_auto_padding() -> u32 {
    1
}
fn default_single_min_pixels() -> u32 {
    256 * 256
}
fn default_schema() -> u32 {
    1
}
fn default_max_size() -> u32 {
    2048
}
fn default_padding() -> u32 {
    2
}
fn default_format() -> String {
    "rltex".to_owned()
}
fn default_auto_format() -> String {
    "auto".to_owned()
}

impl AtlasConfig {
    /// 配置文件在工程根下的固定名字。
    pub const FILE: &'static str = "atlas.json";

    /// 解析 `atlas.json`；不存在返回 `Ok(None)`（= 不做 cook）。
    pub fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
        let config: Self = serde_json::from_str(&text)
            .map_err(|error| anyhow!("解析 {} 失败：{error}", path.display()))?;
        if config.schema != default_schema() {
            bail!("{}: 不支持的 schema {}", path.display(), config.schema);
        }
        config.validate()?;
        Ok(Some(config))
    }

    /// 语义校验：格式取值、include 前缀形状、图集名不空。
    ///
    /// 校验放在**读配置**这一步（而不是写页那一步）是 P0-2 的直接教训：
    /// 错拼的 `format` 曾经要等到 `image` 按扩展名写文件时才报"不认识的图片格式"。
    pub fn validate(&self) -> anyhow::Result<()> {
        for spec in &self.atlases {
            if spec.name.trim().is_empty() {
                bail!("图集名不能为空");
            }
            if spec.include.is_empty() {
                bail!("图集 {} 没有 include：不知道该打哪些图", spec.name);
            }
            for include in &spec.include {
                if !include.ends_with('/') {
                    bail!(
                        "图集 {} 的 include 必须以 '/' 结尾（目录前缀）：{include}",
                        spec.name
                    );
                }
            }
            AtlasFormat::parse(&spec.format)?;
            Oversize::parse(spec.oversize.as_deref())?;
        }
        if let Some(auto) = &self.auto {
            AtlasFormat::parse(&auto.format)?;
        }
        Ok(())
    }

    /// 确定性 JSON（编辑器保存用；与手写文件同构，便于 diff）。
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|error| anyhow!("序列化 atlas.json 失败：{error}"))
    }
}

/// 一张候选源图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasFile {
    /// 绝对路径。
    pub path: PathBuf,
    /// 工程相对路径（`/` 分隔）。
    pub relative: String,
}

/// 一个**已解析**的图集任务：名字 + 具体源文件 + 编码形态。
#[derive(Clone, Debug)]
pub struct ResolvedSpec {
    pub name: String,
    pub max_size: u32,
    pub padding: u32,
    pub pot: bool,
    pub format: AtlasFormat,
    pub oversize: Oversize,
    pub sources: Vec<PackSource>,
    /// 区域名 → 工程相对路径（写重定向表 / 自检"删了必须有重定向"用）。
    pub by_name: BTreeMap<String, String>,
    /// 诊断用：这条任务从哪来（`显式 ui_hud` / `目录 art/ui/hud` / `单图`）。
    pub origin: String,
}

/// 归约结果。
#[derive(Debug, Clone, Default)]
pub struct AtlasPlan {
    pub specs: Vec<ResolvedSpec>,
    /// 被跳过的图（都要能在日志里看见原因，不做静默丢弃）。
    pub warnings: Vec<String>,
}

/// 已经装好箱的一页（**不含像素**）。
///
/// 编辑器预览与发布 cook 共用它：列表页只看尺寸/区域数（不占内存），
/// 用户点开某一页时再 [`render_page_rgba`] 合成那一页的像素。
#[derive(Clone, Debug)]
pub struct PlannedPage {
    pub spec_name: String,
    pub origin: String,
    /// 同一图集名内的页号（`<name>-<index>`）。
    pub index: usize,
    pub format: AtlasFormat,
    pub page: PackedPage,
    pub sources: Vec<PackSource>,
    /// 区域名 → 工程相对路径（写重定向表用）。
    pub by_name: BTreeMap<String, String>,
    /// 被跳过、没有进这一页的源（`oversize = skip`）。
    pub skipped: Vec<String>,
}

impl PlannedPage {
    /// 页像素面积（页总像素是图集的真实成本项，不是页数）。
    #[must_use]
    pub fn pixel_area(&self) -> u64 {
        u64::from(self.page.width) * u64::from(self.page.height)
    }

    #[must_use]
    pub fn region_count(&self) -> usize {
        self.page.regions.len()
    }
}

/// 一整套装箱结果。
#[derive(Debug, Clone, Default)]
pub struct PageSet {
    pub pages: Vec<PlannedPage>,
    pub warnings: Vec<String>,
    /// 被排除规则挡下的候选数（编辑器面板显示）。
    pub excluded: usize,
}

/// 把配置一路做到"可写盘的页"：归约 → 装箱 → 摊平。
pub fn plan_pages(
    config: &AtlasConfig,
    files: &[AtlasFile],
    excluded: &BTreeSet<String>,
) -> anyhow::Result<PageSet> {
    let plan = resolve_specs(config, files, excluded)?;
    let mut set = PageSet {
        pages: Vec::new(),
        warnings: plan.warnings,
        excluded: excluded.len(),
    };
    for spec in &plan.specs {
        let (pages, skipped) = pack_spec(spec)?;
        for name in &skipped {
            set.warnings.push(format!(
                "跳过 {}::{name}（源图超过单页上限 {}，源图原样保留）",
                spec.name, spec.max_size
            ));
        }
        for (index, page) in pages.into_iter().enumerate() {
            set.pages.push(PlannedPage {
                spec_name: spec.name.clone(),
                origin: spec.origin.clone(),
                index,
                format: spec.format,
                page,
                sources: spec.sources.clone(),
                by_name: spec.by_name.clone(),
                skipped: skipped.clone(),
            });
        }
    }
    Ok(set)
}

/// 合成一页的 RGBA8 像素（编辑器预览用；`assetc` 的写盘路径也走这里）。
pub fn render_page_rgba(planned: &PlannedPage) -> anyhow::Result<Vec<u8>> {
    crate::atlas_pack::compose_page_rgba(&planned.page, &planned.sources)
        .map_err(|error| anyhow!("合成图集页失败：{error}"))
}

/// 把一页编码成 `.rltex`（裸像素，压缩交给容器块）。
pub fn encode_page_rltex(planned: &PlannedPage) -> anyhow::Result<Vec<u8>> {
    let pixels = render_page_rgba(planned)?;
    rustlua_assets::rltex::encode(planned.page.width, planned.page.height, &pixels)
        .map_err(|error| anyhow!("编码 .rltex 失败：{error}"))
}

/// 把一页编码成 PNG（开发形态 / 稠密图更省的场合）。
pub fn encode_page_png(planned: &PlannedPage) -> anyhow::Result<Vec<u8>> {
    use image::{ExtendedColorType, ImageEncoder};
    let pixels = render_page_rgba(planned)?;
    let mut out = Vec::with_capacity(pixels.len() / 4);
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &pixels,
            planned.page.width,
            planned.page.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| anyhow!("编码 PNG 失败：{error}"))?;
    Ok(out)
}

/// 收集 `root/<base>` 下的全部 PNG（`base` 是工程相对目录，通常是 `assets`）。
pub fn collect_png_files(root: &Path, base: &str) -> anyhow::Result<Vec<AtlasFile>> {
    let mut found = Vec::new();
    let mut stack = vec![root.join(base)];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("png"))
            {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| anyhow!("{} 不在工程根内", path.display()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push(AtlasFile { path, relative });
            }
        }
    }
    found.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(found)
}

/// 运行期**不支持重定向**的纹理引用集合（工程相对路径）。
///
/// 判据必须与装配期一致（`docs/67` §6.4）：被重定向的只有 `Sprite` 与 UI `Image`
/// 这两条装配路径，其余消费方（`Tilemap`、动画表、九宫格）拿到的必须是整张原图。
/// 历史上这里只放行 `Sprite`，于是 UI `Image` 引用的图永远进不了图集——比实际能力更保守。
pub fn collect_excluded(root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut excluded = BTreeSet::new();
    // ① 场景与预制体里的组件引用。
    for dir in ["scenes", "assets/prefabs"] {
        collect_component_refs(&root.join(dir), &mut excluded)?;
    }
    // ② 动画表（`sheet=`）在装配期按整张表算 UV，不查重定向。
    for dir in ["scenes", "assets/prefabs"] {
        collect_sheet_refs(&root.join(dir), &mut excluded)?;
    }
    // ③ Lua 里拼出来/直接交给 tilemap 的图：静态图看不见，但删了就会白。
    collect_lua_refs(&root.join("scripts"), &mut excluded)?;
    Ok(excluded)
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn collect_component_refs(dir: &Path, out: &mut BTreeSet<String>) -> anyhow::Result<()> {
    let mut files = Vec::new();
    walk_files(dir, &mut files);
    files.sort();
    for path in files {
        let is_scene = path.extension().and_then(|v| v.to_str()) == Some("scene");
        let is_prefab = path.extension().and_then(|v| v.to_str()) == Some("prefab");
        if !is_scene && !is_prefab {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}');
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 || parts[0].trim() != "component" {
                continue;
            }
            let component = parts[2].trim();
            let rest = parts[3..].join("\t");
            // 九宫格按整张纹理算边框：无论组件类型都不许换页。
            let nine_slice = rest
                .split(';')
                .any(|field| field.trim_start().starts_with("nine_slice="));
            let redirectable = matches!(component, "Sprite" | "Image") && !nine_slice;
            for field in rest.split(';') {
                let Some(index) = field.find("texture=") else {
                    continue;
                };
                let value = field[index + "texture=".len()..].trim();
                if value.is_empty() || !value.starts_with("assets/") {
                    continue;
                }
                if !redirectable {
                    out.insert(value.to_owned());
                }
            }
        }
    }
    Ok(())
}

fn collect_sheet_refs(dir: &Path, out: &mut BTreeSet<String>) -> anyhow::Result<()> {
    let mut files = Vec::new();
    walk_files(dir, &mut files);
    files.sort();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for token in text.split([';', '\t', ' ', '\n']) {
            let Some(value) = token.trim().strip_prefix("sheet=") else {
                continue;
            };
            let value = value.trim();
            if value.starts_with("assets/") {
                out.insert(value.to_owned());
            }
        }
    }
    Ok(())
}

/// Lua 侧的纹理引用：字面量路径、`string.format` 模式、以及 `tilemap.*` 调用参数。
///
/// 三条都要收，原因是它们覆盖了静态场景扫描看不到的三类消费方：
/// - `farm_soil.lua` 的 `M.farm_texture = "assets/…/manor_farm.png"`（数据表）；
/// - `season_art.lua` 的 `"assets/…/manor_ground_%s.png"`（`string.format` 拼四季地表）；
/// - `tilemap.set_texture(id, path)`（运行期换整张地表）。
///
/// 只收 `assets/` 开头且以 `.png` 结尾（或含 `%` 的模式）的字面量，避免把
/// 组件串（`texture=…;size=…`）里的普通精灵引用误判成"不可重定向"。
fn collect_lua_refs(dir: &Path, out: &mut BTreeSet<String>) -> anyhow::Result<()> {
    let mut files = Vec::new();
    walk_files(dir, &mut files);
    files.sort();
    for path in files {
        if path.extension().and_then(|value| value.to_str()) != Some("lua") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let is_tilemap_call = line.contains("tilemap.");
            for literal in string_literals(line) {
                let Some(start) = literal.find("assets/") else {
                    continue;
                };
                let candidate = literal[start..].trim();
                if candidate.contains('%') {
                    // 模式串留给 `expand_pattern` 在拿到文件清单之后再展开。
                    out.insert(format!("\u{1}pattern:{candidate}"));
                    continue;
                }
                if !candidate.to_ascii_lowercase().ends_with(".png") {
                    continue;
                }
                if is_tilemap_call {
                    out.insert(candidate.to_owned());
                }
            }
        }
    }
    Ok(())
}

/// 极简 Lua 字符串字面量扫描：跳过 `--` 注释，识别 `"…"`/`'…'` 与反斜杠转义。
fn string_literals(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '-' && chars.get(index + 1) == Some(&'-') {
            break;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            index += 1;
            let mut buffer = String::new();
            while index < chars.len() {
                let current = chars[index];
                if current == '\\' && index + 1 < chars.len() {
                    buffer.push(chars[index + 1]);
                    index += 2;
                    continue;
                }
                if current == quote {
                    index += 1;
                    break;
                }
                buffer.push(current);
                index += 1;
            }
            out.push(buffer);
            continue;
        }
        index += 1;
    }
    out
}

/// 把 `…/x_%s.png` 这类模式按"首个 `%` 之前 + 末个 `%` 之后"展开成具体文件。
///
/// 不做完整 glob：本项目里的模式都是"一个占位符替换成季节/地图名"，
/// 前缀 + 后缀已经足够把它们全部覆盖，而且不会把相邻目录误吞进来。
fn expand_pattern(pattern: &str, files: &[AtlasFile]) -> Vec<String> {
    let Some(first) = pattern.find('%') else {
        return Vec::new();
    };
    let last = pattern.rfind('%').unwrap_or(first);
    let prefix = &pattern[..first];
    let suffix = &pattern[last + 1..];
    files
        .iter()
        .filter(|file| file.relative.starts_with(prefix) && file.relative.ends_with(suffix))
        .map(|file| file.relative.clone())
        .collect()
}

/// 把配置归约成任务清单。
///
/// 顺序：显式声明（配置顺序）→ 目录聚合（目录名排序）→ 单图（路径排序）。
/// 同一张图只进一条任务：显式命中后，目录聚合会跳过它。
pub fn resolve_specs(
    config: &AtlasConfig,
    files: &[AtlasFile],
    excluded: &BTreeSet<String>,
) -> anyhow::Result<AtlasPlan> {
    config.validate()?;

    // 模式串展开成具体路径；`\u{1}pattern:` 前缀只在本函数内存在。
    let mut blocked: BTreeSet<String> = BTreeSet::new();
    for entry in excluded {
        if let Some(pattern) = entry.strip_prefix("\u{1}pattern:") {
            blocked.extend(expand_pattern(pattern, files));
        } else {
            blocked.insert(entry.clone());
        }
    }

    let separate: BTreeSet<&str> = config.separate.iter().map(String::as_str).collect();
    let mut plan = AtlasPlan::default();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();

    // ① 显式声明。
    for spec in &config.atlases {
        let format = AtlasFormat::parse(&spec.format)?;
        let oversize = Oversize::parse(spec.oversize.as_deref())?;
        let mut sources = Vec::new();
        let mut by_name = BTreeMap::new();
        for file in files {
            let Some(stripped) = strip_include(&file.relative, &spec.include) else {
                continue;
            };
            match admissible(stripped, &file.relative, &blocked, &spec.name, true) {
                Ok(Some(region)) => {
                    if let Some(previous) = by_name.insert(region.clone(), file.relative.clone()) {
                        bail!(
                            "图集 {} 的区域名冲突：{previous} 与 {} 都映射到 {region}",
                            spec.name,
                            file.relative
                        );
                    }
                    claimed.insert(file.relative.clone());
                    sources.push(PackSource {
                        name: region,
                        path: file.path.clone(),
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        if sources.is_empty() {
            continue;
        }
        record_name(&mut names, &spec.name, &format!("显式图集 {}", spec.name))?;
        plan.specs.push(ResolvedSpec {
            name: spec.name.clone(),
            max_size: spec.max_size,
            padding: spec.padding,
            pot: spec.pot,
            format,
            oversize,
            sources,
            by_name,
            origin: format!("显式图集 {}", spec.name),
        });
    }

    // ② 目录默认聚合。
    let Some(auto) = config.auto.as_ref().filter(|auto| auto.enabled) else {
        return Ok(plan);
    };
    let format = AtlasFormat::parse(&auto.format)?;
    let root_prefix = if auto.root.ends_with('/') {
        auto.root.clone()
    } else {
        format!("{}/", auto.root)
    };
    let mut buckets: BTreeMap<String, Vec<AtlasFile>> = BTreeMap::new();
    let mut singles: Vec<AtlasFile> = Vec::new();
    for file in files {
        if claimed.contains(&file.relative) {
            continue;
        }
        let Some(stripped) = strip_include(&file.relative, std::slice::from_ref(&root_prefix))
        else {
            continue;
        };
        if blocked.contains(&file.relative) {
            plan.warnings.push(format!(
                "跳过 {}（Tilemap/动画表/九宫格/Lua 拼出来的图：UV 按整张图算，进图集会错位）",
                file.relative
            ));
            continue;
        }
        match admissible(stripped, &file.relative, &blocked, "auto", false) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(error) => {
                plan.warnings.push(error.to_string());
                continue;
            }
        }
        let (width, height) =
            source_dimensions(&file.path).map_err(|error| anyhow!("{}: {error}", file.relative))?;
        let area = u64::from(width) * u64::from(height);
        let dir = match stripped.rsplit_once('/') {
            Some((dir, _)) => dir.to_owned(),
            None => String::new(),
        };
        let is_single =
            separate.contains(file.relative.as_str()) || area >= u64::from(auto.single_min_pixels);
        if is_single {
            singles.push(file.clone());
        } else {
            buckets.entry(dir).or_default().push(file.clone());
        }
    }

    for (dir, items) in buckets {
        // 目录里只剩一张图就没有合并收益：按"单图集"处理。
        if items.len() < 2 {
            singles.extend(items);
            continue;
        }
        let name = format!("{}_default", dir_label(&dir));
        let (sources, by_name) = into_sources(items, &name)?;
        record_name(&mut names, &name, &format!("目录 {dir:?} 的默认聚合"))?;
        plan.specs.push(ResolvedSpec {
            name,
            max_size: auto.max_size,
            padding: auto.padding,
            pot: false,
            format,
            oversize: Oversize::Skip,
            sources,
            by_name,
            origin: format!("目录 {dir:?} 的默认聚合"),
        });
    }

    singles.sort_by(|a, b| a.relative.cmp(&b.relative));
    for file in singles {
        let stem = stem_of(&file.relative);
        let dir = match file.relative.rsplit_once('/') {
            Some((dir, _)) => dir.to_owned(),
            None => String::new(),
        };
        let name = format!("{}_{stem}", dir_label(&dir));
        let region = region_name(&file.relative);
        let mut by_name = BTreeMap::new();
        by_name.insert(region.clone(), file.relative.clone());
        record_name(&mut names, &name, &format!("单图 {}", file.relative))?;
        plan.specs.push(ResolvedSpec {
            name,
            // 单图集不设上限：它本来就是"放不进默认页的大图"。
            max_size: auto.max_size.max(4096),
            padding: auto.padding,
            pot: false,
            format,
            oversize: Oversize::Skip,
            sources: vec![PackSource {
                name: region,
                path: file.path.clone(),
            }],
            by_name,
            origin: format!("单图 {}", file.relative),
        });
    }
    Ok(plan)
}

/// 图集名唯一（P1-3）：两张页写同一个 `assets/atlases/<name>-0.rltex` 会互相覆盖，
/// 先登记的区域指向被覆盖的页 ⇒ 运行期回落已删除的源图 ⇒ 白图。
fn record_name(
    names: &mut BTreeMap<String, String>,
    name: &str,
    origin: &str,
) -> anyhow::Result<()> {
    let key = sanitize(name);
    if let Some(previous) = names.insert(key.clone(), origin.to_owned()) {
        bail!(
            "图集名冲突：{origin} 与 {previous} 都映射到 {key:?}（会让两页写同一个文件名）。\
             给目录改名，或在 `separate` 里点名后改图集名。"
        );
    }
    Ok(())
}

/// 按任务装箱；`oversize = skip` 时把放不进单页的源剔除并继续。
pub fn pack_spec(spec: &ResolvedSpec) -> anyhow::Result<(Vec<PackedPage>, Vec<String>)> {
    let options = AtlasPackerOptions {
        padding: spec.padding,
        max_size: spec.max_size,
        pot: spec.pot,
    };
    match pack_atlas(&spec.sources, &options) {
        Ok(pages) => Ok((pages, Vec::new())),
        Err(_error) if spec.oversize == Oversize::Skip => {
            let mut kept = spec.sources.clone();
            let mut skipped = Vec::new();
            loop {
                match pack_atlas(&kept, &options) {
                    Ok(pages) => return Ok((pages, skipped)),
                    Err(crate::atlas_pack::AtlasPackError::ItemTooLarge { name, .. }) => {
                        let before = kept.len();
                        kept.retain(|source| source.name != name);
                        skipped.push(name);
                        if kept.is_empty() || kept.len() == before {
                            return Ok((Vec::new(), skipped));
                        }
                    }
                    Err(other) => return Err(anyhow!("{other}")),
                }
            }
        }
        Err(error) => Err(anyhow!("{error}")),
    }
}

/// 一张图能不能进图集。`strict` = 显式配置（作者意图与规则冲突必须报错）。
fn admissible(
    stripped: &str,
    relative: &str,
    blocked: &BTreeSet<String>,
    atlas: &str,
    strict: bool,
) -> anyhow::Result<Option<String>> {
    if relative.starts_with(&format!("{ATLAS_DIR}/")) {
        return Ok(None);
    }
    // 预览/调试图（`_preview.png`、`_ground_preview.png`）：不是运行期资源，
    // 进了图集只会把 cook 的内存与页数白抬上去（实测有一页 raw 达 33 MB）。
    if basename(relative).starts_with('_') {
        return Ok(None);
    }
    // 瓦片集目录：UV 按整张纹理算，换页必然错位（四季地表就是 Lua 拼出来的）。
    if relative.starts_with(TILESET_DIR) {
        return Ok(None);
    }
    if blocked.contains(relative) {
        // **显式配置**里点名要打的图被拒 → 硬错误（作者意图与规则冲突，必须让人看见）。
        // **auto 扫描**扫到这类图是常态 → 跳过并告警，否则"打开目录聚合"等于"整个工程打不出包"。
        let message = format!(
            "图集 {atlas} 想打 {relative}，但它是**自取纹理的消费方**（Tilemap / 动画表 / \
             九宫格 / Lua 拼出来的路径）用的图：那类消费方的 UV 按整张图算，换成图集会整体错位\
             （docs/67 §6.2）。"
        );
        if strict {
            bail!("{message} 请把它排除出配置。");
        }
        return Ok(None);
    }
    let region = region_name(stripped);
    // 九宫格（`_9s`）不进图集：边框按整张纹理算，换页会取错位置。
    if region.ends_with("_9s") {
        return Ok(None);
    }
    Ok(Some(region))
}

fn into_sources(
    items: Vec<AtlasFile>,
    atlas: &str,
) -> anyhow::Result<(Vec<PackSource>, BTreeMap<String, String>)> {
    let mut sources = Vec::with_capacity(items.len());
    let mut by_name = BTreeMap::new();
    for file in items {
        let region = stem_of(&file.relative);
        if let Some(previous) = by_name.insert(region.clone(), file.relative.clone()) {
            bail!(
                "图集 {atlas} 的区域名冲突：{previous} 与 {} 都映射到 {region}",
                file.relative
            );
        }
        sources.push(PackSource {
            name: region,
            path: file.path,
        });
    }
    Ok((sources, by_name))
}

/// 相对路径去掉命中的 include 前缀；不命中返回 `None`。
fn strip_include<'a>(relative: &'a str, include: &[String]) -> Option<&'a str> {
    include
        .iter()
        .find_map(|prefix| relative.strip_prefix(prefix.as_str()))
        .filter(|rest| !rest.is_empty())
}

/// 区域名：去掉 `.png`、`/` 换 `_`（稳定、可读、不会因为重名静默覆盖）。
fn region_name(stripped: &str) -> String {
    let stem = stripped
        .strip_suffix(".png")
        .or_else(|| stripped.strip_suffix(".PNG"))
        .unwrap_or(stripped);
    stem.replace('/', "_")
}

fn stem_of(relative: &str) -> String {
    basename(relative)
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_owned())
        .unwrap_or_else(|| basename(relative).to_owned())
}

fn basename(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

/// 目录名取最后一段（`art/ui/title` → `title`），空则 `misc`。
fn dir_label(dir: &str) -> String {
    let label = dir.rsplit('/').next().unwrap_or("").trim();
    if label.is_empty() {
        "misc".to_owned()
    } else {
        sanitize(label)
    }
}

/// 文件名安全形式（`art:ui` → `art-ui`）。
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

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-atlas-plan-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/art/ui/hud")).unwrap();
        std::fs::create_dir_all(root.join("assets/art/ui/title")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        root
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        use image::{ExtendedColorType, ImageEncoder};
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let pixels = vec![255u8; (w * h * 4) as usize];
        let file = std::fs::File::create(path).unwrap();
        let writer = std::io::BufWriter::new(file);
        image::codecs::png::PngEncoder::new(writer)
            .write_image(&pixels, w, h, ExtendedColorType::Rgba8)
            .unwrap();
    }

    fn auto_config(root: &str) -> AtlasConfig {
        AtlasConfig {
            schema: 1,
            atlases: Vec::new(),
            auto: Some(AutoAtlas {
                enabled: true,
                root: root.to_owned(),
                max_size: 64,
                padding: 1,
                format: "rltex".to_owned(),
                single_min_pixels: 256 * 256,
            }),
            separate: Vec::new(),
        }
    }

    #[test]
    fn format_default_is_rltex_and_typos_are_rejected_at_load() {
        let text = r#"{
            "schema": 1,
            "atlases": [{ "name": "ui", "include": ["assets/art/ui/"] }]
        }"#;
        let config: AtlasConfig = serde_json::from_str(text).unwrap();
        assert_eq!(
            config.atlases[0].format, "rltex",
            "省略 format 必须是 rltex"
        );
        config.validate().expect("合法配置");

        let bad: AtlasConfig =
            serde_json::from_str(r#"{ "schema": 1, "auto": { "format": "rtex" } }"#).unwrap();
        let error = bad.validate().expect_err("拼错必须当场拒绝");
        assert!(error.to_string().contains("rltex|png|auto"), "{error}");
    }

    #[test]
    fn auto_merges_small_images_and_skips_previews() {
        let root = temp_root("auto-merge");
        write_png(&root.join("assets/art/ui/title/a.png"), 8, 8);
        write_png(&root.join("assets/art/ui/title/b.png"), 8, 8);
        write_png(&root.join("assets/art/ui/title/_preview.png"), 64, 64);
        let files = collect_png_files(&root, "assets").unwrap();
        let plan = resolve_specs(&auto_config("assets/art/ui"), &files, &BTreeSet::new()).unwrap();
        let spec = plan
            .specs
            .iter()
            .find(|spec| spec.name == "title_default")
            .expect("同名合并应产生 title_default");
        assert_eq!(spec.sources.len(), 2, "预览图必须被跳过");
        assert!(
            spec.sources
                .iter()
                .all(|source| !source.name.contains("preview"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_atlas_names_are_rejected() {
        let root = temp_root("dup-name");
        write_png(&root.join("assets/art/ui/hud/a.png"), 8, 8);
        write_png(&root.join("assets/art/ui/title/a.png"), 8, 8);
        // 两个显式图集用了同一个名字：会写同一个 `assets/atlases/same-0.*`。
        let explicit = AtlasConfig {
            schema: 1,
            atlases: vec![
                AtlasSpec {
                    name: "same".to_owned(),
                    include: vec!["assets/art/ui/hud/".to_owned()],
                    max_size: 64,
                    padding: 1,
                    pot: false,
                    format: "rltex".to_owned(),
                    oversize: None,
                },
                AtlasSpec {
                    name: "same".to_owned(),
                    include: vec!["assets/art/ui/title/".to_owned()],
                    max_size: 64,
                    padding: 1,
                    pot: false,
                    format: "rltex".to_owned(),
                    oversize: None,
                },
            ],
            auto: None,
            separate: Vec::new(),
        };
        let files = collect_png_files(&root, "assets").unwrap();
        let error =
            resolve_specs(&explicit, &files, &BTreeSet::new()).expect_err("同名图集必须被拒绝");
        assert!(error.to_string().contains("图集名冲突"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tilesets_and_nine_slice_are_never_packed() {
        let root = temp_root("boundaries");
        write_png(&root.join("assets/art/tilesets/manor/ground.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/panel_9s.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/icon.png"), 8, 8);
        let files = collect_png_files(&root, "assets").unwrap();
        let mut config = auto_config("assets");
        config.auto.as_mut().unwrap().max_size = 64;
        let plan = resolve_specs(&config, &files, &BTreeSet::new()).unwrap();
        let sources: Vec<&str> = plan
            .specs
            .iter()
            .flat_map(|spec| spec.by_name.values().map(String::as_str))
            .collect();
        assert!(!sources.iter().any(|path| path.contains("tilesets/")));
        assert!(!sources.iter().any(|path| path.contains("_9s")));
        assert!(sources.iter().any(|path| path.ends_with("icon.png")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lua_patterns_expand_to_concrete_tilesets() {
        let root = temp_root("lua-pattern");
        write_png(
            &root.join("assets/art/tilesets/manor/manor_ground_spring.png"),
            8,
            8,
        );
        write_png(
            &root.join("assets/art/tilesets/manor/manor_ground_summer.png"),
            8,
            8,
        );
        std::fs::create_dir_all(root.join("scripts/data")).unwrap();
        std::fs::write(
            root.join("scripts/data/season_art.lua"),
            "local T = {\n  pattern = \"assets/art/tilesets/manor/manor_ground_%s.png\",\n}\n",
        )
        .unwrap();
        let excluded = collect_excluded(&root).unwrap();
        let mut config = auto_config("assets");
        config.auto.as_mut().unwrap().max_size = 64;
        let files = collect_png_files(&root, "assets").unwrap();
        let plan = resolve_specs(&config, &files, &excluded).unwrap();
        assert!(
            plan.specs.is_empty(),
            "只有 Lua 模式命中的瓦片集不该生成任何图集：{plan:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ui_image_components_are_redirectable_but_nine_slice_is_not() {
        let root = temp_root("image-refs");
        write_png(&root.join("assets/art/ui/hud/plain.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/framed.png"), 8, 8);
        std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        std::fs::write(
            root.join("assets/prefabs/hud.prefab"),
            "prefab_version=1\n\
             component\t1\tImage\ttexture=assets/art/ui/hud/plain.png;color=1,1,1\n\
             component\t2\tImage\ttexture=assets/art/ui/hud/framed.png;nine_slice=4,4,4,4\n\
             component\t3\tTilemap\ttexture=assets/art/tilesets/x.png;atlas_cols=2\n",
        )
        .unwrap();
        let excluded = collect_excluded(&root).unwrap();
        assert!(!excluded.contains("assets/art/ui/hud/plain.png"));
        assert!(excluded.contains("assets/art/ui/hud/framed.png"));
        assert!(excluded.contains("assets/art/tilesets/x.png"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn explicit_spec_keeps_oversize_as_error_and_auto_skips() {
        let root = temp_root("oversize");
        write_png(&root.join("assets/art/ui/hud/small.png"), 8, 8);
        write_png(&root.join("assets/art/ui/hud/huge.png"), 200, 8);
        let files = collect_png_files(&root, "assets").unwrap();
        let explicit = AtlasConfig {
            schema: 1,
            atlases: vec![AtlasSpec {
                name: "ui".to_owned(),
                include: vec!["assets/art/ui/hud/".to_owned()],
                max_size: 64,
                padding: 1,
                pot: false,
                format: "rltex".to_owned(),
                oversize: None,
            }],
            auto: None,
            separate: Vec::new(),
        };
        let plan = resolve_specs(&explicit, &files, &BTreeSet::new()).unwrap();
        let error = pack_spec(&plan.specs[0]).expect_err("显式配置默认 oversize=error");
        assert!(error.to_string().contains("200x8"), "{error}");

        let mut auto = auto_config("assets/art/ui");
        auto.auto.as_mut().unwrap().max_size = 64;
        let plan = resolve_specs(&auto, &files, &BTreeSet::new()).unwrap();
        let mut pages = 0usize;
        let mut skipped: Vec<String> = Vec::new();
        for spec in &plan.specs {
            let (spec_pages, spec_skipped) = pack_spec(spec).unwrap();
            pages += spec_pages.len();
            skipped.extend(spec_skipped);
        }
        assert!(pages > 0, "8x8 的小图必须照常成页");
        assert_eq!(skipped, vec!["huge".to_owned()], "超限源由 auto 跳过");
        std::fs::remove_dir_all(&root).ok();
    }
}
