//! `RuntimeScene` 的空间传播、物理权威、可见性掩码与并行度/裁剪开关
//! （WP12 阶段 3a 从 `mod.rs` 原样搬出）。
//!
//! 这里的开关成对出现（`set_*` + 读取器）。`sprite_parallelism` 的读取器按行段正好落在两条
//! collider 调试方法之间，因此单独成段带走——与 `set_sprite_parallelism` 同模块，配对不用跨文件找。

use rustlua_camera::camera::{Camera2D, RenderView, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::SpatialReport;
use rustlua_ecs::particle::{ParticleCpuReason, ParticlePlacement, SceneParticleEmitter};
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_ecs::world::World;
use rustlua_infra::par::Parallelism;
use rustlua_math::math3d::Mat4;
use rustlua_physics::physics::PhysicsAuthority;
use rustlua_render::runtime_gpu::ParticleRenderer;
use rustlua_scene::scene::SceneEntityId;

use super::{ParticleDispatchReport, ParticlePlacementReport, PhysicsStageOrder, RuntimeScene};

/// 按运行时句柄读实体**自己的**局部变换：`Transform3D` 优先，其次能无损压平的 2D
/// [`Transform`]；两者都没有 ⇒ `None`。
///
/// 这是 [`RuntimeScene::local_transform_of`] 的实现体，宿主的批量读数
/// （`RuntimeScene::scene_entity_local_transforms`）走同一条回退链——两处各写一次
/// "3D 优先"的判断，迟早给出两个不一样的答案。
pub(super) fn local_transform_in(
    world: &World,
    entity: EntityId,
) -> Option<rustlua_ecs::spatial_transform::LocalTransform> {
    if let Ok(Some(transform)) = world.get::<Transform3D>(entity) {
        return Some(rustlua_ecs::spatial_transform::LocalTransform::from_3d(
            *transform,
        ));
    }
    world
        .get::<Transform>(entity)
        .ok()
        .flatten()
        .and_then(|transform| {
            rustlua_ecs::spatial_transform::LocalTransform::from_2d(*transform).ok()
        })
}

impl RuntimeScene {
    /// docs/54 §4.1：按**场景实体 id** 读运行时局部变换，**用统一的 `LocalTransform` 表示**。
    ///
    /// 运行时 ECS 里存的是 `Transform`（2D）与 `Transform3D`（3D）——它们是**序列化边界格式**，也是
    /// 2D 快路径（平面 f32 弧度比四元数便宜，10k 精灵档实测过）。跨空间、跨系统要"一个局部变换"
    /// 时就该走这里：`LocalTransform` 是引擎内**唯一的规范表示**（组合、反解、换父、迁移都基于它，
    /// 见 docs/40 WP2 与 docs/54 §8-②），这个读法把它暴露出来，而不是让每个调用方各写一次转换。
    #[must_use]
    pub fn local_transform_of(
        &self,
        entity: SceneEntityId,
    ) -> Option<rustlua_ecs::spatial_transform::LocalTransform> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script
            .with_world(|world| local_transform_in(world, runtime))
    }

    /// docs/54 §4.1：把统一的 `LocalTransform` 写回该实体**自己的空间**。
    ///
    /// 三条明确的出口（都不猜）：
    /// - 实体有 `Transform3D` ⇒ 原样写回；
    /// - 实体只有 2D `Transform` ⇒ 只在**真能无损压平**时才写（四元数只有 z/w 分量、z 位移为 0、
    ///   z 缩放为 1），否则**报错**——手抄一个 XY 会把旋转与缩放悄悄丢掉（docs/54 §1.2）；
    /// - 两种都没有 ⇒ `Ok(false)`（没有局部变换可更新，调用方自己决定要不要新建组件）。
    ///
    /// 拆分批 8：写成功即视为**相机相关的世界变更**（`pose=transform` 的相机可能就在这个实体或
    /// 它的父链上），因此作废本帧定下的 3D 视图。
    pub fn set_local_transform(
        &mut self,
        entity: SceneEntityId,
        value: rustlua_ecs::spatial_transform::LocalTransform,
    ) -> anyhow::Result<bool> {
        let Some(runtime) = self.script.scene_entity_handle(entity) else {
            return Ok(false);
        };
        let written = self
            .script
            .with_world_mut(|world| -> anyhow::Result<bool> {
                if world.get::<Transform3D>(runtime).ok().flatten().is_some() {
                    let transform = Transform3D {
                        position: value.translation,
                        rotation: value.rotation,
                        scale: value.scale,
                    };
                    world.insert(runtime, transform)?;
                    return Ok(true);
                }
                if world.get::<Transform>(runtime).ok().flatten().is_some() {
                    let flat =
                        rustlua_ecs::spatial_transform::plane_local(value).ok_or_else(|| {
                            anyhow::anyhow!(
                                "this entity only has a 2D Transform; the value is not planar \
                         (tilt, z offset or z scale) — flattening would drop it"
                            )
                        })?;
                    world.insert(runtime, flat)?;
                    return Ok(true);
                }
                Ok(false)
            })?;
        if written {
            self.invalidate_frame_view_3d();
        }
        Ok(written)
    }

    /// WP2 ④：按**场景实体 id** 读世界变换（2D）。
    ///
    /// 编辑器拾取包围框必须与渲染用同一份位置：场景视图渲染走 `vertices()`（父链组合后的
    /// 世界变换），若包围框仍读文档里的局部值，带父子关系的实体会**框在人不在的地方**。
    /// 继承未生效（没有 `Parent`）时返回的正是局部值，因此调用方无需分支。
    pub fn world_transform_2d_of(&self, entity: SceneEntityId) -> Option<Transform> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script
            .with_world(|world| rustlua_ecs::hierarchy::world_transform_2d(world, runtime))
            .ok()
            .flatten()
    }

    /// WP2 ④：按**场景实体 id** 读世界变换（3D）。
    pub fn world_transform_3d_of(&self, entity: SceneEntityId) -> Option<Transform3D> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script
            .with_world(|world| rustlua_ecs::hierarchy::world_transform_3d(world, runtime))
            .ok()
            .flatten()
    }

    /// 设置 2D 相机（编辑器场景视图用，匹配编辑器 pan/zoom）。
    pub fn set_camera_2d(&mut self, camera: Camera2D) {
        self.camera = camera;
    }

    /// WP2 ④ 回滚档：`parent` 是否按**局部空间**解释（默认关闭 = 旧行为）。打开后每帧把
    /// 局部变换沿父链组合成世界变换（`WorldTransform` / `WorldTransform3D`），渲染与 Lua
    /// 读到的位置因此是**世界**位置；关掉时这些组件会被清掉，消费者退回局部值。
    pub fn set_spatial_inheritance(&mut self, enabled: bool) {
        self.spatial_inheritance = enabled;
        self.propagate_spatial_now();
    }

    /// 当前空间继承开关。
    #[must_use]
    pub const fn spatial_inheritance(&self) -> bool {
        self.spatial_inheritance
    }

    /// docs/54 §10 批 R：开关"输入没变就跳过传播"。返回先前是否启用。
    ///
    /// - `true`（默认）：静态层级场景整趟跳过传播（世界值仍是上一帧算出来的那一份）。
    /// - `false`：**回滚档**——每帧全量传播，与批 R 之前逐字节一致。
    ///
    /// 跳过只依据"局部真源指纹 + 是否启用继承"，因此任何局部值/父子边/组件增删都会让下一帧退回
    /// 全量传播；指纹不参与渲染，也不是第二份真值。
    pub fn set_spatial_skip_unchanged(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_skip_unchanged;
        self.spatial_skip_unchanged = enabled;
        if !enabled {
            // 关掉时清掉指纹：打开开关后第一帧必须重新全量传播一次（不能靠旧指纹跳过）。
            self.spatial_fingerprint = None;
        }
        previous
    }

    /// 当前是否启用"输入没变就跳过传播"。
    #[must_use]
    pub const fn spatial_skip_unchanged(&self) -> bool {
        self.spatial_skip_unchanged
    }

    /// 自上次取走以来"因为输入没变而跳过传播"的帧数（取走即清零）。
    pub fn take_spatial_skipped_frames(&mut self) -> u64 {
        std::mem::take(&mut self.spatial_skipped_frames)
    }

    /// 累计跳过帧数（不清零）。
    #[must_use]
    pub const fn spatial_skipped_frames(&self) -> u64 {
        self.spatial_skipped_frames
    }
    /// docs/54 §4.1：**场景声明的** `parent` 语义（未声明 ⇒ 组织关系）。宿主开关可以覆盖生效值，
    /// 但这一份永远是场景自己说的，因此"运行时为什么继承/不继承"有据可查。
    #[must_use]
    pub const fn spatial_mode(&self) -> rustlua_scene::scene_space::SpatialMode {
        self.spatial_mode
    }

    /// 本帧是否真的在传播世界变换（开关打开**且**场景里存在父子关系）。
    #[must_use]
    pub const fn spatial_active(&self) -> bool {
        self.spatial_active
    }

    /// 最近一次空间传播的诊断（环、无法无损表达为 TRS 的父子组合）。空表示全部建立成功。
    #[must_use]
    pub fn last_spatial_report(&self) -> &SpatialReport {
        &self.last_spatial_report
    }

    /// WP7：声明谁有权写 `Transform`（默认 `Physics`）。见 [`PhysicsAuthority`]。
    pub const fn set_physics_authority(&mut self, authority: PhysicsAuthority) {
        self.physics_authority = authority;
    }

    /// 当前物理权威。
    #[must_use]
    pub const fn physics_authority(&self) -> PhysicsAuthority {
        self.physics_authority
    }

    /// WP7 回滚档：阶段顺序。`Legacy` 回到"物理步进即回写"的单系统顺序。
    pub const fn set_physics_stage_order(&mut self, order: PhysicsStageOrder) {
        self.physics_stage_order = order;
    }

    /// 当前阶段顺序。
    #[must_use]
    pub const fn physics_stage_order(&self) -> PhysicsStageOrder {
        self.physics_stage_order
    }

    /// WP6 回滚档：世界 Sprite 是否走带深度的通道（默认关闭）。
    pub fn set_world_sprite_depth(&mut self, enabled: bool) {
        self.world_sprite_depth = enabled;
    }

    /// 当前世界 Sprite 深度通道开关。
    #[must_use]
    pub const fn world_sprite_depth(&self) -> bool {
        self.world_sprite_depth
    }

    /// WP6：本帧世界 Sprite 的投影矩阵（宿主喂给 `Renderer::set_world_sprite_camera`）。
    /// 关闭时返回 `None`（渲染器不会用到它）。
    ///
    /// docs/54 §8-③/④：宿主**声明了主视图**时（混合 2D/3D 场景）世界 Sprite 会改用主视图
    /// 的那份矩阵与深度附件，本函数的值只作为纯 2D 画面的回退。
    #[must_use]
    pub fn sprite_view_projection(&self, width: u32, height: u32) -> Option<Mat4> {
        if !self.world_sprite_depth {
            return None;
        }
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32).ok()?;
        // 深度范围取正交视图的默认区间：精灵默认 `z = 0` 落在深度**中间**，向 -z 靠近相机、
        // 向 +z 远离（与 `RenderView::overlay_2d` / `main_2d` 用的是同一组常量）。
        self.camera
            .view_projection(
                viewport,
                rustlua_camera::camera::ORTHO_NEAR,
                rustlua_camera::camera::ORTHO_FAR,
            )
            .ok()
    }

    /// 场景里是否有 3D 内容（网格可见物）。有 ⇒ 宿主会给这一帧声明 3D 主视图，世界通道的
    /// 可见域因此是 3D 视图而不是 2D 相机（docs/54 §8-④：宁可保守不裁剪，也绝不误丢）。
    #[must_use]
    pub fn world_view_has_3d_content(&self) -> bool {
        self.script.with_world(|world| {
            !world
                .entities_with::<rustlua_render::mesh3d::MeshRenderer>()
                .is_empty()
        })
    }

    /// docs/54 §8-①：推进**场景声明的**粒子发射器。
    ///
    /// 每个带 `SceneParticleEmitter` 的实体按 `rate × dt` 累计发射，发射位置取它的**世界**位置
    /// （父链下的子实体因此跟着父一起动）。累计器写回组件，因此帧率变化不改变平均发射量。
    ///
    /// 返回本步发出的粒子数（诊断/验收用）。发射本身复用 Lua `particles.burst` 的同一实现。
    ///
    /// P3-R07-1：这一步同时判定每个发射器的**落点**（[`rustlua_ecs::particle::ParticleEmitter::placement`]）。
    /// GPU 派发要到 R07-2 才接线，因此**今天所有粒子照旧在 CPU 池里生成**——回退不丢粒子、
    /// 不黑屏；落点分布记在 [`RuntimeScene::last_particle_placement`]，每种回退原因喊一层日志。
    pub fn step_scene_emitters(&mut self, delta_seconds: f32) -> usize {
        let compute_available = self.particle_compute_available;
        let (pending, placement) = self.script.with_world_mut(|world| {
            let mut pending = Vec::new();
            let mut placement = ParticlePlacementReport::default();
            for id in world.entities_with::<SceneParticleEmitter>() {
                let Some(mut scene) = world
                    .get::<SceneParticleEmitter>(id)
                    .ok()
                    .flatten()
                    .copied()
                else {
                    continue;
                };
                // 没有变换的发射器没有位置可发，跳过（与"世界变换缺席"的消费端约定一致）。
                let Some(position) = rustlua_ecs::hierarchy::world_transform_2d(world, id)
                    .ok()
                    .flatten()
                    .map(|transform| transform.position)
                else {
                    continue;
                };
                let count = scene.take_steps(delta_seconds);
                // 累计器必须写回：否则每步都从 0 开始，低速率发射器永远发不出粒子。
                let _ = world.insert(id, scene);
                if count == 0 {
                    continue;
                }
                let mut emitter = scene.emitter;
                emitter.count = count;
                placement.record(emitter.placement(compute_available));
                pending.push((position, emitter));
            }
            (pending, placement)
        });
        self.note_particle_placement(placement);
        let emitted = pending.len();
        for (position, emitter) in pending {
            self.script.burst_particles(position, &emitter);
        }
        emitted
    }

    /// 记下本步落点分布，并把**新出现的**回退原因各喊一层日志。
    fn note_particle_placement(&mut self, placement: ParticlePlacementReport) {
        self.last_particle_placement = placement;
        self.note_particle_reasons(placement.fallback_bits, "场景发射器");
    }

    /// 每种回退原因只喊一层：`particle_warned` 是位图，同一原因不因换发射器、换帧或换
    /// 通道（场景发射器 / burst 派发）而刷屏。
    fn note_particle_reasons(&mut self, bits: u32, source: &str) {
        let new_bits = bits & !self.particle_warned;
        self.particle_warned |= bits;
        for reason in ParticleCpuReason::WARNABLE {
            if new_bits & (1 << reason.index()) == 0 {
                continue;
            }
            tracing::warn!(
                target: "particles",
                reason = reason.as_str(),
                source,
                "[particles] particle_fallback=cpu：{}要了 mode=gpu，本步在 CPU 上发射",
                source
            );
        }
    }

    /// 最近一步场景发射器的落点分布。
    #[must_use]
    pub const fn last_particle_placement(&self) -> ParticlePlacementReport {
        self.last_particle_placement
    }

    /// 已经喊过一层的回退原因位图（`ParticleCpuReason::index` 为位）。
    ///
    /// 渲染器侧的派发门要断言"全走 GPU ⇒ 一条回退日志都不该喊"，而那位图是宿主状态；
    /// 放成读取器而不是把字段公开，位图怎么编码就不会漏进调用方。
    #[must_use]
    pub const fn particle_warned_bits(&self) -> u32 {
        self.particle_warned
    }

    /// P3-R07-1：把渲染器的 compute 能力告知运行时。
    ///
    /// 判定 [`rustlua_ecs::particle::ParticleEmitter::placement`] 时要用它，而 ECS 层不该知道设备——宿主（`GameApp`、
    /// 编辑器 Play 预览）拿到渲染器时调一次即可。不调用则按"没有 compute"处理：`mode=gpu`
    /// 的场景照旧回 CPU，只是报告里写 `no_compute` 而不是 `not_wired`。
    pub fn set_particle_compute_available(&mut self, compute_available: bool) {
        self.particle_compute_available = compute_available;
    }

    /// P3-R07-2：把脚本侧 `mode=gpu` 的爆发交给渲染器的 GPU 粒子池，并步进这个池。
    ///
    /// 调用点是宿主的每帧循环：`run_script_steps` **之后**、渲染**之前**——compute 必须在
    /// 本帧图形 pass 之前提交（见 `dispatch_compute` 的提交点约定）。
    ///
    /// 三条保证，各有单测钉住：
    /// - **不丢粒子**：判定不允许（`ParticleEmitter::placement`）或渲染器拒绝的每一批，都用
    ///   **同一份参数**退回 CPU 池，照旧被模拟、照旧被画。
    /// - **Lua 读不到 GPU 粒子**：渲染器收下的一批不进 CPU 池，`particles.count` 因此只数
    ///   玩法可见的那些（GPU 粒子是视觉层）。
    /// - **不谎报**：`gpu` 只在渲染器返回 `Ok` 之后加一；能力位每帧从渲染器重取，设备中途
    ///   丢 compute 也不会继续报成 GPU。
    #[must_use]
    pub fn dispatch_gpu_particle_bursts(
        &mut self,
        renderer: &mut impl ParticleRenderer,
        delta_seconds: f32,
    ) -> ParticleDispatchReport {
        let compute_available = renderer.compute_enabled();
        // 能力位每帧刷新（`set_particle_compute_available` 是宿主启动时的一次性下推；
        // 设备重建/后端切换后只有这里能把它拉回来）。
        self.particle_compute_available = compute_available;
        let bursts = self.script.take_gpu_bursts();
        let mut report = ParticleDispatchReport {
            requested: bursts.len(),
            deferred: std::mem::take(&mut self.particle_deferred),
            overflow: self.script.take_gpu_burst_overflow(),
            ..ParticleDispatchReport::default()
        };
        let mut fallback_bits = 0u32;
        let mut note_fallback = |report: &mut ParticleDispatchReport, reason: ParticleCpuReason| {
            report.fallback += 1;
            report.fallback_reason.get_or_insert(reason);
            fallback_bits |= 1 << reason.index();
        };
        for (position, emitter) in bursts {
            match emitter.placement(compute_available) {
                ParticlePlacement::Cpu(reason) => {
                    note_fallback(&mut report, reason);
                    self.script.push_particles_cpu(position, &emitter);
                }
                ParticlePlacement::Gpu => {
                    // 超过 u32 的 count 会饱和成 `u32::MAX`，由渲染器的容量校验整批拒绝。
                    let capacity = u32::try_from(emitter.count).unwrap_or(u32::MAX);
                    match renderer.push_particle_burst([position.x, position.y], emitter, capacity)
                    {
                        Ok(()) => report.gpu += 1,
                        Err(error) => {
                            tracing::warn!(
                                target: "particles",
                                error = %error,
                                count = emitter.count,
                                "[particles] GPU burst 派发被拒：整批退回 CPU 池"
                            );
                            note_fallback(&mut report, ParticleCpuReason::DispatchRejected);
                            self.script.push_particles_cpu(position, &emitter);
                        }
                    }
                }
            }
        }
        // 队列满时脚本层已经就地回退，这里只把它记进同一个原因位图。
        if report.overflow > 0 {
            fallback_bits |= 1 << ParticleCpuReason::DispatchRejected.index();
        }
        // 上一帧入队却没人来取：这批已经在本帧开头倒回 CPU 池（保底路径，见 `flush_gpu_bursts`）。
        if report.deferred > 0 {
            report.fallback += report.deferred;
            report
                .fallback_reason
                .get_or_insert(ParticleCpuReason::NotWired);
            fallback_bits |= 1 << ParticleCpuReason::NotWired.index();
        }
        // 空池不调用：步进空池是 `NotConfigured`，那不是错误而是"没这东西"。
        if renderer.particle_systems() > 0 {
            match renderer.step_particles(delta_seconds) {
                Ok(step) => {
                    report.systems = step.systems;
                    report.recycled = step.recycled;
                    report.steps = step.steps;
                }
                Err(error) => {
                    tracing::warn!(target: "particles", error = %error, "[particles] GPU 粒子步进失败");
                }
            }
        }
        report.gpu_particles = renderer
            .spawned_particles()
            .map_or(0, |count| count as usize);
        self.note_particle_reasons(fallback_bits, "burst 派发");
        self.last_particle_dispatch = report;
        report
    }

    /// 最近一次 burst 派发通道的读数（诊断与验收用）。
    #[must_use]
    pub const fn last_particle_dispatch(&self) -> ParticleDispatchReport {
        self.last_particle_dispatch
    }

    /// 把脚本侧还没被派发的 GPU burst 倒回 CPU 池（P3-R07-2 的保底，宿主每帧开头调用）。
    ///
    /// 没有这一步，"宿主没接派发通道"的工程里那些 `mode=gpu` 的粒子会卡在队列里——既不模拟
    /// 也不画，等于丢粒子。倒回后它们最多晚一帧出现，语义与 CPU 档一致。
    pub(super) fn requeue_stale_gpu_bursts(&mut self) {
        self.particle_deferred += self.script.flush_gpu_bursts();
    }

    /// 最近一步由场景声明的发射器发出的粒子数。
    #[must_use]
    pub const fn last_scene_emission(&self) -> usize {
        self.last_scene_emission
    }

    /// docs/54 §8-②：最近一次场景加载里**坏的 `parent` 边**（悬空父/自环）的可读描述
    /// （空 = 每条 `parent` 都指向同一文档里的实体且不是自己）。
    #[must_use]
    pub fn scene_parent_issues(&self) -> Vec<String> {
        self.script.scene_parent_issues()
    }

    /// WP7：最近一步 `physics_sync` 同步的刚体数 `(2D, 3D)`——0 表示这一步没做步前同步
    /// （默认 authority=Physics，或回滚档 `Legacy`）。
    #[must_use]
    pub const fn last_physics_sync(&self) -> (usize, usize) {
        self.last_physics_sync
    }

    /// docs/54 §4.1：最近一步每实体脚本调用的实体数（诊断/验收用）。
    #[must_use]
    pub const fn last_entity_script_calls(&self) -> usize {
        self.last_entity_script_calls
    }

    /// docs/54 §4.2：声明本帧两个视图的**可见遮罩**（`world` 对应 `FrameViews::main.visibility`、
    /// `overlay` 对应 `FrameViews::overlay.visibility`）。传同一个 `FrameViews` 值给渲染器与本函数，
    /// 因此不存在第二份真源；不调用则全可见（既有行为与像素不变）。
    pub const fn set_view_visibility(&mut self, world: u32, overlay: u32) {
        self.view_world_mask = world;
        self.view_overlay_mask = overlay;
    }

    /// 本帧两个视图的可见遮罩。
    #[must_use]
    pub const fn view_visibility(&self) -> (u32, u32) {
        (self.view_world_mask, self.view_overlay_mask)
    }

    /// 最近一次提取里被视图遮罩拦掉的实体数（世界遮罩与 overlay 遮罩合计）。
    ///
    /// 取走即清空（与 `take_script_micros` 同一语义），因此调用方拿到的是"自上次取走以来"的数量。
    pub fn take_view_filtered(&self) -> usize {
        self.view_filtered.replace(0)
    }

    /// 看一眼当前累计值（不清空）。
    #[must_use]
    pub fn view_filtered(&self) -> usize {
        self.view_filtered.get()
    }

    /// 传播一次世界变换（幂等）。提取入口（`vertices` / `scene_3d`）与本函数的调用点
    /// 都会走它，因此"这一帧看到的世界位置"永远来自同一次传播。
    pub fn propagate_spatial_now(&mut self) {
        let enabled = self.spatial_inheritance;
        // 只有**真的存在层级**时才传播：平面场景（每个实体都没有 `Parent`）里世界值恒等于局部值，
        // 传播纯属白做——而提取路径是按这个布尔量选组件遍历的，所以它必须与实际写入的组件一致。
        //
        // docs/54 §10 批 R：传播的输入只有四样（参与集合、局部值、父子边、是否启用）。前三样哈希成
        // 一个指纹，**与上一帧相同就整趟跳过**——静态层级场景因此不再付"每实体 3 次组件查找 + 矩阵
        // 组合 + 写入"那三成帧成本。指纹只是"上一帧算出来的世界值还有效吗"的判据，不参与渲染，
        // 也不是第二份真值；任何一位变了（组件增删/实体重生/换父/局部值改动）都会退回全量传播。
        //
        // docs/54 §10 批 S：指纹只说"**有东西**变了"，增量路径再说"**哪些**实体变了"，于是只重算
        // 那些实体的子树。两级判据分工明确：一级是最便宜的哈希（静态帧 0.48 ms/万实体），二级是
        // 逐实体精确快照比较（只在指纹变化时才付）。回滚档是 `set_spatial_incremental(false)`。
        let fingerprint = self
            .script
            .with_world(rustlua_ecs::hierarchy::local_fingerprint);
        let (_, _, parents) = fingerprint;
        let active_now = enabled && parents > 0;
        if self.spatial_skip_unchanged
            && self.spatial_fingerprint == Some(fingerprint)
            && self.spatial_active == active_now
        {
            self.spatial_skipped_frames = self.spatial_skipped_frames.saturating_add(1);
            self.spatial_path_counts[0] = self.spatial_path_counts[0].saturating_add(1);
            self.last_spatial_path = rustlua_ecs::hierarchy::SpatialPath::Skipped;
            self.last_spatial_recomputed = 0;
            return;
        }
        let incremental = self.spatial_incremental_enabled;
        let (outcome, active) = {
            // 分字段借用：`script` 与增量状态是两个不相干的字段。
            let script = &mut self.script;
            let state = &mut self.spatial_incremental;
            let outcome = script.with_world_mut(|world| {
                if incremental {
                    rustlua_ecs::hierarchy::propagate_spatial_incremental(world, state, active_now)
                } else {
                    rustlua_ecs::hierarchy::SpatialOutcome {
                        report: rustlua_ecs::hierarchy::propagate_spatial(world, active_now),
                        path: rustlua_ecs::hierarchy::SpatialPath::Full,
                        recomputed: 0,
                    }
                }
            });
            (outcome, active_now)
        };
        let report = outcome.report;
        self.spatial_fingerprint = Some(fingerprint);
        self.spatial_active = active;
        self.last_spatial_path = outcome.path;
        self.last_spatial_recomputed = outcome.recomputed;
        let slot = match outcome.path {
            rustlua_ecs::hierarchy::SpatialPath::Skipped => 0,
            rustlua_ecs::hierarchy::SpatialPath::Full => 1,
            rustlua_ecs::hierarchy::SpatialPath::Incremental => 2,
        };
        self.spatial_path_counts[slot] = self.spatial_path_counts[slot].saturating_add(1);
        self.spatial_recomputed_total = self
            .spatial_recomputed_total
            .saturating_add(outcome.recomputed as u64);
        if !report.diagnostics.is_empty() && self.last_spatial_report.diagnostics.is_empty() {
            for diagnostic in &report.diagnostics {
                tracing::warn!(
                    target: "scene",
                    entity = ?diagnostic.entity,
                    reason = ?diagnostic.reason,
                    "[scene] 空间继承未能建立：该实体退回局部值（不做近似）"
                );
            }
        }
        self.last_spatial_report = report;
    }

    /// docs/54 §10 批 S：**逐实体增量传播**的开关（默认开启）。返回前一个值。
    ///
    /// 关掉就是回滚档：指纹变化后一律全量传播（与批 S 之前逐字节一致），世界值不变、只是更慢。
    /// 两个方向都会清空缓存，因此重新打开后的第一帧必定全量传播一次。
    pub fn set_spatial_incremental(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_incremental_enabled;
        self.spatial_incremental_enabled = enabled;
        self.spatial_incremental.reset();
        previous
    }

    /// 当前是否启用逐实体增量传播。
    #[must_use]
    pub const fn spatial_incremental(&self) -> bool {
        self.spatial_incremental_enabled
    }

    /// 最近一次传播走的路径（`Skipped` / `Full` / `Incremental`）。
    #[must_use]
    pub const fn last_spatial_path(&self) -> rustlua_ecs::hierarchy::SpatialPath {
        self.last_spatial_path
    }

    /// 最近一次传播**真正重算**的实体数（增量路径 = 脏子树大小）。
    #[must_use]
    pub const fn last_spatial_recomputed(&self) -> usize {
        self.last_spatial_recomputed
    }

    /// 累计的三条路径调用次数 `[跳过, 全量, 增量]`（不清零）。
    ///
    /// 一步里传播会被调用多次（步末 + `spatial_propagate` 帧系统），因此"最后一帧走了哪条路"
    /// 需要看计数而不是看 `last_spatial_path`。
    #[must_use]
    pub const fn spatial_path_counts(&self) -> [u64; 3] {
        self.spatial_path_counts
    }

    /// 取走三条路径的调用次数（取走即清零）。
    pub fn take_spatial_path_counts(&mut self) -> [u64; 3] {
        std::mem::take(&mut self.spatial_path_counts)
    }

    /// 累计"真正重算的实体数"（不清零）。
    #[must_use]
    pub const fn spatial_recomputed_total(&self) -> u64 {
        self.spatial_recomputed_total
    }

    /// 取走"真正重算的实体数"（取走即清零）。**这是逐实体增量的判据**：一帧里只挪一个叶子时，
    /// 一整个窗口里重算的实体数应该等于帧数，而不是帧数 × 实体数。
    pub fn take_spatial_recomputed_total(&mut self) -> u64 {
        std::mem::take(&mut self.spatial_recomputed_total)
    }

    /// 当前 2D 相机中心（世界坐标）。编辑器 Scene 视图用它定位设计分辨率包围框，
    /// 使包围框与游戏取景范围一致（默认 `DEFAULT_2D_CAMERA_CENTER`）。
    pub fn camera_center_2d(&self) -> Vec2 {
        self.camera.center
    }

    /// 当前 2D 相机。
    #[must_use]
    pub const fn camera_2d(&self) -> Camera2D {
        self.camera
    }

    /// WP5：本帧的 2D overlay 视图（相机 + 视口 + 清屏 + 顺序的唯一说法）。
    ///
    /// 与 `vertices(width, height)` **同源**（同一个 `self.camera`），因此渲染器不需要
    /// 再自己从别处拼一个相机；宿主把两份视图交给 `Renderer::set_frame_views` 即可。
    pub fn render_view_2d(&self, width: u32, height: u32) -> RenderView {
        // 窗口最小化时宽高可能是 0；`Viewport::new` 拒绝非正数，这里退到 1×1。
        let viewport =
            Viewport::new(width.max(1) as f32, height.max(1) as f32).unwrap_or(Viewport {
                width: 1.0,
                height: 1.0,
            });
        RenderView::overlay_2d(self.camera, viewport)
    }

    /// 设置 Canvas 空间 UI 的参考分辨率（设计分辨率）。同步给 Lua 侧，
    /// 保证渲染与按钮命中测试（`process_ui_clicks`）用同一参考。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        self.ui_reference = reference;
        self.script.set_ui_reference(reference);
    }

    /// 是否渲染屏幕空间 UI 覆盖层。编辑器场景视图置 false（UI 由 egui 在
    /// 设计分辨率包围框内绘制）；游戏预览保持 true。
    pub fn set_render_ui_overlay(&mut self, enabled: bool) {
        self.render_ui_overlay = enabled;
    }

    /// 设置每精灵顶点生成的并行度（WP8）。`Parallelism::SEQUENTIAL` 即回滚单线程；
    /// 两条路径的输出逐字节相同，因此这是一个纯粹的性能开关，不是行为开关。
    pub fn set_sprite_parallelism(&mut self, parallelism: Parallelism) {
        self.sprite_parallelism = parallelism;
    }

    /// P1-③：系统级数据并行开关（粒子积分等系统内纯数据子任务）。默认取检测到的
    /// 核数；`Parallelism::SEQUENTIAL` 回滚单线程——输出逐位相同（见 `particle` 的测试）。
    pub fn set_system_parallelism(&mut self, parallelism: Parallelism) {
        self.system_parallelism = parallelism;
    }

    /// 当前系统级并行开关。
    #[must_use]
    pub const fn system_parallelism(&self) -> Parallelism {
        self.system_parallelism
    }

    /// P1-①：渲染级空间索引（视口候选预筛）开关。返回旧值（回滚档）；关闭时清掉
    /// 网格缓存（下次打开重建）。与 `set_spatial_index_cell_size` 配套。
    pub fn set_spatial_index(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_index;
        self.spatial_index = enabled;
        if !enabled {
            self.spatial_grid = None;
            self.spatial_index_fingerprint = None;
        }
        previous
    }

    /// P1-①：空间索引网格 cell 边长（世界单位）。返回旧值；只影响性能不影响正确性。
    pub fn set_spatial_index_cell_size(&mut self, cell_size: f32) -> f32 {
        let previous = self.spatial_index_cell_size;
        self.spatial_index_cell_size = cell_size;
        previous
    }

    /// WP4 ④：视口外瓦片剔除开关（默认开启）。关掉即"整层全画"，输出像素必须与开启时
    /// **逐字节相同**——这是该优化的回滚档，也是它正确性的对拍基准。
    pub fn set_tilemap_culling(&mut self, enabled: bool) {
        self.tilemap_culling = enabled;
    }

    /// 当前视口外瓦片剔除开关。
    #[must_use]
    pub const fn tilemap_culling(&self) -> bool {
        self.tilemap_culling
    }

    /// WP6：视口外精灵剔除开关（默认开启）。关掉即"所有精灵都生成顶点"，像素必须与开启时
    /// **逐字节相同**——这是回滚档，也是正确性的对拍基准。
    pub fn set_sprite_culling(&mut self, enabled: bool) {
        self.sprite_culling = enabled;
    }

    /// 当前视口外精灵剔除开关。
    #[must_use]
    pub const fn sprite_culling(&self) -> bool {
        self.sprite_culling
    }

    /// 当前每精灵顶点生成的并行度。
    #[must_use]
    pub const fn sprite_parallelism(&self) -> Parallelism {
        self.sprite_parallelism
    }
}
