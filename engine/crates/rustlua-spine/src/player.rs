//! 中性播放层：动画状态、事件与生命周期。只依赖 `backend` 的中性输出。

use std::sync::Arc;

use crate::{
    backend::{BackendData, BackendInstance},
    model::{SpineError, SpineEvent, SpineRenderOutput, SpineSkeletonComponent},
};

/// 资源侧的不可变骨骼数据。多个实体可共享同一份 `Arc`，各自持有独立骨架实例。
#[derive(Clone)]
pub struct SpineData {
    inner: Arc<BackendData>,
}

impl SpineData {
    /// `atlas_dir` 是 atlas 文件的工程内目录（页名相对它解析，见 `BackendData::from_bytes`）。
    pub fn from_bytes(
        skeleton_bytes: &[u8],
        binary: bool,
        atlas_text: &str,
        spine_version: &str,
        atlas_dir: &str,
    ) -> Result<Self, SpineError> {
        Ok(Self {
            inner: Arc::new(BackendData::from_bytes(
                skeleton_bytes,
                binary,
                atlas_text,
                spine_version,
                atlas_dir,
            )?),
        })
    }

    #[must_use]
    pub fn animation_names(&self) -> &[String] {
        self.inner.animations()
    }

    #[must_use]
    pub fn skin_names(&self) -> &[String] {
        self.inner.skins()
    }

    pub fn instantiate(
        &self,
        component: &SpineSkeletonComponent,
    ) -> Result<SpinePlayer, SpineError> {
        let mut instance = BackendInstance::new(&self.inner)?;
        if !component.skin.is_empty() && component.skin != "default" {
            instance.set_skin(&self.inner, &component.skin)?;
        }
        if !component.animation.is_empty() {
            instance.set_animation(&self.inner, &component.animation, component.looped)?;
        }
        instance.set_time_scale(component.speed);
        Ok(SpinePlayer {
            data: Arc::clone(&self.inner),
            instance,
            component: component.clone(),
            output: SpineRenderOutput::default(),
        })
    }
}

/// 运行时骨架 + 玩家状态。作为 ECS 组件挂在实体上。
pub struct SpinePlayer {
    data: Arc<BackendData>,
    instance: BackendInstance,
    component: SpineSkeletonComponent,
    output: SpineRenderOutput,
}

impl SpinePlayer {
    #[must_use]
    pub fn component(&self) -> &SpineSkeletonComponent {
        &self.component
    }

    #[must_use]
    pub fn output(&self) -> &SpineRenderOutput {
        &self.output
    }

    #[must_use]
    pub fn animation(&self) -> &str {
        self.instance.animation_name()
    }

    #[must_use]
    pub fn animation_names(&self) -> &[String] {
        self.data.animations()
    }

    #[must_use]
    pub fn skin_names(&self) -> &[String] {
        self.data.skins()
    }

    /// 图集页名 → 工程内路径。供快照/导出把纹理读进内存（见 `snapshot.rs`）。
    #[must_use]
    pub fn page_paths(&self) -> &std::collections::HashMap<String, std::sync::Arc<str>> {
        self.data.page_paths()
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.component.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.component.enabled = enabled;
    }

    pub fn set_speed(&mut self, speed: f32) {
        if !speed.is_finite() || speed < 0.0 {
            return;
        }
        self.component.speed = speed;
        self.instance.set_time_scale(speed);
    }

    pub fn set_animation(&mut self, name: &str, looped: bool) -> Result<(), SpineError> {
        self.instance.set_animation(&self.data, name, looped)?;
        self.component.animation = name.to_owned();
        self.component.looped = looped;
        Ok(())
    }

    pub fn set_skin(&mut self, name: &str) -> Result<(), SpineError> {
        self.instance.set_skin(&self.data, name)?;
        self.component.skin = name.to_owned();
        Ok(())
    }

    /// 推进并采样。禁用时保留上一帧网格但不再更新动画（避免闪烁）。
    pub fn advance(&mut self, dt: f32) -> Result<(), SpineError> {
        if !self.component.enabled {
            return Ok(());
        }
        self.instance.advance(
            &self.data,
            dt,
            self.component.scale,
            self.component.z,
            &mut self.output,
        )
    }

    /// 把**当前**姿态采样成网格，不推进时间（`dt = 0`）。
    ///
    /// 存在的理由：编辑器场景视图不跑调度（`from_scene_edit` / `run_script = false`），
    /// `spine_advance` 从不执行，于是 `output` 一直是 `instantiate` 留下的空值——
    /// 表现成"场景视图里挂了 SpineSkeleton 的实体完全不显示"，而 Play 里正常。
    ///
    /// 与 [`Self::advance`] 的区别只有 `dt`：`advance(0.0)` 本身就会重采样一次 setup pose
    /// 并产出 draw list，所以这里不另写一套采样逻辑（两套一定会漂移）。
    ///
    /// 设计约束：**只给编辑态用**。运行时每帧该走真正的 `advance(dt)`，否则动画不会动。
    pub fn snap_pose(&mut self) -> Result<(), SpineError> {
        self.advance(0.0)
    }

    pub fn drain_events(&mut self) -> Vec<SpineEvent> {
        self.instance.drain_events()
    }

    /// 只推进动画状态与骨骼采样，**不**产出 draw list。
    ///
    /// 仅供性能归因使用（`spine_bench`）：把每帧分配拆成"状态机 + 采样"与
    /// "draw list 生成"两块，否则不知道该优化哪一侧。生产路径请用 [`Self::advance`]。
    pub fn advance_state_only(&mut self, dt: f32) -> Result<(), SpineError> {
        if !self.component.enabled {
            return Ok(());
        }
        self.instance.advance_state_only(dt);
        Ok(())
    }

    /// 只把已有 draw list 复制到输出（性能归因用）。
    pub fn advance_copy_only(&mut self) {
        self.instance
            .advance_copy_only(&self.data, &mut self.output);
    }

    /// 只重建 draw list，不推进动画（性能归因用）。
    pub fn advance_draw_list_only(&mut self) -> Result<(), SpineError> {
        self.instance.advance_draw_list_only(&self.data);
        Ok(())
    }

    pub fn clear_for_scene_transition(&mut self) {
        self.instance.clear();
        self.output.clear();
    }
}
