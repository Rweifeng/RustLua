// P3-R244：路径流实例位姿的 GPU 求值。
//
// 输入是"扁平的路径 + 逐实例运动参数"，输出是 N 份世界矩阵（列主序，64 B/实例）。
// 求值顺序与 CPU `BatchMotion3D::instance_matrix` 同式：弧长取模/夹取 → 段内插值 →
// 前向轴 yaw（align 时）→ T * R * S。
//
// 与 CPU 的一处**刻意不同**：CPU 把非法实例（缩放 0 / 非有限、align 段切线水平长度 0）
// 从矩阵表里丢掉；默认入口写**零矩阵**（画面同样不出现，但实例数保持 N）。
// P3-R251 的 `*_compact` 入口改成"给可见实例领前缀槽位 + indirect 参数带真实实例数"，
// 顺序由原子加决定、不保证稳定；两套入口的画面等价（对账门按集合比对）。
// 这一点写在 `renderer/batch_motion_gpu.rs` 的模块文档里，对账门只用合法实例。

struct Params {
    // 场景时钟（秒）。
    time: f32,
    // 批次速度（世界单位/秒）。
    batch_speed: f32,
    // 路径总长（闭合时含末→首那一段）。
    total: f32,
    // 1 = 闭合路径（弧长按总长循环），0 = 开口（夹到 [0, total]）。
    closed: u32,
    // 路径节点数（>= 2）。
    node_count: u32,
    // 本批实例数。
    instance_count: u32,
    // 目标矩阵的起始下标（目标缓冲按 64 B/矩阵整体绑定）。
    base_matrix: u32,
    // P3-R245：导入设置里的模型缩放（与 CPU `scaled_model` 同式：只乘 3x3，不动平移）。
    model_scale: f32,
    // 法线矩阵目标区的起始下标（只有 `main_normals` 读；`main` 用不到但同布局）。
    normal_base_matrix: u32,
    // P3-R255：1 = 逐实例视锥剔除开；0 = 关（默认）。
    frustum_enabled: u32,
    // P3-R255：网格局部包围球半径（从局部原点算，含球心偏移），逐实例再乘 max|scale|·model_scale。
    local_radius: f32,
    // P3-R273：LOD 侧（0 = 关，1 = 近侧 dist <= 第一台阶，2 = 中侧，3 = 远侧）。
    // 侧 2/3 的分界见 `lod_distance_far`；没有第二台阶时侧 2 与侧 3 同义。
    lod_side: u32,
    // P3-R250：逐实例距离剔除。xyz = 剔除球心（相机世界位置），w = 半径（米）；
    // w <= 0 = 关闭（默认，逐矩阵与开档前逐字节相同）。
    cull_origin: vec3<f32>,
    cull_distance: f32,
    // P3-R266：远程密度档。离 `cull_origin`（同一条相机位置）超过 `thin_distance` 的实例
    // 只保留 `index % thin_stride == 0` 的那些；`thin_stride < 2` 或距离 <= 0 = 关（默认）。
    // 判据用**批次内实例下标**而不是位置哈希：下标是静态的 ⇒ 同一只每帧留/走一致，不闪。
    thin_distance: f32,
    thin_stride: u32,
    // P3-R273：LOD 第一台阶半径（米，离 `cull_origin`）；`<= 0` 或 `lod_side == 0` = 关。
    lod_distance: f32,
    // P3-R274：LOD 第二台阶半径（米）；`<= lod_distance` = 没有第二台阶（中/远同侧）。
    lod_distance_far: f32,
    // P3-R267：第二远带（可选）。每带是**交集**语义：第一带已留下标集合，第二带在该集合上再抽。
    // `thin_stride2 < 2` 或距离 <= 0 = 关（默认）。
    thin_distance2: f32,
    thin_stride2: u32,
    _pad2: vec2<f32>,
    // P3-R255：主视图视锥六面（归一化，[nx, ny, nz, d]，正 = 内侧）。关档时全 0。
    frustum: array<vec4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> params: Params;
// 路径节点（xyz 有效，w 恒 0）。
@group(0) @binding(1) var<storage, read> nodes: array<vec4<f32>>;
// 节点累计弧长（与 nodes 一一对应）。
@group(0) @binding(2) var<storage, read> cumulative: array<f32>;
// 逐实例 3 个 vec4：[offset, speed, flags, 0] / [qx, qy, qz, qw] / [sx, sy, sz, 0]。
// flags：bit0 = align（跟随切线），bit1 = 前向轴是 +Z（否则 +X）。
@group(0) @binding(3) var<storage, read> instances: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> matrices: array<mat4x4<f32>>;

// P3-R245：帧路径同时要的逐实例法线矩阵（模型矩阵 3x3 的逆转置）。
// 只有 `main_normals` 入口用；`main` 的管线布局不含它。
@group(0) @binding(5) var<storage, read_write> normal_matrices: array<mat4x4<f32>>;

// P3-R251：压缩通道。存活实例用原子计数器领前缀槽位（顺序不确定，但画面等价）。
// P3-R271：计数器就是 indirect 参数的第 2 个 u32（instance_count）——atomicAdd 的返回值
// 直接当槽位，计数与参数是同一次原子操作 ⇒ 不再需要单独的 counter 缓冲与收尾 kernel。
@group(0) @binding(6) var<storage, read_write> compact_args: array<atomic<u32>>;

// 不用 `isNan`/`isInf`：wgpu 27 的 WGSL 前端里没有这两个内建，等价写法是"自反 + 上界"。
fn finite(value: f32) -> bool {
    return value == value && abs(value) < 1e38;
}

// 与 `Mat4::from_quat` 同式：非单位四元数先归一化，长度太小退回单位阵（identity 四元数）。
fn rotation_matrix(q: vec4<f32>) -> mat3x3<f32> {
    let length = sqrt(dot(q, q));
    var unit = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if (length > 1.1920929e-7) {
        unit = q / length;
    }
    let x = unit.x;
    let y = unit.y;
    let z = unit.z;
    let w = unit.w;
    return mat3x3<f32>(
        vec3<f32>(1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)),
        vec3<f32>(2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)),
        vec3<f32>(2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)),
    );
}

fn zero_matrix() -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
    );
}

struct Pose {
    // P3-R251：1 = 这个实例会真的被画出来（合法且未被剔除）；0 = 退化/剔除。
    // 非压缩入口不看它（零矩阵照写、实例数保持 N）；压缩入口只给 1 领槽位。
    visible: u32,
    matrix: mat4x4<f32>,
    // CPU `pbr::normal_matrix` 是模型矩阵 4x4 逆矩阵的转置；模型 = T * R * S 且 S 对角 ⇒
    // 前三个 3x3 列就是 `R 的第 j 列 / (s_j · model_scale)`，第四列 [0,0,0,1]。
    normal: mat4x4<f32>,
};

// 单实例求值。非法实例（缩放 0/非有限、align 切线水平长度为 0）返回两份零矩阵——
// 与 CPU `filter_map` 丢弃等价（画面同样不出现），但实例数保持 N（见模块文档）。
fn evaluate(index: u32) -> Pose {
    var pose: Pose;
    pose.visible = 0u;
    pose.matrix = zero_matrix();
    pose.normal = zero_matrix();

    let base = index * 3u;
    let head = instances[base];
    let static_rotation = instances[base + 1u];
    let scale = instances[base + 2u].xyz;

    for (var axis = 0u; axis < 3u; axis = axis + 1u) {
        if (!finite(scale[axis]) || scale[axis] == 0.0) {
            return pose;
        }
    }

    let flags = u32(head.z);
    var distance = head.x + head.y * params.batch_speed * params.time;
    if (params.closed == 1u) {
        // f32::rem_euclid：先取余再补回正区间。
        distance = distance - floor(distance / params.total) * params.total;
        if (distance < 0.0) {
            distance = distance + params.total;
        }
    } else {
        distance = clamp(distance, 0.0, params.total);
    }

    // 定位段：与 CPU `partition_point(cumulative <= s)` 同口径（累计长度 <= s 的节点数减一）。
    var passed = 0u;
    for (var node = 0u; node < params.node_count; node = node + 1u) {
        if (cumulative[node] <= distance) {
            passed = node + 1u;
        }
    }
    let last = params.node_count - 1u;
    // `cumulative[0]` 恒为 0 ⇒ `distance >= 0` 时 `passed >= 1`，段号就是 `passed - 1`。
    // 别把它写成"至少 1"：那会把落在**第一段**的实例整段推进到第二段（起点差一整条边）。
    var segment = 0u;
    if (passed > 0u) {
        segment = passed - 1u;
    }
    if (segment > last) {
        segment = last;
    }
    let start = cumulative[segment];
    var end = params.total;
    if (segment < last) {
        end = cumulative[segment + 1u];
    }
    let length = end - start;
    var u = 0.0;
    if (length > 0.0) {
        u = clamp((distance - start) / length, 0.0, 1.0);
    }

    let p0 = nodes[segment].xyz;
    // 末段（闭合路径的末→首）在 CPU 里取 nodes[0]；开口路径走到末段时 length = 0 ⇒ u = 0，
    // 端点不参与插值，取哪个都一样，为了与 CPU 的 `segment_points` 同形也取 nodes[0]。
    var p1 = nodes[0].xyz;
    if (segment < last) {
        p1 = nodes[segment + 1u].xyz;
    }
    let position = p0 + (p1 - p0) * u;

    // P3-R250：逐实例距离剔除——离剔除球心超过半径的实例写零矩阵（与非法实例同一条
    // 退化路径，实例数仍保持 N）。矩阵随后同时供主视图/阴影/planar 用 ⇒ 这是一条
    // **全链一致**的取舍，不是只掐某一条 pass。
    if (params.cull_distance > 0.0) {
        let delta = position - params.cull_origin;
        if (dot(delta, delta) > params.cull_distance * params.cull_distance) {
            return pose;
        }
    }
    // P3-R266：远程密度档——同一批里远的那部分只留 1/thin_stride。下标口径是批次内静态
    // 下标（不是哈希）⇒ 留下的那几只每帧固定，不会闪；近距离一只不动（逐字节不变）。
    if (params.thin_stride > 1u && params.thin_distance > 0.0) {
        let thin_delta = position - params.cull_origin;
        if (dot(thin_delta, thin_delta) > params.thin_distance * params.thin_distance
            && index % params.thin_stride != 0u) {
            return pose;
        }
    }
    // P3-R267：第二远带——比第一带更远的那部分再抽一次（下标仍是批次内静态下标）。
    if (params.thin_stride2 > 1u && params.thin_distance2 > 0.0) {
        let thin2_delta = position - params.cull_origin;
        if (dot(thin2_delta, thin2_delta) > params.thin_distance2 * params.thin_distance2
            && index % params.thin_stride2 != 0u) {
            return pose;
        }
    }
    // P3-R273：网格 LOD 侧选择。P3-R274：三档——近（1）`dist <= 第一台阶`、中（2）夹在
    // 两台阶之间、远（3）`dist > 第二台阶`；三档两两互斥且合起来完备（并集 = 关 LOD 时
    // 会画出来的全部存活实例）。第二台阶没开（`lod_distance_far <= lod_distance`）时中/远
    // 同义，与 R273 的两档行为逐字节一致。关档（lod_side=0 或半径 <= 0）这一段不执行，
    // 逐矩阵与开档前逐字节相同。NaN 位置按"未过第一台阶"处理（与 R273 同口径），保证
    // 病态输入也不会把实例漏给所有档。
    if (params.lod_side != 0u && params.lod_distance > 0.0) {
        let lod_delta = position - params.cull_origin;
        let lod_d2 = dot(lod_delta, lod_delta);
        let beyond_near = lod_d2 > params.lod_distance * params.lod_distance;
        let far_band = params.lod_distance_far > params.lod_distance;
        let beyond_far = far_band
            && lod_d2 > params.lod_distance_far * params.lod_distance_far;
        var keep_lod = false;
        if (params.lod_side == 1u) {
            keep_lod = !beyond_near;
        } else if (params.lod_side == 2u) {
            keep_lod = beyond_near && !beyond_far;
        } else {
            keep_lod = beyond_far;
        }
        if (!keep_lod) {
            return pose;
        }
    }
    // P3-R255：逐实例视锥剔除（保守包围球：实例位置 ± 局部半径×max|scale|×model_scale）。
    // 球心在任一平面外侧一个半径之外 ⇒ 该实例在主视图里一个像素都不会出，写零矩阵/不领槽位。
    // 与 P3-R250 同一条退化路径；矩阵同时供主视图/阴影/planar ⇒ 全链一致的取舍。
    if (params.frustum_enabled != 0u) {
        let extent = max(max(abs(scale.x), abs(scale.y)), abs(scale.z)) * params.model_scale;
        let radius = params.local_radius * extent;
        for (var plane_index = 0u; plane_index < 6u; plane_index = plane_index + 1u) {
            let plane = params.frustum[plane_index];
            if (dot(plane.xyz, position) + plane.w < -radius) {
                return pose;
            }
        }
    }

    var rotation = static_rotation;
    if ((flags & 1u) != 0u) {
        let dx = p1.x - p0.x;
        let dz = p1.z - p0.z;
        let horizontal = sqrt(dx * dx + dz * dz);
        if (!(horizontal > 0.0)) {
            return pose;
        }
        // 前向轴口径与 CPU 同式：+X 前向 ⇒ yaw = atan2(−dz, dx)；+Z 前向 ⇒ atan2(dx, dz)。
        var yaw = atan2(-dz, dx);
        if ((flags & 2u) != 0u) {
            yaw = atan2(dx, dz);
        }
        let half = yaw * 0.5;
        rotation = vec4<f32>(0.0, sin(half), 0.0, cos(half));
    } else if (!finite(rotation.x) || !finite(rotation.y) || !finite(rotation.z) || !finite(rotation.w)) {
        return pose;
    }

    let r = rotation_matrix(rotation);
    let s = scale * params.model_scale;
    // T * R * S：三列的旋转部分各自乘缩放，第四列是平移；列主序。
    pose.matrix = mat4x4<f32>(
        vec4<f32>(r[0] * s.x, 0.0),
        vec4<f32>(r[1] * s.y, 0.0),
        vec4<f32>(r[2] * s.z, 0.0),
        vec4<f32>(position, 1.0),
    );
    pose.normal = mat4x4<f32>(
        vec4<f32>(r[0] / s.x, 0.0),
        vec4<f32>(r[1] / s.y, 0.0),
        vec4<f32>(r[2] / s.z, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    pose.visible = 1u;
    return pose;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= params.instance_count) {
        return;
    }
    matrices[params.base_matrix + index] = evaluate(index).matrix;
}

@compute @workgroup_size(64)
fn main_normals(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= params.instance_count) {
        return;
    }
    let pose = evaluate(index);
    matrices[params.base_matrix + index] = pose.matrix;
    normal_matrices[params.normal_base_matrix + index] = pose.normal;
}

// P3-R251：压缩入口——只给可见实例领前缀槽位（顺序由原子加决定，不保证稳定）。
// P3-R271：`compact_args[1]` 既是 instance_count 又是槽位分配器，主 kernel 结束即参数就绪。
@compute @workgroup_size(64)
fn main_compact(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= params.instance_count) {
        return;
    }
    let pose = evaluate(index);
    if (pose.visible == 0u) {
        return;
    }
    let slot = atomicAdd(&compact_args[1], 1u);
    matrices[params.base_matrix + slot] = pose.matrix;
}

@compute @workgroup_size(64)
fn main_normals_compact(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= params.instance_count) {
        return;
    }
    let pose = evaluate(index);
    if (pose.visible == 0u) {
        return;
    }
    let slot = atomicAdd(&compact_args[1], 1u);
    matrices[params.base_matrix + slot] = pose.matrix;
    normal_matrices[params.normal_base_matrix + slot] = pose.normal;
}
