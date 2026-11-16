//! M82-B2：`texture` 模块（维度无关基底）——程序化贴图的生成与显式释放。
//!
//! 生成核心在 [`crate::texture_gen`]（纯 CPU、seed 确定、跨平台逐字节一致）；这里持**运行期
//! 注册表**与 Lua 绑定。消费走与文件贴图完全相同的缓存键：宿主每帧取走 `pending` 上传
//! `Renderer::upload_texture_rgba8`，sprite/quad 按哨兵键 `__texture__?<id>` 取样——渲染器
//! 零结构改动（`rustlua-gpu` 离屏门 `texture_gen_gpu` 已证采样即生成、确定性、原地替换）。
//!
//! 防泄漏纪律（计划 §风险）：键由 id 唯一决定，**同 id 重生成原地替换**（渲染器缓存按 key
//! 复用槽位），于是"每帧重生成同一张"纹理数恒定。`free` 释放 Lua 侧句柄；渲染器 2D 缓存
//! 当前只追加，故 free 不回收 GPU 槽位——这是已登记的边界，浸泡判据落在"稳定键复用不增长"。

use crate::text_texture::{TextSpec, render_text};
use crate::texture::TextureAsset;
use crate::texture_gen::{TextureGenError, TextureKind, TextureSpec};
use mlua::{Lua, Result as LuaResult, Table};
use rustlua_lua_api::{Handle, ProjectRoot, api_error};
use std::collections::HashMap;
use std::path::Path;

/// 哨兵键前缀（与 mesh 的 `__primitive__` 同族：串就是身份，不是文件路径）。
pub const TEXTURE_SENTINEL_PREFIX: &str = "__texture__?";

/// id → 消费键。键只在这一处决定（上传与 sprite 消费共用同一串，避免两处各挑一次）。
#[must_use]
pub fn texture_key(id: u32) -> String {
    format!("{TEXTURE_SENTINEL_PREFIX}{id}")
}

/// 是否程序化贴图哨兵键（sprite/材质消费侧与文法校验用）。
#[must_use]
pub fn is_texture_sentinel(path: &str) -> bool {
    path.starts_with(TEXTURE_SENTINEL_PREFIX)
}

/// 工程字体缓存。ab_glyph `FontArc` 不实现 `Debug`，包一层手动 Debug 只暴露数量。
#[derive(Default)]
struct FontCache(HashMap<String, ab_glyph::FontArc>);

impl std::fmt::Debug for FontCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontCache")
            .field("len", &self.0.len())
            .finish()
    }
}

/// 运行期生成贴图注册表：Lua 写、宿主每帧取走上传。与 `RenderableState` 同构（能力单元）。
#[derive(Debug, Default)]
pub struct TextureGenState {
    next_id: u32,
    live: HashMap<u32, TextureAsset>,
    /// 待宿主上传的 id（新生成/更新）；取走后清空，长度恒 ≤ 一帧的生成量。
    pending: Vec<u32>,
    /// 已解析的工程字体（按绝对路径缓存；`texture.from_text` 复用）。
    fonts: FontCache,
}

impl TextureGenState {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "texture_gen")
    }

    /// 生成一张贴图并登记，返回 id。规格非法时报错（不静默成功）。
    pub fn generate(&mut self, spec: &TextureSpec) -> Result<u32, TextureGenError> {
        let asset = spec.generate()?;
        Ok(self.insert(asset))
    }

    /// 登记一张已生成的贴图（`from_text` 复用），返回 id。
    pub fn insert(&mut self, asset: TextureAsset) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.live.insert(id, asset);
        self.pending.push(id);
        id
    }

    /// 显式释放句柄；返回是否确有其 id。残留的 pending 项上传时查不到 live 即跳过。
    pub fn free(&mut self, id: u32) -> bool {
        self.live.remove(&id).is_some()
    }

    /// id 是否仍存活（消费侧校验用）。
    #[must_use]
    pub fn is_live(&self, id: u32) -> bool {
        self.live.contains_key(&id)
    }

    /// 存活贴图数（浸泡判据：稳定键复用下应恒定）。
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// 取走本帧待上传的 (键, 贴图)，去重且只取仍存活的。宿主上传后清空。
    pub fn take_pending(&mut self) -> Vec<(String, TextureAsset)> {
        let ids: Vec<u32> = self.pending.drain(..).collect();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for id in ids {
            if !seen.insert(id) {
                continue;
            }
            if let Some(asset) = self.live.get(&id) {
                out.push((texture_key(id), asset.clone()));
            }
        }
        out
    }

    /// 读并缓存工程字体（路径相对工程根）。`texture.from_text` 用；同路径只读/解析一次。
    pub fn font(&mut self, root: &Path, relative: &str) -> Result<ab_glyph::FontArc, String> {
        let full = root.join(relative);
        let key = full.to_string_lossy().into_owned();
        if let Some(font) = self.fonts.0.get(&key) {
            return Ok(font.clone());
        }
        let bytes = rustlua_infra::vfs::read(&full)
            .map_err(|reason| format!("read font `{relative}`: {reason}"))?;
        let font = ab_glyph::FontArc::try_from_vec(bytes)
            .map_err(|_| format!("parse font `{relative}`: not a valid font"))?;
        self.fonts.0.insert(key, font.clone());
        Ok(font)
    }
}

/// Lua spec 表 → [`TextureSpec`]。kind 必填；size/seed/scale/colors 有缺省。
fn parse_spec(table: &Table) -> LuaResult<TextureSpec> {
    let kind_token: String = table
        .get::<Option<String>>("kind")?
        .ok_or_else(|| api_error("texture spec requires `kind`"))?;
    let kind = TextureKind::from_token(&kind_token)
        .ok_or_else(|| api_error(format!("unknown texture kind `{kind_token}`")))?;
    let mut spec = TextureSpec::default_for(kind);
    if let Some(size) = table.get::<Option<u32>>("size")? {
        spec.size = size;
    }
    if let Some(seed) = table.get::<Option<u32>>("seed")? {
        spec.seed = seed;
    }
    if let Some(scale) = table.get::<Option<f32>>("scale")? {
        spec.scale = scale;
    }
    if let Some(colors) = table.get::<Option<Table>>("colors")? {
        spec.colors = parse_colors(&colors)?;
    }
    Ok(spec)
}

/// 调色板表 → RGBA 数组列表。每色是 3 或 4 个 0..255 整数（缺 alpha 补 255）。
fn parse_colors(table: &Table) -> LuaResult<Vec<[u8; 4]>> {
    let mut out = Vec::new();
    for pair in table.clone().sequence_values::<Table>() {
        let color = pair.map_err(|error| api_error(error.to_string()))?;
        let mut rgba = [0u8; 4];
        let len = color.raw_len();
        if len != 3 && len != 4 {
            return Err(api_error("each texture color must have 3 or 4 channels"));
        }
        for (channel, slot) in rgba.iter_mut().take(len).enumerate() {
            let value: i64 = color
                .get::<Option<i64>>(channel + 1)?
                .ok_or_else(|| api_error("texture color channels must be integers"))?;
            if !(0..=255).contains(&value) {
                return Err(api_error("texture color channels must be 0..=255"));
            }
            *slot = value as u8;
        }
        if len == 3 {
            rgba[3] = 255;
        }
        out.push(rgba);
    }
    Ok(out)
}

/// 单个颜色表 → RGBA（3 或 4 通道，缺 alpha 补 255）。
fn parse_color(table: &Table) -> LuaResult<[u8; 4]> {
    let len = table.raw_len();
    if len != 3 && len != 4 {
        return Err(api_error("texture color must have 3 or 4 channels"));
    }
    let mut rgba = [0u8; 4];
    for (channel, slot) in rgba.iter_mut().take(len).enumerate() {
        let value: i64 = table
            .get::<Option<i64>>(channel + 1)?
            .ok_or_else(|| api_error("texture color channels must be integers"))?;
        if !(0..=255).contains(&value) {
            return Err(api_error("texture color channels must be 0..=255"));
        }
        *slot = value as u8;
    }
    if len == 3 {
        rgba[3] = 255;
    }
    Ok(rgba)
}

/// `texture.from_text(text, opts)` 的 opts 表 → (TextSpec, 可选字体相对路径)。
fn parse_text_opts(opts: Option<&Table>) -> LuaResult<(TextSpec, Option<String>)> {
    let mut spec = TextSpec::default();
    let mut font = None;
    let Some(opts) = opts else {
        return Ok((spec, font));
    };
    if let Some(px) = opts.get::<Option<u32>>("size")? {
        spec.font_px = px;
    }
    if let Some(padding) = opts.get::<Option<u32>>("padding")? {
        spec.padding = padding;
    }
    if let Some(color) = opts.get::<Option<Table>>("color")? {
        spec.color = parse_color(&color)?;
    }
    if let Some(background) = opts.get::<Option<Table>>("bg")? {
        spec.background = parse_color(&background)?;
    }
    if let Some(max_width) = opts.get::<Option<u32>>("max_width")? {
        spec.max_width = Some(max_width);
    }
    if let Some(path) = opts.get::<Option<String>>("font")? {
        font = Some(path);
    }
    Ok((spec, font))
}

/// 装 `texture` 基底表（2D/3D 都登记）。消费侧（sprite/材质）在各自模块按哨兵键取样。
pub fn install(
    lua: &Lua,
    state: &Handle<TextureGenState>,
    project_root: &Handle<ProjectRoot>,
) -> LuaResult<()> {
    let table = lua.create_table()?;

    let gen_state = state.clone();
    table.set(
        "generate",
        lua.create_function(move |_, spec: Table| {
            let parsed = parse_spec(&spec)?;
            let id = gen_state
                .borrow_mut()?
                .generate(&parsed)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(i64::from(id))
        })?,
    )?;

    let free_state = state.clone();
    table.set(
        "free",
        lua.create_function(move |_, id: i64| {
            let id = u32::try_from(id).map_err(|_| api_error("invalid texture id"))?;
            Ok(free_state.borrow_mut()?.free(id))
        })?,
    )?;

    // `texture.from_text(text, opts)`：把文本用工程字体栅格化成独立 RGBA 贴图（容器编号/限重标签）。
    let text_state = state.clone();
    let text_root = project_root.clone();
    table.set(
        "from_text",
        lua.create_function(move |_, (text, opts): (String, Option<Table>)| {
            let (spec, font_relative) = parse_text_opts(opts.as_ref())?;
            let root = text_root
                .borrow()?
                .path()
                .map(Path::to_path_buf)
                .ok_or_else(|| api_error("texture.from_text requires a project root"))?;
            let font_relative =
                font_relative.unwrap_or_else(|| crate::font::project_font::primary_path(&root));
            let font = text_state
                .borrow_mut()?
                .font(&root, &font_relative)
                .map_err(api_error)?;
            let asset =
                render_text(&font, &text, &spec).map_err(|error| api_error(error.to_string()))?;
            let id = text_state.borrow_mut()?.insert(asset);
            Ok(i64::from(id))
        })?,
    )?;

    lua.globals().set("texture", table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> TextureSpec {
        TextureSpec::default_for(TextureKind::Checker)
    }

    #[test]
    fn generate_registers_and_queues_upload() {
        let mut state = TextureGenState::default();
        let id = state.generate(&checker()).unwrap();
        assert_eq!(id, 0);
        assert!(state.is_live(id));
        assert_eq!(state.live_count(), 1);
        let pending = state.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, texture_key(id));
        assert_eq!(pending[0].1.width(), 64);
        // 取走后清空：下一帧没有新生成就没有待上传。
        assert!(state.take_pending().is_empty());
    }

    #[test]
    fn ids_are_unique_and_keys_are_sentinels() {
        let mut state = TextureGenState::default();
        let a = state.generate(&checker()).unwrap();
        let b = state.generate(&checker()).unwrap();
        assert_ne!(a, b);
        assert!(is_texture_sentinel(&texture_key(a)));
        assert!(!is_texture_sentinel("assets/foo.png"));
        assert!(!is_texture_sentinel("__primitive__?kind=box"));
    }

    #[test]
    fn free_releases_handle_and_skips_pending_upload() {
        let mut state = TextureGenState::default();
        let id = state.generate(&checker()).unwrap();
        assert!(state.free(id));
        assert!(!state.is_live(id));
        assert!(!state.free(id), "重复 free 返回 false");
        assert_eq!(state.live_count(), 0);
        // 已 free 的 id 不该再被上传。
        assert!(state.take_pending().is_empty());
    }

    #[test]
    fn regenerate_same_id_replaces_in_place() {
        let mut state = TextureGenState::default();
        let id = state.generate(&checker()).unwrap();
        let _ = state.take_pending();
        // 同 id 重新 insert（模拟重生成）：live 数不增长，键不变。
        let asset = checker().generate().unwrap();
        state.live.insert(id, asset);
        state.pending.push(id);
        assert_eq!(state.live_count(), 1);
        let pending = state.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, texture_key(id));
    }

    #[test]
    fn invalid_spec_is_rejected_without_registering() {
        let mut state = TextureGenState::default();
        let mut bad = checker();
        bad.size = 0;
        assert!(state.generate(&bad).is_err());
        assert_eq!(state.live_count(), 0);
        assert!(state.take_pending().is_empty());
    }

    #[test]
    fn take_pending_dedupes_repeated_ids() {
        let mut state = TextureGenState::default();
        let id = state.generate(&checker()).unwrap();
        state.pending.push(id);
        state.pending.push(id);
        let pending = state.take_pending();
        assert_eq!(pending.len(), 1, "同帧多次登记同 id 只上传一次");
    }
}
