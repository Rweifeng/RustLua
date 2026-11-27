//! 3D 实体包围盒：选中轮廓、点选与 Gizmo 轴心共用的**唯一**来源。
//!
//! 以前这里在 `view_3d.rs` 里写死成 `±0.5` 的立方：缩放到 10 倍的立方体只有中心 1 米内
//! 能点中、轴心"中心"模式与"枢轴"模式在单选时完全等价。
//!
//! 现在按网格规格取真实范围（**与宿主渲染提取同一批解析器**，见
//! `host/runtime_scene/frame_data.rs` 的 `collect`），再按 Transform 缩放放大。
//! 两条口径一致，编辑器的"框在哪、点中谁、手柄画在哪"才不会与渲染器的剔除判据打架。
//!
//! 诚实边界：glTF 网格目前在宿主侧也只有 0.5 立方兜底（提取期没有网格 AABB 源），
//! 这里如实同口径——不另造一套"看起来更准"的假包围盒。

use super::*;

/// 没有可用网格规格时的兜底：边长 1、以实体原点为中心的立方体。
fn fallback_bounds() -> ([f32; 3], [f32; 3]) {
    ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5])
}

/// 网格的**局部**范围（未缩放）：按路径识别规格，规则与宿主提取逐条对齐。
pub(crate) fn mesh_local_bounds(mesh_path: &str) -> ([f32; 3], [f32; 3]) {
    if mesh_path == rustlua_render::mesh3d::GROUND_PLANE_SENTINEL {
        // 宿主的地面是 ±100 的薄板（见 `frame_data.rs` 的同一条分支）。
        return ([-100.0, -0.05, -100.0], [100.0, 0.05, 100.0]);
    }
    if rustlua_render::terrain::TerrainSpec::is_terrain_path(mesh_path) {
        return rustlua_render::terrain::TerrainSpec::parse(mesh_path)
            .map(|spec| spec.bounds())
            .unwrap_or_else(|_| fallback_bounds());
    }
    if rustlua_render::water::WaterSpec::is_water_path(mesh_path) {
        return rustlua_render::water::WaterSpec::parse(mesh_path)
            .map(|spec| spec.bounds())
            .unwrap_or_else(|_| fallback_bounds());
    }
    if rustlua_render::cellmesh::CellMeshSpec::is_cells_path(mesh_path) {
        return rustlua_render::cellmesh::CellMeshSpec::parse(mesh_path)
            .map(|spec| spec.bounds())
            .unwrap_or_else(|_| fallback_bounds());
    }
    if rustlua_render::primitives::PrimitiveSpec::is_primitive_path(mesh_path) {
        return rustlua_render::primitives::PrimitiveSpec::parse(mesh_path)
            .map(|spec| spec.bounds())
            .unwrap_or_else(|_| fallback_bounds());
    }
    fallback_bounds()
}

/// 把局部范围按各轴缩放成 AABB。负缩放按绝对值处理，并重新排序 min/max
/// （镜像不会把盒子翻成负尺寸）。
pub(crate) fn scaled_bounds(
    local: ([f32; 3], [f32; 3]),
    scale: Vec3,
) -> rustlua_math::math3d::Aabb3 {
    let axes = [
        (local.0[0], local.1[0], scale.x),
        (local.0[1], local.1[1], scale.y),
        (local.0[2], local.1[2], scale.z),
    ];
    let mut min = [0.0_f32; 3];
    let mut max = [0.0_f32; 3];
    for (axis, (low, high, scale)) in axes.into_iter().enumerate() {
        let a = low * scale;
        let b = high * scale;
        min[axis] = a.min(b);
        max[axis] = a.max(b);
    }
    rustlua_math::math3d::Aabb3 {
        min: Vec3::new(min[0], min[1], min[2]),
        max: Vec3::new(max[0], max[1], max[2]),
    }
}

/// 一个 3D 实体的局部包围盒：网格规格 × Transform 缩放。
pub(crate) fn entity_local_bounds(mesh_path: &str, scale: Vec3) -> rustlua_math::math3d::Aabb3 {
    scaled_bounds(mesh_local_bounds(mesh_path), scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_specs_provide_real_extents_instead_of_a_unit_box() {
        // 4×2×6 的立方体：旧实现只认 ±0.5，缩放与尺寸都不进包围盒。
        let path = "__primitive__?kind=box&w=4&h=2&d=6";
        let bounds = entity_local_bounds(path, Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(bounds.min, Vec3::new(-2.0, -1.0, -3.0));
        assert_eq!(bounds.max, Vec3::new(2.0, 1.0, 3.0));
    }

    #[test]
    fn transform_scale_grows_the_box_and_negative_scale_keeps_min_le_max() {
        let path = "__primitive__?kind=box&w=1&h=1&d=1";
        let scaled = entity_local_bounds(path, Vec3::new(10.0, 2.0, 1.0));
        assert_eq!(
            scaled.max,
            Vec3::new(5.0, 1.0, 0.5),
            "缩放到 10 倍要真的变大"
        );
        let mirrored = entity_local_bounds(path, Vec3::new(-2.0, 1.0, 1.0));
        assert!(mirrored.min.x <= mirrored.max.x, "负缩放不得产生反向 AABB");
        assert_eq!(mirrored.min.x, -1.0);
        assert_eq!(mirrored.max.x, 1.0);
    }

    #[test]
    fn unknown_meshes_fall_back_to_the_same_unit_box_as_the_host() {
        let bounds = entity_local_bounds("assets/models/hero.glb", Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(bounds.min, Vec3::new(-0.5, -0.5, -0.5));
        assert_eq!(bounds.max, Vec3::new(0.5, 0.5, 0.5));
    }

    /// 偏心的包围盒让 Center 轴心与 Pivot 真正分开（巢体从 `origin` 起算、地形高度段
    /// 不居中都是这种形状）。旧实现写死 ±0.5 的居中立方体，单选时两个轴心永远重合。
    #[test]
    fn off_center_bounds_make_center_differ_from_pivot() {
        use crate::scene_interaction::GizmoPivot;
        let off_center = rustlua_math::math3d::Aabb3 {
            min: Vec3::new(-2.0, 0.0, 0.0),
            max: Vec3::new(0.0, 2.0, 2.0),
        };
        let pivot =
            crate::gizmo::gizmo_anchor(GizmoPivot::Pivot, Vec3::ZERO, [(Vec3::ZERO, off_center)]);
        let center =
            crate::gizmo::gizmo_anchor(GizmoPivot::Center, Vec3::ZERO, [(Vec3::ZERO, off_center)]);
        assert_eq!(pivot, Vec3::ZERO, "Pivot 就是实体原点");
        assert_eq!(
            center,
            Vec3::new(-1.0, 1.0, 1.0),
            "Center 必须是包围盒中心，且与 Pivot 不同"
        );
    }
}
