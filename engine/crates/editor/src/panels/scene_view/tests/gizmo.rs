//! gizmo：命中测试随工具/锁定变化、旋转与缩放的枢轴、轴方向随相机与实体旋转、
//! 以及世界轴 / 自身轴的选择。

use super::*;

#[test]
fn gizmo_hit_testing_respects_tool_and_lock_state() {
    let center = egui::pos2(100.0, 100.0);
    // 旧观感的三条臂：X 向右、Y 向上、Z 斜向左上（等价于旧版的固定方向）。
    let axes = [
        egui::vec2(40.0, 0.0),
        egui::vec2(0.0, -40.0),
        egui::vec2(-28.0, -28.0),
    ];
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Move,
            center,
            egui::pos2(140.0, 100.0),
            false,
            axes
        ),
        Some(GizmoAxis::X)
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Move,
            center,
            egui::pos2(100.0, 60.0),
            false,
            axes
        ),
        Some(GizmoAxis::Y)
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::Move, center, egui::pos2(72.0, 72.0), false, axes),
        Some(GizmoAxis::Z)
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::Move, center, center, false, axes),
        Some(GizmoAxis::Free)
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::Move, center, center, true, axes),
        None
    );
    // 缩放（M27-02）：**等比**改落在中心方框上，把手各自管单轴/双轴，
    // 由 `scale_handles` 一份数据同时驱动绘制与命中。
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::Scale, center, center, false, axes),
        Some(GizmoAxis::Scale),
        "等比缩放搬到中心方框"
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Scale,
            center,
            center + egui::vec2(40.0, 0.0),
            false,
            axes
        ),
        Some(GizmoAxis::ScaleX),
        "X 臂末端现在是单轴把手"
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Scale,
            center,
            center + egui::vec2(18.0, -18.0),
            false,
            axes
        ),
        Some(GizmoAxis::ScaleXY),
        "XY 平面把手落在两臂之间"
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Scale,
            center,
            egui::pos2(128.0, 72.0),
            false,
            axes
        ),
        None,
        "旧的对角线等比把手位置已让位给 XY 把手（12 点外不再命中）"
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::TilePaint, center, center, false, axes),
        None
    );
}

/// **绕轴心旋转/缩放**（第 48 轮，收尾第 46 轮的缺口）：`Center` 模式下实体绕手柄原点
/// 公转、`Pivot` 模式下原地转；缩放同理。两条路径共用同一份旋转增量。
#[test]
fn gizmo_rotate_and_scale_about_the_pivot() {
    use crate::scene_interaction::GizmoSpace;
    use rustlua_ecs::transform::Transform3D;
    use rustlua_math::math3d::Quat;
    let initial = Transform3D {
        position: Vec3::new(1.0, 0.0, 0.0),
        rotation: Quat::identity(),
        scale: Vec3::new(1.0, 1.0, 1.0),
    };
    let quarter = std::f32::consts::FRAC_PI_2;
    let expected_quarter = Quat::from_axis_angle(Vec3::Y, quarter).expect("四元数");
    let same_rotation = |rotation: Quat| {
        (rotation.y - expected_quarter.y).abs() < 1e-4
            && (rotation.w - expected_quarter.w).abs() < 1e-4
    };

    // 原地（Pivot）：位置不动、朝向变。
    let (position, rotation) =
        crate::gizmo::rotate_about(GizmoSpace::World, Vec3::Y, &initial, None, quarter);
    assert_eq!(position, initial.position, "Pivot 模式下位置不得变化");
    assert!(same_rotation(rotation), "原地旋转仍要改朝向：{rotation:?}");

    // 绕原点公转（Center）：位置转到 (0,0,-1) 附近，且到轴心的距离不变。
    let pivot = Vec3::new(0.0, 0.0, 0.0);
    let (position, rotation) =
        crate::gizmo::rotate_about(GizmoSpace::World, Vec3::Y, &initial, Some(pivot), quarter);
    assert!(
        position.x.abs() < 1e-4 && (position.z + 1.0).abs() < 1e-4,
        "绕世界 Y 转 90° 后实体应到 (0,0,-1)：{position:?}"
    );
    assert!(
        ((position - pivot).length() - (initial.position - pivot).length()).abs() < 1e-4,
        "公转不得改变到轴心的距离"
    );
    assert!(
        same_rotation(rotation),
        "公转时朝向同样要跟着转：{rotation:?}"
    );
    // 轴心就是实体自己时，公转与原地等价。
    let (position, rotation) = crate::gizmo::rotate_about(
        GizmoSpace::World,
        Vec3::Y,
        &initial,
        Some(initial.position),
        quarter,
    );
    assert_eq!(position, initial.position);
    assert!(same_rotation(rotation));

    // 旋转增量：世界模式绕世界轴，局部模式绕实体自己的该轴；两者都满足
    // `delta * rotation == gizmo_rotate(...)`（同一份实现，不会漂移）。
    let tilted = Quat::from_axis_angle(Vec3::new(1.0, 0.0, 1.0), 0.6).expect("四元数");
    for space in [GizmoSpace::World, GizmoSpace::Local] {
        let delta = crate::gizmo::rotation_delta(space, tilted, Vec3::Y, 0.4);
        let composed = delta * tilted;
        // 生产路径（`gizmo_rotate_about` 的 `Pivot` 分支）必须给出同一份朝向。
        let composed_alt = crate::gizmo::rotate_about(
            space,
            Vec3::Y,
            &Transform3D {
                position: Vec3::new(0.0, 0.0, 0.0),
                rotation: tilted,
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
            None,
            0.4,
        )
        .1;
        let worst = (composed.x - composed_alt.x)
            .abs()
            .max((composed.y - composed_alt.y).abs())
            .max((composed.z - composed_alt.z).abs())
            .max((composed.w - composed_alt.w).abs());
        assert!(
            worst < 1e-5,
            "{space:?}: 轨道增量必须与生产路径的旋转语义一致（最大差 {worst}）"
        );
    }
    let local_axis = crate::gizmo::rotation_delta(GizmoSpace::Local, tilted, Vec3::Y, 0.4);
    let own_y = tilted.rotate(Vec3::Y).expect("旋转 Y");
    // 局部增量绕的是实体自己的 Y：作用在该轴上不改变它。
    let rotated = local_axis.rotate(own_y).expect("旋转 Y");
    assert!((rotated - own_y).length() < 1e-4, "局部增量应绕自身 Y 轴");

    // 缩放：Center 时位置按比例远离轴心；Pivot 时位置不动。
    let all = [true, true, true];
    let (position, scale) = crate::gizmo::scale_about(&initial, None, all, 2.0);
    assert_eq!(position, initial.position);
    assert_eq!(scale, Vec3::new(2.0, 2.0, 2.0));
    let (position, scale) = crate::gizmo::scale_about(&initial, Some(pivot), all, 2.0);
    assert!(
        (position.x - 2.0).abs() < 1e-4,
        "放大 2 倍应把实体推离轴心 2 倍：{position:?}"
    );
    assert_eq!(scale, Vec3::new(2.0, 2.0, 2.0));
    // 缩放系数下限仍然生效（不会被压成 0 或负数）。
    let (_, scale) = crate::gizmo::scale_about(&initial, None, all, 0.0);
    assert_eq!(scale, Vec3::new(0.05, 0.05, 0.05));
}

/// **Gizmo 轴心模式**（第 46 轮 P1）：`Pivot` = 实体原点、`Center` = AABB 中心；
/// 2D 里 `Pivot` 绕主选中自身、`Center` 绕拖动集合的位置平均（多选时才看得出差别）。
#[test]
fn gizmo_pivot_mode_moves_the_origin_and_the_2d_center() {
    use crate::scene_interaction::GizmoPivot;
    let position = Vec3::new(10.0, 0.0, 0.0);
    // 一个"原点在脚下"的模型：AABB 从 0 到 2 高。
    let bounds = rustlua_math::math3d::Aabb3 {
        min: Vec3::new(-0.5, 0.0, -0.5),
        max: Vec3::new(0.5, 2.0, 0.5),
    };
    assert_eq!(
        crate::gizmo::single_anchor(GizmoPivot::Pivot, position, bounds),
        position,
        "轴心模式就是实体原点（既有行为）"
    );
    let center = crate::gizmo::single_anchor(GizmoPivot::Center, position, bounds);
    assert!(
        (center.y - 1.0).abs() < 1e-6 && (center.x - 10.0).abs() < 1e-6,
        "中心模式应落在 AABB 中心（高度一半处）：{center:?}"
    );

    // 2D：两个实体（x=0 与 x=10）。轴心 = 主选中自己；中心 = 两者平均。
    let mut initial = std::collections::BTreeMap::new();
    initial.insert(
        SceneEntityId(1),
        TransformFields {
            position_x: 0.0,
            position_y: 0.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        },
    );
    initial.insert(
        SceneEntityId(2),
        TransformFields {
            position_x: 10.0,
            position_y: 4.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        },
    );
    assert_eq!(
        scene_drag_pivot(GizmoPivot::Pivot, Some(SceneEntityId(2)), &initial),
        (10.0, 4.0),
        "轴心模式绕主选中实体自己"
    );
    assert_eq!(
        scene_drag_pivot(GizmoPivot::Center, Some(SceneEntityId(2)), &initial),
        (5.0, 2.0),
        "中心模式绕拖动集合的位置平均"
    );
    // 主选中不在集合里（例如选区刚变化）→ 退回平均，不退化成 (0,0)。
    assert_eq!(
        scene_drag_pivot(GizmoPivot::Pivot, Some(SceneEntityId(9)), &initial),
        (5.0, 2.0)
    );
    // 单选时两种模式等价（这正是默认模式下行为不变的原因）。
    let single = std::collections::BTreeMap::from([(
        SceneEntityId(1),
        TransformFields {
            position_x: 3.0,
            position_y: -2.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        },
    )]);
    assert_eq!(
        scene_drag_pivot(GizmoPivot::Pivot, Some(SceneEntityId(1)), &single),
        scene_drag_pivot(GizmoPivot::Center, Some(SceneEntityId(1)), &single)
    );
}

/// **手柄方向来自真实轴的投影**（第 45 轮）：相机/实体一变，手柄的屏幕方向就跟着变；
/// 命中检测用的也是同一份方向（"看到哪里点哪里"）；轴与视线平行（投影退化）时退回固定方向。
#[test]
fn gizmo_axes_follow_the_camera_projection() {
    use crate::scene_interaction::GizmoSpace;
    use rustlua_math::math3d::Quat;
    let center = egui::pos2(200.0, 150.0);
    let len = 40.0;
    // 假投影：x 向右、y 向上（屏幕 y 反向）、z **完全压扁**（模拟正俯视：
    // 世界 Z 与视线平行，投影退缩成一个点）。
    let project = |point: Vec3| {
        Some(egui::pos2(
            center.x + point.x * 10.0,
            center.y - point.y * 10.0,
        ))
    };
    let axes = gizmo_screen_axes(
        Vec3::new(0.0, 0.0, 0.0),
        center,
        len,
        GizmoSpace::World,
        Quat::identity(),
        project,
    );
    assert!(
        (axes[0] - egui::vec2(len, 0.0)).length() < 1e-3,
        "世界 X 应朝屏幕右：{:?}",
        axes[0]
    );
    assert!(
        (axes[1] - egui::vec2(0.0, -len)).length() < 1e-3,
        "世界 Y 应朝屏幕上：{:?}",
        axes[1]
    );
    // Z 与视线接近平行 → 投影极短 → 退回固定方向（不能缩成一个点）。
    assert!(
        (axes[2] - egui::vec2(-len * 0.7, -len * 0.7)).length() < 1e-3,
        "退化轴退回固定方向：{:?}",
        axes[2]
    );

    // 局部模式：绕 Y 转 90° 后，局部 X 指向世界 -Z（投影退化 → 退回固定方向），
    // 而局部 Z 指向世界 +X（朝屏幕右）——手柄确实跟着实体转。
    let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).expect("四元数");
    let local = gizmo_screen_axes(
        Vec3::new(0.0, 0.0, 0.0),
        center,
        len,
        GizmoSpace::Local,
        rotation,
        project,
    );
    assert!(
        (local[2] - egui::vec2(len, 0.0)).length() < 1e-3,
        "局部 Z 应朝世界 +X（屏幕右）：{:?}",
        local[2]
    );
    // 投影完全失败时三条臂都退回固定方向（手柄不会消失）。
    let fallback = gizmo_screen_axes(
        Vec3::new(0.0, 0.0, 0.0),
        center,
        len,
        GizmoSpace::World,
        Quat::identity(),
        |_| None,
    );
    assert_eq!(fallback[0], egui::vec2(len, 0.0));
    assert_eq!(fallback[1], egui::vec2(0.0, -len));
    // 命中检测跟随投影后的方向：屏幕右侧不再是 X 臂，而是被投影到别处的臂。
    let rotated = [
        egui::vec2(0.0, -len),
        egui::vec2(-len, 0.0),
        egui::vec2(len, 0.0),
    ];
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(SceneTool::Move, center, center + rotated[0], false, rotated),
        Some(GizmoAxis::X)
    );
    assert_eq!(
        crate::gizmo::pick_gizmo_axis(
            SceneTool::Move,
            center,
            center + egui::vec2(len, 0.0),
            false,
            rotated
        ),
        Some(GizmoAxis::Z),
        "同一个屏幕位置，方向变了命中的轴也跟着变"
    );
}

/// **Gizmo 变换空间**（第 43 轮 P1）：局部模式下手柄轴跟随实体旋转，世界模式不受影响；
/// 坏掉的四元数退回世界轴（否则手柄会消失、连修都修不了）。
#[test]
fn gizmo_local_axes_follow_the_entity_rotation() {
    use crate::scene_interaction::GizmoSpace;
    use rustlua_math::math3d::Quat;
    // 绕 Y 转 90°：局部 X 变成世界 -Z，局部 Z 变成世界 +X（右手系）。
    let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).expect("单位四元数");
    let local_x = gizmo_axis_world(GizmoSpace::Local, rotation, GizmoAxis::X).expect("X 轴");
    let local_z = gizmo_axis_world(GizmoSpace::Local, rotation, GizmoAxis::Z).expect("Z 轴");
    assert!(
        (local_x.z + 1.0).abs() < 1e-4 && local_x.x.abs() < 1e-4,
        "局部 X 应指向世界 -Z：{local_x:?}"
    );
    assert!(
        (local_z.x - 1.0).abs() < 1e-4 && local_z.z.abs() < 1e-4,
        "局部 Z 应指向世界 +X：{local_z:?}"
    );
    // 世界模式与旋转无关（既有行为一字不改）。
    let world_x = gizmo_axis_world(GizmoSpace::World, rotation, GizmoAxis::X).expect("X 轴");
    assert!((world_x.x - 1.0).abs() < 1e-4 && world_x.z.abs() < 1e-4);
    // 单位旋转下两种空间一致。
    let identity = Quat::identity();
    assert_eq!(
        gizmo_axis_world(GizmoSpace::Local, identity, GizmoAxis::Y),
        Some(Vec3::Y)
    );
    // 非法四元数（零长度） → 退回世界轴，而不是 None。
    let broken = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 0.0,
    };
    assert_eq!(
        gizmo_axis_world(GizmoSpace::Local, broken, GizmoAxis::Z),
        Some(Vec3::Z),
        "坏旋转不得让手柄消失"
    );
    // 非轴手柄（Free/Scale）没有方向。
    assert_eq!(
        gizmo_axis_world(GizmoSpace::Local, identity, GizmoAxis::Free),
        None
    );
}

/// 旋转空间：**世界模式左乘**（绕世界 Y 公转），**局部模式右乘**（只改自身朝向——
/// 实体自己的 Y 轴在旋转后必须原样不动）。这条就是 Local/World 在旋转上的全部区别。
#[test]
fn gizmo_rotation_space_picks_world_or_own_axis() {
    use crate::scene_interaction::GizmoSpace;
    use rustlua_math::math3d::Quat;
    let initial = Quat::from_axis_angle(Vec3::new(1.0, 0.0, 1.0), 0.6).expect("单位四元数");
    let own_y = initial.rotate(Vec3::Y).expect("旋转 Y");
    let angle = 0.5_f32;

    // 局部模式：绕**实体自己的 Y** 转——实体自己的 Y 轴在旋转后原样不动。
    let local = rotate_only(GizmoSpace::Local, initial, angle);
    let local_y_after = local.rotate(Vec3::Y).expect("旋转 Y");
    assert!(
        (local_y_after.x - own_y.x).abs() < 1e-4
            && (local_y_after.y - own_y.y).abs() < 1e-4
            && (local_y_after.z - own_y.z).abs() < 1e-4,
        "局部旋转必须绕实体自己的 Y：{own_y:?} → {local_y_after:?}"
    );

    // 世界模式：绕**世界 Y** 转——实体自己的 Y 会被带偏。
    let world = rotate_only(GizmoSpace::World, initial, angle);
    let world_y_after = world.rotate(Vec3::Y).expect("旋转 Y");
    assert!(
        (world_y_after.x - own_y.x).abs() > 1e-3
            || (world_y_after.y - own_y.y).abs() > 1e-3
            || (world_y_after.z - own_y.z).abs() > 1e-3,
        "世界旋转会带动实体朝向（绕世界 Y 转）：{own_y:?} → {world_y_after:?}"
    );
    // 世界模式的结果就是把初始朝向整体绕世界 Y 转 angle。
    let expected = Quat::from_axis_angle(Vec3::Y, angle)
        .expect("单位四元数")
        .rotate(own_y)
        .expect("旋转 Y");
    assert!((world_y_after.x - expected.x).abs() < 1e-4);
    assert!((world_y_after.z - expected.z).abs() < 1e-4);
    // 角度为 0 时两种空间都是原姿态。
    assert_eq!(rotate_only(GizmoSpace::Local, initial, 0.0), initial);
    assert_eq!(rotate_only(GizmoSpace::World, initial, 0.0), initial);
    // 非有限角度不得改姿态（引擎的四元数构造会拒绝）。
    assert_eq!(rotate_only(GizmoSpace::World, initial, f32::NAN), initial);
}
