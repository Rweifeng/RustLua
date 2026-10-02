#[cfg(test)]
mod projection_tests {
    use crate::camera::*;
    use rustlua_ecs::transform::Transform3D;
    use rustlua_math::math2d::{WORLD_RIGHT, WORLD_UP};
    use rustlua_math::math3d::{Aabb3, Quat, Vec3};

    fn camera() -> Camera3D {
        Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .unwrap()
    }
    /// 透视换算一律要一个视口（宽高比只从这里来，拆分批 7）。
    fn viewport(width: f32, height: f32) -> Viewport {
        Viewport::new(width, height).unwrap()
    }
    /// 1:1 视口：把这些测试的数值与"宽高比为 1"的旧基线对齐（旧版相机自带 `aspect_ratio=1.0`）。
    fn square() -> Viewport {
        viewport(1.0, 1.0)
    }
    #[test]
    fn camera_center_round_trips_through_ndc() {
        let camera = camera();
        let point = Vec3::new(0.0, 0.0, 0.0);
        let ndc = camera.world_to_ndc(point, square()).unwrap();
        assert!((ndc.x.abs() + ndc.y.abs() + ndc.z - 0.981).abs() < 0.01);
        let recovered = camera.ndc_to_world(ndc, square()).unwrap();
        assert!((recovered - point).length() < 1e-4);
    }
    #[test]
    fn screen_center_ray_points_toward_camera_target() {
        let camera = camera();
        let (origin, direction) = camera
            .ray_from_screen(320.0, 240.0, viewport(640.0, 480.0))
            .unwrap();
        assert!(origin.z < camera.position.z);
        assert!(direction.z < -0.9);
        assert!(direction.y.abs() < 0.01);
    }
    #[test]
    fn ray_intersects_aabb_and_rejects_parallel_miss() {
        let ray = Ray3::new(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z).unwrap();
        let bounds = Aabb3::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert_eq!(ray.intersect_aabb(bounds), Some(4.0));
        let miss = Ray3::new(Vec3::new(2.0, 0.0, 5.0), -Vec3::Z).unwrap();
        assert_eq!(miss.intersect_aabb(bounds), None);
    }
    #[test]
    fn ray_hits_nearest_sphere_point() {
        let ray = Ray3::new(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        assert_eq!(ray.intersect_sphere(Vec3::ZERO, 1.0), Some(4.0));
        assert_eq!(ray.intersect_sphere(Vec3::new(5.0, 0.0, 0.0), 0.5), None);
    }
    #[test]
    fn frustum_classifies_front_and_rear_points() {
        let camera = camera();
        let frustum = camera.frustum(square()).unwrap();
        assert!(frustum.contains_point(Vec3::ZERO));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, 6.0)));
    }

    #[test]
    fn frustum_sphere_culling_respects_radius() {
        let camera = camera();
        let frustum = camera.frustum(square()).unwrap();
        // Behind the camera (near plane) is culled; a large radius that reaches
        // back in front is kept.
        assert!(!frustum.contains_sphere(Vec3::new(0.0, 0.0, 6.0), 0.0));
        assert!(frustum.contains_sphere(Vec3::new(0.0, 0.0, 6.0), 1.5));
        // Far outside a side plane is culled unless the radius reaches in.
        assert!(!frustum.contains_sphere(Vec3::new(6.0, 0.0, 0.0), 0.1));
        assert!(frustum.contains_sphere(Vec3::new(6.0, 0.0, 0.0), 3.0));
        // Inside and boundary points are always visible.
        assert!(frustum.contains_sphere(Vec3::ZERO, 0.0));
        assert!(frustum.contains_sphere(Vec3::new(0.0, 0.0, 4.9), 0.0));
        // Non-finite radii degrade to visible (never cull).
        assert!(frustum.contains_sphere(Vec3::new(50.0, 50.0, 50.0), f32::NAN));
    }

    #[test]
    fn frustum_planes_agree_with_point_classification() {
        let camera = camera();
        let frustum = camera.frustum(square()).unwrap();
        let inside = Vec3::ZERO;
        let outside = Vec3::new(0.0, 0.0, 6.0);
        // The origin is inside every plane (positive signed distance).
        for plane in frustum.planes() {
            assert!(
                plane.signed_distance(inside) >= 0.0,
                "origin inside all planes"
            );
        }
        // A point far behind the camera violates at least one plane.
        assert!(
            frustum
                .planes()
                .iter()
                .any(|plane| plane.signed_distance(outside) < 0.0)
        );
    }
    #[test]
    fn invalid_perspective_is_rejected() {
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::Z, 0.0, 0.1, 10.0).is_err());
        assert!(Camera3D::perspective(Vec3::ZERO, Vec3::ZERO, 1.0, 0.1, 10.0).is_err());
    }
    #[test]
    fn parses_editor_component_string() {
        let camera =
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100")
                .unwrap();
        assert_eq!(camera.position, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(camera.target, Vec3::ZERO);
        assert!((camera.fov_y - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(camera.near, 0.1);
        assert_eq!(camera.far, 100.0);
        assert_eq!(
            Camera3D::from_component_str(&camera.to_component_str()).unwrap(),
            camera
        );
    }
    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;near=0.1;far=100"),
            Err(Camera3dComponentError::MissingField("fov"))
        ));
        assert!(matches!(
            Camera3D::from_component_str("position=0,0;target=0,0,0;fov=90;near=0.1;far=100"),
            Err(Camera3dComponentError::InvalidValue(_))
        ));
        // fov out of (0, 180) degrees
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=0;near=0.1;far=100",)
                .is_err()
        );
        // far must exceed near
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,0;fov=90;near=10;far=1",)
                .is_err()
        );
        // look-at degenerate
        assert!(
            Camera3D::from_component_str("position=0,0,5;target=0,0,5;fov=90;near=0.1;far=100",)
                .is_err()
        );
    }

    /// docs/54 §8-①：正交（2D）相机也是场景组件，因此组件串必须能往返、且拼错要报错。
    #[test]
    fn camera2d_component_string_round_trips_and_rejects_bad_input() {
        let camera =
            Camera2D::from_component_str("center=100,50;zoom=2").expect("minimal camera2d");
        assert_eq!(camera.center, Vec2::new(100.0, 50.0));
        assert_eq!(camera.zoom, 2.0);
        assert_eq!(camera.rotation, 0.0);
        assert!(camera.bounds.is_none());

        let full = Camera2D::from_component_str(
            "center=640,360;rotation=0.5;zoom=1.5;bounds=0,0,1280,720",
        )
        .expect("full camera2d");
        assert_eq!(full.rotation, 0.5);
        assert_eq!(full.bounds.expect("bounds").min, Vec2::new(0.0, 0.0));
        // 往返稳定：解析 → 写回 → 再解析得到同一台相机。
        assert_eq!(
            Camera2D::from_component_str(&full.to_component_str()).expect("reparse"),
            full
        );
        assert_eq!(
            Camera2D::from_component_str(&camera.to_component_str()).expect("reparse minimal"),
            camera
        );

        assert_eq!(
            Camera2D::from_component_str(""),
            Err(Camera2dComponentError::Empty)
        );
        assert_eq!(
            Camera2D::from_component_str("center=0,0"),
            Err(Camera2dComponentError::MissingField("zoom"))
        );
        assert_eq!(
            Camera2D::from_component_str("zoom=1"),
            Err(Camera2dComponentError::MissingField("center"))
        );
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=1;fov=60"),
            Err(Camera2dComponentError::UnknownField(_))
        ));
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=0"),
            Err(Camera2dComponentError::Invalid(_))
        ));
        assert!(matches!(
            Camera2D::from_component_str("center=0,0;zoom=1;bounds=10,10,0,0"),
            Err(Camera2dComponentError::Invalid(_))
        ));
    }
    use rustlua_ecs::transform::Vec2;
    fn assert_vec_close(actual: Vec2, expected: Vec2, epsilon: f32) {
        assert!(
            (actual.x - expected.x).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
        assert!(
            (actual.y - expected.y).abs() <= epsilon,
            "{actual:?} != {expected:?}"
        );
    }
    #[test]
    fn coordinate_axes_and_center_match_convention() {
        let viewport = Viewport::new(800.0, 600.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0).unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(400.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(410.0, 300.0)
        );
        assert_eq!(
            camera.world_to_screen(WORLD_UP, viewport).unwrap(),
            Vec2::new(400.0, 290.0)
        );
    }
    #[test]
    fn coordinate_conversion_round_trips_at_multiple_viewports_and_zooms() {
        for (width, height, zoom) in [
            (800.0, 600.0, 1.0),
            (1920.0, 1080.0, 100.0),
            (320.0, 240.0, 0.25),
        ] {
            let viewport = Viewport::new(width, height).unwrap();
            let camera = Camera2D::new(Vec2::new(3.5, -2.0), zoom).unwrap();
            let world = Vec2::new(-12.25, 8.75);
            let screen = camera.world_to_screen(world, viewport).unwrap();
            assert_vec_close(
                camera.screen_to_world(screen, viewport).unwrap(),
                world,
                0.0001,
            );
        }
    }
    #[test]
    fn invalid_coordinate_inputs_are_rejected() {
        assert!(matches!(
            Viewport::new(0.0, 1.0),
            Err(CoordinateError::InvalidViewport { .. })
        ));
        assert!(matches!(
            Camera2D::new(Vec2::zero(), 0.0),
            Err(CoordinateError::InvalidZoom(_))
        ));
        assert!(matches!(
            Camera2D::new(Vec2::new(f32::NAN, 0.0), 1.0),
            Err(CoordinateError::NonFiniteVector { .. })
        ));
    }

    #[test]
    fn camera_rotation_round_trips_and_rotates_axes() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::zero(), 10.0)
            .unwrap()
            .with_rotation(std::f32::consts::FRAC_PI_2)
            .unwrap();
        assert_vec_close(
            camera.world_to_screen(WORLD_RIGHT, viewport).unwrap(),
            Vec2::new(100.0, 60.0),
            0.0001,
        );
        let world = Vec2::new(2.5, -3.0);
        assert_vec_close(
            camera
                .screen_to_world(camera.world_to_screen(world, viewport).unwrap(), viewport)
                .unwrap(),
            world,
            0.0001,
        );
    }

    #[test]
    fn camera_bounds_keep_viewport_inside_world_rectangle() {
        let viewport = Viewport::new(100.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(-100.0, 100.0), 10.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::new(-5.0, -5.0), Vec2::new(5.0, 5.0)).unwrap())
            .unwrap();
        assert_eq!(
            camera.world_to_screen(Vec2::zero(), viewport).unwrap(),
            Vec2::new(50.0, 50.0)
        );
        assert_eq!(
            camera
                .screen_to_world(Vec2::new(0.0, 0.0), viewport)
                .unwrap(),
            Vec2::new(-5.0, 5.0)
        );
    }

    /// WP5 旧新对拍基线：**投影与射线的约定**逐条钉住（3D 侧）。
    ///
    /// 上面那些测试是性质型的（往返一致、分类正确），它们拦不住"约定被悄悄换掉"——把 NDC 的 y
    /// 翻转、把深度范围从 0..1 换成 -1..1、把 FOV 从垂直改成水平，往返测试全都照样通过，而画面
    /// 与拾取会整体错位。相机收敛要动投影，所以先把**数值基线**钉在这里：任何一处变了都必须
    /// 是有意识的决定，而不是顺手改掉。
    #[test]
    fn projection_conventions_are_pinned_for_the_convergence() {
        let camera = camera(); // position (0,0,5)、看向原点、fov_y = π/2、near 0.1、far 100
        // 宽高比只从视口来（拆分批 7）。这一条数值基线取 1:1 视口，与旧版"相机自带
        // `aspect_ratio = 1.0`"逐值可比——换的是参数来源，不是算法。
        let unit = square();

        // ① 投影矩阵：f = 1/tan(fov_y/2) = 1；aspect 只影响 x；深度映射为 **0..1**。
        let projection = camera.projection_matrix(unit).unwrap();
        let column = |index: usize| projection.columns[index];
        assert!(
            (column(0)[0] - 1.0).abs() < 1e-5,
            "fov_y 是**垂直**视场角，x 除以 aspect"
        );
        assert!((column(1)[1] - 1.0).abs() < 1e-5);
        let range = 100.0 - 0.1;
        assert!(
            (column(2)[2] - (-100.0 / range)).abs() < 1e-4,
            "深度范围 0..1（不是 -1..1）"
        );
        assert!((column(2)[3] + 1.0).abs() < 1e-5, "透视除法项必须是 -1");
        assert!((column(3)[2] - (-0.1 * 100.0 / range)).abs() < 1e-4);

        // ② 视图矩阵是右手系、看向 -Z：相机在 (0,0,5) 看向原点时，原点映射到 z = -5。
        let view = camera.view_matrix().unwrap();
        let origin_in_view = view.transform_point(Vec3::ZERO).unwrap();
        assert!(origin_in_view.x.abs() < 1e-5 && origin_in_view.y.abs() < 1e-5);
        assert!(
            (origin_in_view.z + 5.0).abs() < 1e-5,
            "前方向是 -Z（右手系）"
        );

        // ③ 世界 → NDC：世界 +x 落 NDC 右半边、世界 +y 落 NDC **上**半边；
        //    深度随距离单调增且始终在 0..1 内。
        let right = camera.world_to_ndc(Vec3::new(1.0, 0.0, 0.0), unit).unwrap();
        let up = camera.world_to_ndc(Vec3::new(0.0, 1.0, 0.0), unit).unwrap();
        assert!(right.x > 0.0 && right.y.abs() < 1e-5, "世界 +x → NDC +x");
        assert!(
            up.y > 0.0 && up.x.abs() < 1e-5,
            "世界 +y → NDC +y（NDC y 向上）"
        );
        let near_point = camera.world_to_ndc(Vec3::new(0.0, 0.0, 4.0), unit).unwrap();
        let far_point = camera
            .world_to_ndc(Vec3::new(0.0, 0.0, -50.0), unit)
            .unwrap();
        assert!(near_point.z > 0.0 && near_point.z < far_point.z && far_point.z < 1.0);

        // ④ 射线与投影必须互为逆：把任意世界点投到屏幕，再从该屏幕坐标发射线，射线必须**穿过**
        //    该点。投影与"屏幕多大"必须来自**同一个视口**（这正是"点中的位置"与"看到的位置"
        //    同源的那条要求；旧版这里投影用组件比例 1.0、屏幕却按 640×480，是两份状态）。
        let world = Vec3::new(1.5, -0.75, 0.5);
        let screen_viewport = viewport(640.0, 480.0);
        let ndc = camera.world_to_ndc(world, screen_viewport).unwrap();
        let screen_x = (ndc.x + 1.0) * 0.5 * screen_viewport.width;
        let screen_y = (1.0 - ndc.y) * 0.5 * screen_viewport.height;
        let (origin, direction) = camera
            .ray_from_screen(screen_x, screen_y, screen_viewport)
            .unwrap();
        let along = (world - origin).dot(direction);
        let closest = origin + direction * along;
        assert!(
            (closest - world).length() < 1e-3,
            "屏幕坐标反投影出的射线必须穿过该世界点：closest={closest:?} world={world:?}"
        );

        // ⑤ 视锥：正前方的点在视锥内、相机背后的点在视锥外。
        let frustum = camera.frustum(unit).unwrap();
        assert!(frustum.contains_point(Vec3::ZERO));
        assert!(
            !frustum.contains_point(Vec3::new(0.0, 0.0, 50.0)),
            "相机背后的点必须在视锥外"
        );

        // ⑥ **宽高比只有视口一个来源**：换视口就换矩阵，相机载荷不变。
        let wide = camera.projection_matrix(viewport(16.0, 9.0)).unwrap();
        assert!(
            (wide.columns[0][0] - 1.0 / (16.0 / 9.0)).abs() < 1e-5,
            "x 缩放 = f / (视口宽高比)"
        );
        assert_eq!(
            camera.projection_matrix(unit).unwrap().columns[0][0],
            projection.columns[0][0],
            "同一台相机在同一视口下必须给同一矩阵（矩阵不缓存任何比例）"
        );
    }

    /// 拆分批 7（负向契约）：`Camera3D` 里**没有**宽高比字段，因此不可能出现
    /// "组件里存的比例"与"渲染/拾取实际用的比例"两份状态。
    ///
    /// 这条测试是**编译期契约**的运行期备忘：任何把比例塞回相机类型的改动都会让上面那些
    /// 调用点（`world_to_ndc`/`ray_from_screen`/`frustum`/`projection_matrix` 全都要视口）
    /// 不再能写出"用相机自带比例"的形态。
    #[test]
    fn aspect_ratio_only_comes_from_the_viewport() {
        let camera = camera();
        // 组件串文法里从来没有 aspect 字段：往返只搬运姿态与投影参数。
        let payload = camera.to_component_str();
        assert!(
            !payload.contains("aspect"),
            "组件串不该出现 aspect 字段：{payload}"
        );
        assert_eq!(Camera3D::from_component_str(&payload).unwrap(), camera);
        // 两种比例给出两份矩阵，且都等于"按该视口比例现算"的那一份。
        let a = viewport(640.0, 360.0);
        let b = viewport(360.0, 640.0);
        assert_ne!(
            camera.view_projection_matrix(a).unwrap(),
            camera.view_projection_matrix(b).unwrap()
        );
        assert_eq!(
            camera.view_projection_matrix(a).unwrap(),
            camera.projection_matrix(a).unwrap() * camera.view_matrix().unwrap()
        );
        assert_eq!(a.aspect(), 640.0 / 360.0);
        assert_eq!(b.aspect(), 360.0 / 640.0);
    }

    /// WP5 基线（2D 侧）：剔除用的 `visible_world_bounds` 与绘制用的 `world_to_screen` 必须是
    /// 同一套换算——各算各的就会出现"剔除认为不可见、绘制却要画"的静默错位。
    #[test]
    fn visible_world_bounds_agrees_with_the_projection() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let camera = Camera2D::new(Vec2::new(10.0, 20.0), 2.0).unwrap();
        let bounds = camera.visible_world_bounds(viewport).unwrap();
        // 半宽 = 200/2/2 = 50，半高 = 100/2/2 = 25。
        assert_vec_close(bounds.min, Vec2::new(-40.0, -5.0), 0.0001);
        assert_vec_close(bounds.max, Vec2::new(60.0, 45.0), 0.0001);
        // 世界 y 向上、屏幕 y 向下：屏幕左上角对应 (min.x, max.y)。
        assert_vec_close(
            camera.screen_to_world(Vec2::zero(), viewport).unwrap(),
            Vec2::new(bounds.min.x, bounds.max.y),
            0.0001,
        );
        assert_vec_close(
            camera
                .screen_to_world(Vec2::new(200.0, 100.0), viewport)
                .unwrap(),
            Vec2::new(bounds.max.x, bounds.min.y),
            0.0001,
        );

        // 旋转后 bounds 是外接 AABB：视口四角仍必须落在其中（宁可多留不可少算）。
        let rotated_camera = camera.with_rotation(std::f32::consts::FRAC_PI_6).unwrap();
        let rotated = rotated_camera.visible_world_bounds(viewport).unwrap();
        for corner in [(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)] {
            let world = rotated_camera
                .screen_to_world(Vec2::new(corner.0, corner.1), viewport)
                .unwrap();
            assert!(
                world.x >= rotated.min.x - 0.0001
                    && world.x <= rotated.max.x + 0.0001
                    && world.y >= rotated.min.y - 0.0001
                    && world.y <= rotated.max.y + 0.0001,
                "旋转后视口角点 {world:?} 落在 bounds 之外：{rotated:?}"
            );
        }
        // 有边界钳制时，bounds 必须跟着**钳制后的**中心走（否则剔除域与画面不一致）。
        let clamped = camera
            .with_bounds(CameraBounds::new(Vec2::zero(), Vec2::new(1000.0, 1000.0)).unwrap())
            .unwrap()
            .visible_world_bounds(viewport)
            .unwrap();
        assert_vec_close(clamped.min, Vec2::zero(), 0.0001);
        assert_vec_close(clamped.max, Vec2::new(100.0, 50.0), 0.0001);
    }

    /// WP5：相机姿态可以来自实体的 `Transform3D`（`pose=transform`）。
    ///
    /// 三个要点：① 默认不开启且规范字符串不变；② 开启后位置/视线/上方向都来自变换
    /// （视线 = 变换的 -Z、上方向 = 变换的 +Y）；③ 姿态无法表达时报错，不静默退回。
    #[test]
    fn camera_pose_can_come_from_a_transform() {
        let base = camera(); // 默认 look-at 姿态
        assert!(!base.pose_from_transform, "默认必须是不跟随变换");
        assert!(
            !base.to_component_str().contains("pose="),
            "默认不写 pose 段，既有规范字符串不变"
        );

        let parsed = Camera3D::from_component_str(
            "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform",
        )
        .unwrap();
        assert!(parsed.pose_from_transform);
        assert!(parsed.to_component_str().ends_with(";pose=transform"));
        // 拼错的值必须报错，不能当默认。
        assert!(
            Camera3D::from_component_str(
                "position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=guess"
            )
            .is_err()
        );

        // 姿态：实体在 (5, 1, 0) 绕 Y 转 90°（右手系）⇒ 视线朝 -X。
        let pose = Transform3D {
            position: Vec3::new(5.0, 1.0, 0.0),
            rotation: Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2).unwrap(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let posed = base.with_pose(pose).unwrap();
        assert!(posed.pose_from_transform);
        assert!((posed.position - pose.position).length() < 1e-5);
        let forward = (posed.target - posed.position).normalized().unwrap();
        assert!(
            (forward - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-4,
            "绕 Y 转 90° 后视线应为 -X：{forward:?}"
        );
        assert!(
            (posed.up - Vec3::Y).length() < 1e-4,
            "上方向应仍是 +Y：{:?}",
            posed.up
        );
        // 投影参数不受姿态影响。
        assert!((posed.fov_y - base.fov_y).abs() < 1e-6);
        assert!((posed.far - base.far).abs() < 1e-6);

        // 退化姿态必须报错，而不是悄悄用回旧姿态。
        let mut degenerate = pose;
        degenerate.rotation = Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 0.0,
        };
        assert!(base.with_pose(degenerate).is_err());
    }

    /// WP5 收敛本身：统一的 [`Projection`] 必须与两套相机的既有数值**一致**，
    /// [`RenderView`] 与 [`CameraSelection`] 给出"一帧怎么投影、哪台相机生效"的单一说法。
    #[test]
    fn unified_projection_and_render_view_match_both_families() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let camera2d = Camera2D::new(Vec2::new(100.0, 50.0), 2.0).unwrap();
        let camera3d = camera();

        // 正交投影的视野半宽 = 视口宽 / 2 / zoom（像素 → 世界单位），深度不参与。
        let orthographic = Projection::from_camera2d(camera2d);
        assert_eq!(orthographic, Projection::Orthographic { zoom: 2.0 });
        assert!(orthographic.is_orthographic() && !orthographic.uses_depth());
        // 透视侧沿用同一个 `Camera3D` 实现，因此矩阵逐元素相同（收敛不换算法）。
        let perspective = Projection::from_camera3d(camera3d);
        assert_eq!(
            perspective,
            Projection::Perspective {
                fov_y: camera3d.fov_y,
                near: camera3d.near,
                far: camera3d.far
            }
        );
        assert!(perspective.uses_depth() && !perspective.is_orthographic());

        // 渲染视图：2D overlay 不清屏、后画；3D 主视图清屏、先画——顺序与清屏策略只有这一处说法。
        let overlay = RenderView::overlay_2d(camera2d, viewport);
        let main = RenderView::main_3d(camera3d, viewport, [0.1, 0.2, 0.3, 1.0]);
        assert!(overlay.order > main.order, "overlay 必须画在主视图之后");
        assert_eq!(overlay.clear, ClearPolicy::None);
        assert_eq!(main.clear, ClearPolicy::Color([0.1, 0.2, 0.3, 1.0]));
        assert!((overlay.aspect() - main.aspect()).abs() < 1e-6);

        // 多相机选择：显式选中优先，否则退回排序序第一个（同一输入必须给同一结果）。
        let mut selection = CameraSelection::new();
        let candidates = [(7u64, "seven"), (3, "three"), (9, "nine")];
        assert_eq!(selection.resolve(candidates), Some("seven"));
        selection.select(Some(9));
        assert_eq!(selection.resolve(candidates), Some("nine"));
        selection.select(Some(42));
        assert_eq!(
            selection.resolve(candidates),
            Some("seven"),
            "选中的实体不存在时退回第一个候选，而不是什么都不画"
        );
        assert_eq!(selection.resolve(std::iter::empty::<(u64, &str)>()), None);
    }

    /// WP6：2D 相机的**矩阵形式**必须与既有的逐点换算完全一致（否则"世界 Sprite 走深度通道"
    /// 会把 2D 内容画到别处）。这条同时钉住 y 翻转、旋转、缩放与深度范围两端。
    #[test]
    fn two_d_view_projection_matches_the_pointwise_projection() {
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        for rotation in [0.0_f32, 0.35, -1.2] {
            for zoom in [1.0_f32, 2.5] {
                let camera = Camera2D {
                    center: Vec2::new(100.0, 50.0),
                    rotation,
                    zoom,
                    bounds: None,
                    enabled: true,
                };
                let matrix = camera.view_projection(viewport, 0.0, 100.0).unwrap();
                for world in [
                    Vec2::new(100.0, 50.0),
                    Vec2::new(160.0, 50.0),
                    Vec2::new(100.0, 90.0),
                    Vec2::new(40.0, 10.0),
                ] {
                    // 逐点换算 → 屏幕像素 → NDC。
                    let screen = camera.world_to_screen(world, viewport).unwrap();
                    let expected_x = screen.x / viewport.width * 2.0 - 1.0;
                    let expected_y = 1.0 - screen.y / viewport.height * 2.0;
                    let ndc = matrix
                        .transform_point(Vec3::new(world.x, world.y, 0.0))
                        .unwrap();
                    assert!(
                        (ndc.x - expected_x).abs() < 1e-4 && (ndc.y - expected_y).abs() < 1e-4,
                        "rotation={rotation} zoom={zoom} world={world:?}: 矩阵 {ndc:?} vs 逐点 ({expected_x}, {expected_y})"
                    );
                    // 世界 z = near/far 正好映射到 NDC z = 0/1（与 3D 同一约定）。
                    assert!(ndc.z.abs() < 1e-5, "near 平面应映射到 z=0：{ndc:?}");
                    let far_ndc = matrix
                        .transform_point(Vec3::new(world.x, world.y, 100.0))
                        .unwrap();
                    assert!(
                        (far_ndc.z - 1.0).abs() < 1e-5,
                        "far 平面应映射到 z=1：{far_ndc:?}"
                    );
                }
            }
        }
        // 非法深度范围必须报错，而不是产出一个把深度压平的矩阵。
        let camera = Camera2D::new(Vec2::zero(), 1.0).unwrap();
        assert!(camera.view_projection(viewport, 10.0, 10.0).is_err());
        assert!(camera.view_projection(viewport, 0.0, f32::NAN).is_err());
    }
    /// WP5 接线：渲染器消费的是 [`FrameViews`]——清屏由**顺序最前**的已声明视图给出，绘制顺序由
    /// `order` 决定；**什么都没声明**时清屏策略为 `None`，渲染器据此回退到历史默认。
    ///
    /// docs/54 §4.2：本批之前清屏硬编码取 `main`，`order` 因此是个没有读者的字段。现在两者共用
    /// [`FrameViews::ordered`] 一处规则：声明一个 `order` 更小的视图就能接管清屏。唯一的语义变化是
    /// **只声明 overlay** 的场景（此前退回历史默认色、现在由该视图的 `ClearPolicy` 决定）——仓库里
    /// 没有这样的宿主（编辑器与运行时都是"声明 main"），下面这条断言把这个变化钉住。
    #[test]
    fn frame_views_declare_clear_and_draw_order() {
        let viewport = Viewport::new(320.0, 180.0).unwrap();
        let camera2d = Camera2D::new(Vec2::new(10.0, 20.0), 1.0).unwrap();
        let overlay = RenderView::overlay_2d(camera2d, viewport);

        // 什么都没声明：渲染器保持历史默认（默认清屏色 + 整目标视口）。
        assert_eq!(FrameViews::new().clear(), None);
        assert_eq!(FrameViews::default().clear(), None);

        // 纯 2D：只有 overlay 视图 ⇒ **由它决定清屏**（`overlay_2d` 声明的是"不清屏"，因此渲染器
        // 不再清屏——这是"清屏由顺序最前的视图说了算"的直接后果，见上面的测试说明）。
        let two_d = FrameViews::overlay_only(overlay);
        assert_eq!(two_d.clear(), Some(ClearPolicy::None));
        assert_eq!(two_d.ordered(), [Some(overlay), None]);

        // 2D + 3D：清屏色由主视图给出，顺序固定为"主视图先、overlay 后"。
        let main = RenderView::main_3d(camera(), viewport, DEFAULT_3D_CLEAR);
        let layered = FrameViews::layered(main, overlay);
        assert_eq!(layered.clear(), Some(ClearPolicy::Color(DEFAULT_3D_CLEAR)));
        assert_eq!(layered.ordered(), [Some(main), Some(overlay)]);

        // 主视图显式声明"不清屏"时，宿主可以把它叠到已有内容上。
        let no_clear = FrameViews {
            main: Some(main.with_clear(ClearPolicy::None)),
            overlay: Some(overlay),
        };
        assert_eq!(no_clear.clear(), Some(ClearPolicy::None));

        // overlay 声明的 order 比主视图更小 ⇒ 它先画（顺序由声明决定，不是写死的）。
        let early_overlay = FrameViews::layered(
            main,
            // 显式声明清屏色：证明"谁清屏"真的跟着 `order` 走（而不是永远取 main）。
            overlay
                .with_order(-1)
                .with_clear(ClearPolicy::Color([0.0, 1.0, 0.0, 1.0])),
        );
        assert_eq!(early_overlay.ordered()[0].map(|view| view.order), Some(-1));
        // docs/54 §4.2：`order` 现在真的有消费者——清屏由**顺序最前**的视图说了算。
        assert_eq!(
            early_overlay.clear(),
            Some(ClearPolicy::Color([0.0, 1.0, 0.0, 1.0])),
            "order 更小的 overlay 应当拥有本帧的清屏策略"
        );
        assert_eq!(
            layered.clear(),
            Some(ClearPolicy::Color(DEFAULT_3D_CLEAR)),
            "顺序相等时 main 优先（既有宿主行为不变）"
        );
    }
}
#[cfg(test)]
mod controller_tests {
    use crate::camera::*;
    use rustlua_ecs::entity::EntityId;
    use rustlua_ecs::transform::Vec2;

    fn controller() -> Camera2dController {
        let mut controller = Camera2dController::default();
        controller.follow(Some(EntityId::from_raw(1, 1)));
        controller
    }

    /// WP5 收敛后的新边界：请求消费 → 跟随 → advance → bounds 钳制的**次序**固定在这一处，
    /// 宿主不再自己拼；`resolve_view` 再把它交给统一的 [`RenderView`]。
    #[test]
    fn resolve_camera_owns_request_follow_and_bounds_order() {
        let viewport = Viewport::new(200.0, 100.0).unwrap();
        let mut controller = Camera2dController::default();
        let base = Camera2D::new(Vec2::new(500.0, 500.0), 1.0)
            .unwrap()
            .with_bounds(CameraBounds::new(Vec2::zero(), Vec2::new(1000.0, 1000.0)).unwrap())
            .unwrap();

        // ① 脚本请求优先，且**取走即清**：第二次推进不再重放同一个请求。
        controller.request_center(Vec2::new(200.0, 120.0));
        controller.request_zoom(3.0);
        let resolved = controller.resolve_camera(base, None, viewport, 1.0 / 60.0);
        assert_eq!(resolved.zoom, 3.0);
        assert_eq!(resolved.center, Vec2::new(200.0, 120.0));
        let again = controller.resolve_camera(resolved, None, viewport, 1.0 / 60.0);
        assert_eq!(again.center, Vec2::new(200.0, 120.0), "请求只消费一次");

        // ② 跟随 + 立即跟随（smoothing = 0）：中心落到 目标 + 偏移，再被 bounds 钳制。
        //    视口 200×100、zoom 1 ⇒ 半宽 100、半高 50 ⇒ 中心最小为 (100, 50)。
        controller.follow(Some(EntityId::from_raw(1, 1)));
        controller.set_offset(Vec2::new(0.0, -20.0));
        let followed =
            controller.resolve_camera(base, Some(Vec2::new(5.0, 5.0)), viewport, 1.0 / 60.0);
        assert_eq!(
            followed.center,
            Vec2::new(100.0, 50.0),
            "bounds 必须把整个视口夹进世界矩形"
        );

        // ③ 同一条路径给出统一视图描述（2D overlay：正交 + 不清屏）。
        let view =
            controller.resolve_view(base, Some(Vec2::new(500.0, 500.0)), viewport, 1.0 / 60.0);
        assert_eq!(view.clear, ClearPolicy::None);
        assert!(view.projection.is_orthographic());
        assert_eq!(view.viewport, viewport);
    }
    #[test]
    fn without_follow_the_center_is_untouched() {
        let mut controller = Camera2dController::default();
        let center = Vec2::new(10.0, 20.0);
        assert_eq!(
            controller.advance(center, Some(Vec2::new(500.0, 500.0)), 1.0 / 60.0),
            center
        );
    }

    #[test]
    fn immediate_follow_snaps_to_target_plus_offset() {
        let mut controller = controller();
        controller.set_offset(Vec2::new(0.0, -32.0));
        let center = controller.advance(Vec2::zero(), Some(Vec2::new(100.0, 50.0)), 1.0 / 60.0);
        assert_eq!(center, Vec2::new(100.0, 18.0));
    }

    #[test]
    fn dead_zone_holds_still_until_the_target_leaves_it() {
        let mut controller = controller();
        controller.set_dead_zone(Vec2::new(16.0, 16.0));
        let current = Vec2::new(100.0, 100.0);
        // 目标在死区内：不动。
        assert_eq!(
            controller.advance(current, Some(Vec2::new(110.0, 90.0)), 0.016),
            current
        );
        // 超出死区 24px：推进到只差死区边界（x 方向超 24，推 24-16=8）。
        let moved = controller.advance(current, Some(Vec2::new(124.0, 100.0)), 0.016);
        assert!((moved.x - 108.0).abs() < 1e-4, "{moved:?}");
        assert!((moved.y - 100.0).abs() < 1e-4, "{moved:?}");
    }

    #[test]
    fn smoothing_converges_independently_of_frame_rate() {
        let mut coarse = controller();
        coarse.set_smoothing(6.0);
        let mut fine = controller();
        fine.set_smoothing(6.0);
        let target = Some(Vec2::new(100.0, 0.0));

        // 同样的 1 秒总时长，步长差 4 倍，指数收敛的结果应当几乎一致。
        let mut coarse_center = Vec2::zero();
        for _ in 0..10 {
            coarse_center = coarse.advance(coarse_center, target, 0.1);
        }
        let mut fine_center = Vec2::zero();
        for _ in 0..40 {
            fine_center = fine.advance(fine_center, target, 0.025);
        }
        assert!(
            (coarse_center.x - fine_center.x).abs() < 1.0,
            "{} vs {}",
            coarse_center.x,
            fine_center.x
        );
        // 且确实在向目标收敛。
        assert!(coarse_center.x > 90.0, "{}", coarse_center.x);
    }

    #[test]
    fn shake_decays_to_zero_and_is_deterministic() {
        let mut first = controller();
        first.shake(8.0, 0.2);
        let mut second = controller();
        second.shake(8.0, 0.2);
        let mut previous = Vec2::zero();
        let mut moved_off_center = false;
        for _ in 0..20 {
            let a = first.advance(Vec2::zero(), None, 1.0 / 60.0);
            let b = second.advance(Vec2::zero(), None, 1.0 / 60.0);
            assert_eq!(a, b, "同一帧序列必须给出同一抖动结果");
            if a.x.abs() > 1e-6 || a.y.abs() > 1e-6 {
                moved_off_center = true;
            }
            previous = a;
        }
        assert!(moved_off_center, "抖动期间应偏离基准中心");
        assert!(!first.is_shaking(), "0.2s 后抖动应结束");
        // 结束后回到基准中心。
        assert!(
            previous.x.abs() < 1e-3 && previous.y.abs() < 1e-3,
            "{previous:?}"
        );
    }

    #[test]
    fn rejects_degenerate_configuration() {
        let mut controller = Camera2dController::default();
        controller.set_smoothing(f32::NAN);
        assert_eq!(controller.smoothing(), 0.0);
        controller.set_smoothing(-3.0);
        assert_eq!(controller.smoothing(), 0.0);
        controller.shake(f32::NAN, 0.1);
        assert!(!controller.is_shaking());
        controller.shake(4.0, 0.0);
        assert!(!controller.is_shaking());
    }

    #[test]
    fn huge_delta_is_clamped_to_keep_motion_predictable() {
        let mut controller = controller();
        controller.set_smoothing(6.0);
        let moved = controller.advance(Vec2::zero(), Some(Vec2::new(1000.0, 0.0)), 10.0);
        // 步长被夹到 0.1s：收敛比例 = 1-e^-0.6 ≈ 0.451，而不是一步到位。
        assert!(moved.x > 400.0 && moved.x < 480.0, "{moved:?}");
    }
}
