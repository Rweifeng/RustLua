//! M84 P1：Spine 三角网格在渲染帧数据里的形态。
//!
//! 与 `rustlua-spine` 的中性输出是两套类型：本层是 GPU 提交视角（纹理下标、u32 索引），
//! 不反向依赖 spine crate；宿主在 frame_data 里把中性输出转换过来。
//! 几何顺序=slot draw order，上层不得按纹理重排或合批。

/// Spine 混合模式。P1 管线只渲染 `Normal`，其余变体留给 P2。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpineBlendMode {
    Normal,
    Additive,
    Multiply,
    Screen,
}

impl SpineBlendMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Additive => "additive",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
        }
    }
}

/// 世界空间顶点：位置 `(x, y, z)` 由相机投影，颜色为 light tint。
/// 布局与 `WorldSpriteVertex` 完全一致，GPU 侧可直接 `cast_slice` 复用同一顶点布局。
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct SpineVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// 一段索引绘制。`texture` 是 `SceneMesh.paths` 的下标（与 `SceneDraw` 同语义）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpineDraw {
    pub texture: u32,
    pub first_index: u32,
    pub index_count: u32,
    pub blend: SpineBlendMode,
    /// P1 管线不支持 PMA：为 true 时明确跳过并计 dropped，不静默错色。
    pub premultiplied_alpha: bool,
    /// 世界深度：同一实体的 draw 段共享，跨实体排序时用。
    pub z: f32,
}

/// 一帧内全部 Spine 实体。默认空时渲染器零成本跳过。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpineMesh {
    pub vertices: Vec<SpineVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<SpineDraw>,
}

impl SpineMesh {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Returns local indices offset into a merged vertex buffer.
    ///
    /// Every index must address a vertex in the local mesh, and offsetting must
    /// remain representable by the GPU's `u32` index format. No truncation occurs.
    pub fn offset_indices(
        local_indices: &[u32],
        local_vertex_count: usize,
        base_vertex: usize,
    ) -> Option<Vec<u32>> {
        let local_vertex_count = u32::try_from(local_vertex_count).ok()?;
        let base_vertex = u32::try_from(base_vertex).ok()?;

        local_indices
            .iter()
            .map(|index| {
                if *index >= local_vertex_count {
                    return None;
                }
                index.checked_add(base_vertex)
            })
            .collect()
    }
}

#[cfg(test)]
mod index_tests {
    use super::SpineMesh;

    #[test]
    fn merged_indices_preserve_values_above_u16_range() {
        let indices =
            SpineMesh::offset_indices(&[30_000], 40_000, 40_000).expect("valid u32 offset");

        assert_eq!(indices, [70_000]);
    }

    #[test]
    fn rejects_invalid_local_indices_and_u32_overflow() {
        assert!(SpineMesh::offset_indices(&[4], 4, 0).is_none());
        assert!(SpineMesh::offset_indices(&[1], 2, u32::MAX as usize).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mesh_is_empty_and_costs_nothing() {
        let mesh = SpineMesh::default();
        assert!(mesh.is_empty());
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());
    }
}
