//! spine2d 0.5 适配：Spine 4.3 数据 → 中性渲染输出。
//!
//! 每帧必须先 `setup_pose` 再 `state.apply`，与官方 runtime 的采样口径一致
//! （P1 golden 对拍 8/8 通过，见 `M84-02`）。`spine2d::DrawList` 的 buffer 复用，
//! 稳态不重新分配。

use std::sync::Arc;

use spine2d::{
    AnimationState, AnimationStateData, Atlas, AttachmentData, BlendMode, Bone, Physics, Skeleton,
    SkeletonData,
};

use crate::model::{
    SpineAttachment, SpineBlend, SpineDraw, SpineError, SpineEvent, SpineEventKind,
    SpineRenderOutput, SpineVertex,
};

/// 与实体共享的不可变骨骼数据。动画名/skin 名预先展开成中性字符串，
/// 上层不必触碰 spine2d 的 `Animation`/`SkinData`。
pub struct BackendData {
    pub(crate) skeleton: Arc<SkeletonData>,
    pub(crate) atlas: Arc<Atlas>,
    animations: Vec<String>,
    skins: Vec<String>,
    /// 页名 → 已解析的工程内路径（加载期建一次）。
    ///
    /// 逐帧产出 draw 时只做一次查表 + `Arc` 克隆，不拼字符串。页的数量在骨架生命周期内
    /// 固定，所以这张表不会失效。
    page_paths: std::collections::HashMap<String, std::sync::Arc<str>>,
}

impl BackendData {
    /// `skeleton_bytes` 是 `.skel`（binary）或 `.json` 的原始字节；`atlas_text` 是
    /// Spine `.atlas`（工程里改名 `.spineatlas`）的 UTF-8 文本。
    ///
    /// `atlas_dir` 是 atlas 文件的**工程内目录**（例如 `assets/spine`）。atlas 里写的是
    /// 相对该目录的页名，输出时用它拼成工程内路径。传空串表示页名已经是工程内路径。
    pub fn from_bytes(
        skeleton_bytes: &[u8],
        binary: bool,
        atlas_text: &str,
        spine_version: &str,
        atlas_dir: &str,
    ) -> Result<Self, SpineError> {
        if !spine_version.starts_with("4.3.") {
            return Err(SpineError::Version(spine_version.to_owned()));
        }
        let skeleton = if binary {
            SkeletonData::from_skel_bytes(skeleton_bytes)
                .map_err(|error| SpineError::Runtime(error.to_string()))?
        } else {
            let text = std::str::from_utf8(skeleton_bytes)
                .map_err(|error| SpineError::Runtime(format!("skeleton is not UTF-8: {error}")))?;
            SkeletonData::from_json_str(text)
                .map_err(|error| SpineError::Runtime(error.to_string()))?
        };
        let atlas = atlas_text
            .parse::<Atlas>()
            .map_err(|error| SpineError::Runtime(format!("atlas: {error}")))?;
        let animations = skeleton
            .get_animations()
            .iter()
            .map(|animation| animation.get_name().to_owned())
            .collect();
        let skins = skeleton
            .get_skins()
            .iter()
            .map(|skin| skin.get_name().to_owned())
            .collect();
        let normalized_dir = normalize_atlas_dir(atlas_dir);
        // 页路径在加载期解析并驻留：逐帧只查表，不拼字符串（P4 稳态零分配）。
        let page_paths = atlas
            .get_pages()
            .iter()
            .map(|page| {
                let resolved: std::sync::Arc<str> =
                    resolve_page_path(&normalized_dir, &page.texture_path).into();
                (page.texture_path.clone(), resolved)
            })
            .collect();
        Ok(Self {
            skeleton,
            atlas: Arc::new(atlas),
            animations,
            skins,
            page_paths,
        })
    }

    #[must_use]
    pub fn animations(&self) -> &[String] {
        &self.animations
    }

    #[must_use]
    pub fn skins(&self) -> &[String] {
        &self.skins
    }

    /// 只校验 `.skel` 字节能否解析成骨骼数据（导入期用，不留状态）。
    pub fn validate_skeleton_bytes(bytes: &[u8]) -> Result<(), SpineError> {
        Self::validate_skeleton(bytes, true)
    }

    /// 只校验 Spine JSON 骨骼能否解析（导入期用，不留状态）。
    pub fn validate_skeleton_json_bytes(bytes: &[u8]) -> Result<(), SpineError> {
        Self::validate_skeleton(bytes, false)
    }

    fn validate_skeleton(bytes: &[u8], binary: bool) -> Result<(), SpineError> {
        parse_skeleton_data(bytes, binary)
            .map(|_| ())
            .map_err(|error| SpineError::Runtime(error.to_string()))
    }

    /// 只校验 `.spineatlas` 文本能否解析（导入期用，不留状态）。
    pub fn validate_atlas_text(text: &str) -> Result<(), SpineError> {
        text.parse::<Atlas>()
            .map(|_| ())
            .map_err(|error| SpineError::Runtime(format!("atlas: {error}")))
    }

    /// 枚举骨架里全部 clipping 附件（`(skin, slot, attachment)`）。
    ///
    /// 为什么要遍历而不是"解析到就报错"：`spine2d` 的 JSON/SKEL 解析器**完全支持**
    /// clipping 附件（能读、能建 skin），只是我们的 P1/P2 渲染管线不实现裁剪。因此错误
    /// 不能由解析器抛出，必须在解析之后按类型筛查——否则会误伤"能读但画不对"这一档。
    pub fn clipping_attachments(
        bytes: &[u8],
        binary: bool,
    ) -> Result<Vec<SpineAttachment>, SpineError> {
        let skeleton = parse_skeleton_data(bytes, binary)?;
        let slot_names: Vec<&str> = skeleton
            .get_slots()
            .iter()
            .map(|slot| slot.get_name())
            .collect();
        let mut found = Vec::new();
        for skin in skeleton.get_skins() {
            for entry in skin.get_attachments() {
                let AttachmentData::Clipping(clipping) = entry.get_attachment() else {
                    continue;
                };
                let slot = slot_names
                    .get(entry.get_slot_index())
                    .copied()
                    .unwrap_or("<unknown-slot>");
                found.push(SpineAttachment {
                    skin: skin.get_name().to_owned(),
                    slot: slot.to_owned(),
                    name: clipping.get_name().to_owned(),
                });
            }
        }
        Ok(found)
    }
}

/// 按扩展名判定走二进制还是 JSON 解析，再返回解析出的骨骼数据。
fn parse_skeleton_data(bytes: &[u8], binary: bool) -> Result<Arc<SkeletonData>, SpineError> {
    if binary {
        SkeletonData::from_skel_bytes(bytes).map_err(|error| SpineError::Runtime(error.to_string()))
    } else {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| SpineError::Runtime(format!("skeleton is not UTF-8: {error}")))?;
        SkeletonData::from_json_str(text).map_err(|error| SpineError::Runtime(error.to_string()))
    }
}

/// 一个骨架实例。事件队列由本层产出中性事件，宿主取走即清。
pub struct BackendInstance {
    skeleton: Skeleton,
    state: AnimationState,
    list: spine2d::DrawList,
    events: Vec<SpineEvent>,
    last_animation: String,
}

impl BackendInstance {
    pub fn new(data: &BackendData) -> Result<Self, SpineError> {
        // 引擎世界是 Y-up，与 Spine 默认一致；这里显式钉住，避免别的 crate 改全局后漂移。
        Bone::set_y_down(false);
        let mut skeleton = Skeleton::new(Arc::clone(&data.skeleton));
        skeleton.setup_pose();
        let state = AnimationState::new(AnimationStateData::new(Arc::clone(&data.skeleton)));
        Ok(Self {
            skeleton,
            state,
            list: spine2d::DrawList::default(),
            events: Vec::new(),
            last_animation: String::new(),
        })
    }

    pub fn set_animation(
        &mut self,
        data: &BackendData,
        name: &str,
        looped: bool,
    ) -> Result<(), SpineError> {
        if name.is_empty() {
            self.state.clear_track(0);
            self.last_animation.clear();
            return Ok(());
        }
        let animation = data
            .skeleton
            .find_animation(name)
            .ok_or_else(|| SpineError::AnimationNotFound(name.to_owned()))?;
        self.state.set_animation(0, animation, looped);
        self.last_animation = name.to_owned();
        // P1 事件口径：动画切换产生 start 事件，Lua 能真实取走。
        // Spine timeline 的自定义事件与 complete 事件留给 P2 接 listener。
        self.events.push(SpineEvent {
            kind: SpineEventKind::Start,
            name: name.to_owned(),
            animation: name.to_owned(),
        });
        Ok(())
    }

    pub fn set_skin(&mut self, data: &BackendData, name: &str) -> Result<(), SpineError> {
        let skin = data
            .skeleton
            .find_skin(name)
            .ok_or_else(|| SpineError::SkinNotFound(name.to_owned()))?;
        let _ = skin;
        self.skeleton.set_skin(Some(name));
        Ok(())
    }

    pub fn set_time_scale(&mut self, scale: f32) {
        self.state.set_time_scale(scale);
    }

    #[must_use]
    pub fn time_scale(&self) -> f32 {
        self.state.get_time_scale()
    }

    #[must_use]
    pub fn animation_name(&self) -> &str {
        &self.last_animation
    }

    pub fn clear(&mut self) {
        self.state.clear_track(0);
        self.events.clear();
        self.last_animation.clear();
        self.list.clear();
    }

    /// 推进一帧并产出中性网格。`z` 是本实体在世界的深度，缩放由 `scale` 施加到顶点。
    /// 非 Normal blend 保留在 draw 里但 P1 不渲染（由 GPU 侧明确计数/跳过）。
    pub fn advance(
        &mut self,
        data: &BackendData,
        dt: f32,
        scale: f32,
        z: f32,
        out: &mut SpineRenderOutput,
    ) -> Result<(), SpineError> {
        self.state.update(dt);
        self.skeleton.setup_pose();
        self.state.apply(&mut self.skeleton);
        self.skeleton.update(dt);
        self.skeleton
            .update_world_transform_with_physics(Physics::None);

        self.list.clear();
        spine2d::append_draw_list_with_atlas(&mut self.list, &self.skeleton, &data.atlas);
        if self.list.vertices.len() > SpineRenderOutput::MAX_VERTICES {
            return Err(SpineError::TooManyVertices(self.list.vertices.len()));
        }
        self.copy_to_output(data, scale, z, out);
        Ok(())
    }

    /// 把已生成的 `list` 同步到 `out`（顶点变换 + 索引 + draw 元数据）。
    ///
    /// 单独抽出来是为了性能归因（`advance_copy_only`），也让 `advance` 的"生成"与"搬运"
    /// 两段边界清楚。稳态下这段**不分配**：`Vec::clear` 保留容量，draw 槽位复用 `Arc`。
    fn copy_to_output(&self, data: &BackendData, scale: f32, z: f32, out: &mut SpineRenderOutput) {
        out.vertices.reserve(self.list.vertices.len());
        out.indices.reserve(self.list.indices.len());
        // 这里**不** `out.clear()`：`clear()` 把 len 归零，后面就没法复用已有 draw 的
        // `Arc<str>` 槽位了。改成"按需覆盖 + 尾部截断"，稳态下完全不碰分配器。
        out.vertices.clear();
        out.indices.clear();
        for vertex in &self.list.vertices {
            out.vertices.push(SpineVertex {
                position: [vertex.position[0] * scale, vertex.position[1] * scale, z],
                uv: vertex.uv,
                color: vertex.color,
            });
        }
        out.indices.extend_from_slice(&self.list.indices);
        // 逐帧把上游 `DrawList` 同步到我们的输出。
        //
        // 上游的 `Draw.texture_path` 是 `String`，而 `append_draw_list_with_atlas` 每次都
        // `page.texture_path.clone()`——一帧 20 个 draw 就是 20 次堆分配（P4 基准实测
        // 单角色每帧 181 次分配的主要来源）。我们改不动上游类型，但可以**复用自己这侧的
        // `Arc<str>`**：页路径在加载期已驻留，这里只做查表 + 引用计数自增。
        for (index, draw) in self.list.draws.iter().enumerate() {
            // 页表在加载期就建好了。查不到只有两种可能：atlas 缺这个 region（导出/配置错），
            // 或上游换了页名。两种情况都是**数据问题**，不该靠每帧现场拼字符串来掩盖——
            // 那样会变成"能跑、但每帧多 N 次分配，且可能静默贴错页"。
            //
            // 这里的兜底只保证不 panic（保持既有的"坏数据点名报错、不崩"口径），
            // 同时按 debug 断言把问题暴露给开发期：基准里一旦出现 `ours > 0`，
            // 就说明夹具/资产的页映射又坏了。
            let Some(texture) = data.page_paths.get(&draw.texture_path) else {
                debug_assert!(
                    false,
                    "atlas 页表缺少 `{}`：region 与图集页不匹配（会退化为每帧拼路径）",
                    draw.texture_path
                );
                out.draws.truncate(index);
                return;
            };
            let texture = texture.clone();
            let first_index = draw.first_index as u32;
            let index_count = draw.index_count as u32;
            let blend = map_blend(draw.blend);
            if let Some(slot) = out.draws.get_mut(index) {
                // 稳态：改写已有槽位，不 push（push 会增长 Vec，虽然摊还但仍有抖动）。
                slot.texture = texture;
                slot.first_index = first_index;
                slot.index_count = index_count;
                slot.blend = blend;
                slot.premultiplied_alpha = draw.premultiplied_alpha;
            } else {
                out.draws.push(SpineDraw {
                    texture,
                    first_index,
                    index_count,
                    blend,
                    premultiplied_alpha: draw.premultiplied_alpha,
                });
            }
        }
        out.draws.truncate(self.list.draws.len());
    }

    /// 只推进状态机与骨骼采样，不产 draw list（性能归因用，见 `player::advance_state_only`）。
    pub fn advance_state_only(&mut self, dt: f32) {
        self.state.update(dt);
        self.skeleton.setup_pose();
        self.state.apply(&mut self.skeleton);
        self.skeleton.update(dt);
        self.skeleton
            .update_world_transform_with_physics(Physics::None);
    }

    /// 只把已有 draw list 复制到输出（不推进状态机、不重建 list）。性能归因用。
    pub fn advance_copy_only(&mut self, data: &BackendData, out: &mut SpineRenderOutput) {
        self.copy_to_output(data, 1.0, 0.0, out);
    }

    /// 只重建 draw list，不推进状态机（性能归因用）。
    pub fn advance_draw_list_only(&mut self, data: &BackendData) {
        self.list.clear();
        spine2d::append_draw_list_with_atlas(&mut self.list, &self.skeleton, &data.atlas);
    }

    /// 取走并清空事件（与引擎既有的「事件取走即清」语义一致）。
    pub fn drain_events(&mut self) -> Vec<SpineEvent> {
        std::mem::take(&mut self.events)
    }
}

/// 把 atlas 页名解析成工程内相对路径。
///
/// 三条规则（与 Spine 官方 runtime 的页解析一致）：
/// 1. 页名已是绝对路径或已带目录时不动（有些工作流导出时就把目录写进 atlas）；
/// 2. `atlas_dir` 为空时原样返回；
/// 3. 否则拼成 `<atlas_dir>/<页名>`，并统一 `/` 分隔符（工程路径一律正斜杠）。
fn resolve_page_path(atlas_dir: &str, page: &str) -> String {
    if atlas_dir.is_empty() || page.contains('/') || page.contains('\\') {
        return page.replace('\\', "/");
    }
    format!("{atlas_dir}/{page}")
}

/// 规范化 atlas 目录：去尾部分隔符、统一正斜杠。
fn normalize_atlas_dir(dir: &str) -> String {
    let normalized = dir.replace('\\', "/");
    normalized.trim_end_matches('/').to_owned()
}

fn map_blend(blend: BlendMode) -> SpineBlend {
    match blend {
        BlendMode::Normal => SpineBlend::Normal,
        BlendMode::Additive => SpineBlend::Additive,
        BlendMode::Multiply => SpineBlend::Multiply,
        BlendMode::Screen => SpineBlend::Screen,
    }
}
