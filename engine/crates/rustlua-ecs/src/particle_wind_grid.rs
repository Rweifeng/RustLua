//! 网格体积风场 + 温度浮力场（P3-R233）。
//!
//! 此前的风只有全局向量（`set_wind`）、高度剪切/阵风（`set_wind_profile`）、4 个解析
//! 局部风源（`set_wind_source`）与程序化 curl 噪声——都不是**可沉积、可扩散的空间场**：
//! 火柱不能往格子里注入热气、热气不能随时间上升外扩、粒子不能按所在格子读同一份风速。
//! 这里给出一张轴对齐的三维速度网格 + 温度网格：显式扩散（Jacobi 迭代）+ 逐格衰减 +
//! 温度浮力（热气把速度往世界 `+y` 抬），再三线性采样成粒子加速度。
//!
//! 契约（CPU/GPU 后续 parity 都按这一份）：
//! - 坐标系与 `particles.burst3d` 一致——采样点 `(x, height, z)`，`height` 是世界竖直轴。
//! - `sample_accel` 直接返回**加速度**（单位/秒²），与 `wind`/`curl` 同口径，不再乘寿命。
//! - 网格原点 `origin` 是格 `(0,0,0)` 中心的世界坐标；节点 `i` 落在 `origin + i*cell`。
//!   盒外按边界钳制（clamp-to-edge），与纹理采样一致。
//! - 没有场（`ParticleSystem` 未 `set_wind_grid`）时逐粒子积分逐位不变。

use rustlua_math::math3d::Vec3;

/// 每轴节点上限（含边界）；`nx*ny*nz` 因此 ≤ 32³ = 32768。
pub const MAX_WIND_GRID_AXIS: usize = 32;
/// 网格总格数上限。
pub const MAX_WIND_GRID_CELLS: usize = MAX_WIND_GRID_AXIS.pow(3);

/// 轴对齐三维风场：速度 `velocity`（单位/秒²）与温度 `temperature`（无量纲，浮力源）。
#[derive(Clone)]
pub struct WindGrid {
    dims: [usize; 3],
    origin: Vec3,
    cell: f32,
    velocity: Vec<Vec3>,
    temperature: Vec<f32>,
    /// 每步扩散混合系数 `0..~0.16`（显式稳定上限 1/6），乘 `dt*60` 归一到 60 Hz。
    diffusion: f32,
    /// 每秒速度衰减比例（≥0）。
    velocity_decay: f32,
    /// 每秒温度衰减比例（≥0）。
    temperature_decay: f32,
    /// 温度→上升加速度增益（单位/秒² 每温度单位）。
    buoyancy: f32,
    /// 每步 Jacobi 扩散迭代次数 `1..=8`。
    iterations: u32,
    /// 扩散用的第二缓冲：构造时按格数一次分配，步进时复用。
    ///
    /// 此前每步 `diffuse_*` 都 `clone()` 整场（8192 格 = 98 KB + 32 KB），一次步进两次
    /// 大额分配 ⇒ 每次都走分配器的大块路径，页错误把步进代价抬到"plain 臂"的 15 倍。
    scratch_velocity: Vec<Vec3>,
    scratch_temperature: Vec<f32>,
}

// 语义比较只关心场本身与系数；scratch 是实现细节（内容是上一步的旧场），不参与相等判断。
impl PartialEq for WindGrid {
    fn eq(&self, other: &Self) -> bool {
        self.dims == other.dims
            && self.origin == other.origin
            && self.cell == other.cell
            && self.velocity == other.velocity
            && self.temperature == other.temperature
            && self.diffusion == other.diffusion
            && self.velocity_decay == other.velocity_decay
            && self.temperature_decay == other.temperature_decay
            && self.buoyancy == other.buoyancy
            && self.iterations == other.iterations
    }
}

impl std::fmt::Debug for WindGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印整场数组：一次失败就是几万行数字，没人读。
        f.debug_struct("WindGrid")
            .field("dims", &self.dims)
            .field("origin", &self.origin)
            .field("cell", &self.cell)
            .field("diffusion", &self.diffusion)
            .field("velocity_decay", &self.velocity_decay)
            .field("temperature_decay", &self.temperature_decay)
            .field("buoyancy", &self.buoyancy)
            .field("iterations", &self.iterations)
            .finish()
    }
}

impl WindGrid {
    /// 建一张零场。`dims` 每轴 `2..=32`、`cell > 0`、`origin` 有限，否则报错。
    pub fn new(dims: [usize; 3], origin: Vec3, cell: f32) -> Result<Self, String> {
        for (axis, &n) in dims.iter().enumerate() {
            if !(2..=MAX_WIND_GRID_AXIS).contains(&n) {
                return Err(format!(
                    "wind grid axis {axis} must be 2..={MAX_WIND_GRID_AXIS}"
                ));
            }
        }
        if !cell.is_finite() || cell <= 0.0 || cell > 1024.0 {
            return Err("wind grid cell must be in (0, 1024]".to_owned());
        }
        if !origin.is_finite() {
            return Err("wind grid origin must be finite".to_owned());
        }
        let len = dims[0] * dims[1] * dims[2];
        Ok(Self {
            dims,
            origin,
            cell,
            velocity: vec![Vec3::ZERO; len],
            temperature: vec![0.0; len],
            diffusion: 0.0,
            velocity_decay: 0.0,
            temperature_decay: 0.0,
            buoyancy: 0.0,
            iterations: 1,
            scratch_velocity: vec![Vec3::ZERO; len],
            scratch_temperature: vec![0.0; len],
        })
    }

    /// 配置模拟系数；全部有限、范围见下，否则整批拒绝（不半改）。
    pub fn set_params(
        &mut self,
        diffusion: f32,
        velocity_decay: f32,
        temperature_decay: f32,
        buoyancy: f32,
        iterations: u32,
    ) -> Result<(), String> {
        if ![diffusion, velocity_decay, temperature_decay, buoyancy]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("wind grid params must be finite".to_owned());
        }
        // 显式三维扩散稳定上限是 1/6；留一点余量到 0.16。
        if !(0.0..=0.16).contains(&diffusion) {
            return Err("wind grid diffusion must be 0..=0.16".to_owned());
        }
        if !(0.0..=64.0).contains(&velocity_decay) {
            return Err("wind grid velocity_decay must be 0..=64".to_owned());
        }
        if !(0.0..=64.0).contains(&temperature_decay) {
            return Err("wind grid temperature_decay must be 0..=64".to_owned());
        }
        if !(-256.0..=256.0).contains(&buoyancy) {
            return Err("wind grid buoyancy must be -256..=256".to_owned());
        }
        if !(1..=8).contains(&iterations) {
            return Err("wind grid iterations must be 1..=8".to_owned());
        }
        self.diffusion = diffusion;
        self.velocity_decay = velocity_decay;
        self.temperature_decay = temperature_decay;
        self.buoyancy = buoyancy;
        self.iterations = iterations;
        Ok(())
    }

    #[must_use]
    pub fn dims(&self) -> [usize; 3] {
        self.dims
    }

    #[must_use]
    pub fn cell_count(&self) -> usize {
        self.velocity.len()
    }

    /// 格 `(0,0,0)` 中心的世界坐标（GPU parity 上传给着色器同一份原点）。
    #[must_use]
    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    /// 节点间距（世界单位）；节点 `i` 落在 `origin + i*cell`。
    #[must_use]
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// 温度→上升加速度增益（`sample_accel` 的 `+y` 分量系数）。
    #[must_use]
    pub fn buoyancy(&self) -> f32 {
        self.buoyancy
    }

    /// 速度场原始切片，按 `index(x,y,z)=x+dims[0]*(y+dims[1]*z)` 线性排列。
    /// GPU parity 按同一顺序打包成 `vec4`（xyz=速度，w=温度）。
    #[must_use]
    pub fn velocity_data(&self) -> &[Vec3] {
        &self.velocity
    }

    /// 温度场原始切片，与 [`Self::velocity_data`] 同一线性顺序。
    #[must_use]
    pub fn temperature_data(&self) -> &[f32] {
        &self.temperature
    }

    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.dims[0] * (y + self.dims[1] * z)
    }

    fn in_bounds(&self, x: usize, y: usize, z: usize) -> bool {
        x < self.dims[0] && y < self.dims[1] && z < self.dims[2]
    }

    /// 往格 `(x,y,z)` **累加**速度增量（脉冲注入，如火柱每步往上推一点）。越界/非有限拒绝。
    pub fn add_velocity(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        delta: Vec3,
    ) -> Result<(), String> {
        if !self.in_bounds(x, y, z) {
            return Err("wind grid cell index out of bounds".to_owned());
        }
        if !delta.is_finite() {
            return Err("wind grid velocity delta must be finite".to_owned());
        }
        let idx = self.index(x, y, z);
        self.velocity[idx] = self.velocity[idx] + delta;
        Ok(())
    }

    /// 设置格 `(x,y,z)` 温度（绝对值，如把火源格钉在某温度）。越界/非有限拒绝。
    pub fn set_temperature(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        value: f32,
    ) -> Result<(), String> {
        if !self.in_bounds(x, y, z) {
            return Err("wind grid cell index out of bounds".to_owned());
        }
        if !value.is_finite() || value.abs() > 1e6 {
            return Err("wind grid temperature must be finite and |t| <= 1e6".to_owned());
        }
        let idx = self.index(x, y, z);
        self.temperature[idx] = value;
        Ok(())
    }

    #[must_use]
    pub fn velocity_at(&self, x: usize, y: usize, z: usize) -> Vec3 {
        self.velocity[self.index(x, y, z)]
    }

    #[must_use]
    pub fn temperature_at(&self, x: usize, y: usize, z: usize) -> f32 {
        self.temperature[self.index(x, y, z)]
    }

    /// 推进一格时间：Jacobi 扩散（速度 + 温度）→ 温度浮力抬升 `velocity.y` → 逐格衰减。
    /// 非正的 `dt` 空操作。扩散用归一化系数 `diffusion*dt*60` 并再钳到 0.16 保证大 `dt` 不发散。
    pub fn step(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let blend = (self.diffusion * dt * 60.0).min(0.16);
        if blend > 0.0 {
            for _ in 0..self.iterations {
                self.diffuse_velocity(blend);
                self.diffuse_temperature(blend);
            }
        }
        let lift = self.buoyancy * dt;
        if lift != 0.0 {
            for (v, &t) in self.velocity.iter_mut().zip(self.temperature.iter()) {
                v.y += lift * t;
            }
        }
        let v_damp = (1.0 - self.velocity_decay * dt).max(0.0);
        let t_damp = (1.0 - self.temperature_decay * dt).max(0.0);
        if v_damp != 1.0 {
            for v in &mut self.velocity {
                *v = *v * v_damp;
            }
        }
        if t_damp != 1.0 {
            for t in &mut self.temperature {
                *t *= t_damp;
            }
        }
    }

    /// Jacobi 扩散（速度）：读 `scratch_velocity`（上一步的旧场），写回 `velocity`。
    ///
    /// 越界邻居用本格值填（零通量 Neumann）；除以固定 6 而非实际邻居数，扩散算子才对称、
    /// 总动量守恒（否则边界格分母小、会凭空产热）。邻居下标用**钳位后的平移**直接算，
    /// 不再逐偏移试越界——这是步进热路径，占 CPU 预算的大头。
    fn diffuse_velocity(&mut self, blend: f32) {
        let (nx, ny, nz) = (self.dims[0], self.dims[1], self.dims[2]);
        let plane = nx * ny;
        self.scratch_velocity.copy_from_slice(&self.velocity);
        let src = &self.scratch_velocity;
        for z in 0..nz {
            for y in 0..ny {
                let row = nx * (y + ny * z);
                for x in 0..nx {
                    let idx = row + x;
                    let xp = if x + 1 < nx { idx + 1 } else { idx };
                    let xm = if x > 0 { idx - 1 } else { idx };
                    let yp = if y + 1 < ny { idx + nx } else { idx };
                    let ym = if y > 0 { idx - nx } else { idx };
                    let zp = if z + 1 < nz { idx + plane } else { idx };
                    let zm = if z > 0 { idx - plane } else { idx };
                    let mean = (src[xp] + src[xm] + src[yp] + src[ym] + src[zp] + src[zm]) / 6.0;
                    self.velocity[idx] = src[idx] + (mean - src[idx]) * blend;
                }
            }
        }
    }

    /// Jacobi 扩散（温度）：与 [`Self::diffuse_velocity`] 同形，读 `scratch_temperature`。
    fn diffuse_temperature(&mut self, blend: f32) {
        let (nx, ny, nz) = (self.dims[0], self.dims[1], self.dims[2]);
        let plane = nx * ny;
        self.scratch_temperature.copy_from_slice(&self.temperature);
        let src = &self.scratch_temperature;
        for z in 0..nz {
            for y in 0..ny {
                let row = nx * (y + ny * z);
                for x in 0..nx {
                    let idx = row + x;
                    let xp = if x + 1 < nx { idx + 1 } else { idx };
                    let xm = if x > 0 { idx - 1 } else { idx };
                    let yp = if y + 1 < ny { idx + nx } else { idx };
                    let ym = if y > 0 { idx - nx } else { idx };
                    let zp = if z + 1 < nz { idx + plane } else { idx };
                    let zm = if z > 0 { idx - plane } else { idx };
                    let mean = (src[xp] + src[xm] + src[yp] + src[ym] + src[zp] + src[zm]) / 6.0;
                    self.temperature[idx] = src[idx] + (mean - src[idx]) * blend;
                }
            }
        }
    }

    /// 世界点 → **最近节点**的下标；点在采样域外返回 `None`。
    ///
    /// 与 [`Self::sample_coords`] 的差别有两处，注入必须按这里的口径：
    /// - 采样是 clamp-to-edge（盒外取边缘格），注入则要区分"在域内"与"在域外"——
    ///   把域外的火灌到边缘节点会凭空造出一处假热源。
    /// - 采样在两个节点之间插值，注入只落在**一个**节点上；取最近节点（而不是 floor）
    ///   让注入点与采样点的偏差不超过半格。
    ///
    /// 域的判定与采样一致：每轴局部坐标落在 `[0, dims-1]`。
    ///
    /// 正好落在两节点中间（半格）时按 `f32::round` 断**向上**（0.5 → 1），
    /// 这是确定性的，但注入点与采样点的偏差恰好等于半格。
    #[must_use]
    pub fn node_of(&self, point: Vec3) -> Option<[usize; 3]> {
        let local = [
            (point.x - self.origin.x) / self.cell,
            (point.y - self.origin.y) / self.cell,
            (point.z - self.origin.z) / self.cell,
        ];
        let mut node = [0usize; 3];
        for axis in 0..3 {
            let max_i = (self.dims[axis] - 1) as f32;
            // NaN 走这一支：`>=`/`>` 对 NaN 一律为假 ⇒ 判成域外。
            if !(local[axis] >= 0.0 && local[axis] <= max_i) {
                return None;
            }
            node[axis] = (local[axis].round() as usize).min(self.dims[axis] - 1);
        }
        Some(node)
    }

    /// 三线性采样速度场（盒外 clamp-to-edge）。
    #[must_use]
    pub fn sample_velocity(&self, point: Vec3) -> Vec3 {
        let (cell0, frac, hi) = self.sample_coords(point);
        let mut acc = Vec3::ZERO;
        for (cz, wz) in [(cell0[2], 1.0 - frac[2]), (hi[2], frac[2])] {
            for (cy, wy) in [(cell0[1], 1.0 - frac[1]), (hi[1], frac[1])] {
                for (cx, wx) in [(cell0[0], 1.0 - frac[0]), (hi[0], frac[0])] {
                    acc = acc + self.velocity[self.index(cx, cy, cz)] * (wx * wy * wz);
                }
            }
        }
        acc
    }

    /// 采样加速度：三线性速度 + 三线性温度 × `buoyancy` 沿世界 `+y`。这是喂给粒子的量。
    #[must_use]
    pub fn sample_accel(&self, point: Vec3) -> Vec3 {
        let (cell0, frac, hi) = self.sample_coords(point);
        let mut vel = Vec3::ZERO;
        let mut temp = 0.0;
        for (cz, wz) in [(cell0[2], 1.0 - frac[2]), (hi[2], frac[2])] {
            for (cy, wy) in [(cell0[1], 1.0 - frac[1]), (hi[1], frac[1])] {
                for (cx, wx) in [(cell0[0], 1.0 - frac[0]), (hi[0], frac[0])] {
                    let w = wx * wy * wz;
                    let idx = self.index(cx, cy, cz);
                    vel = vel + self.velocity[idx] * w;
                    temp += self.temperature[idx] * w;
                }
            }
        }
        vel + Vec3::new(0.0, self.buoyancy * temp, 0.0)
    }

    /// 世界点 → 低角格索引、三轴小数、高角格索引（各轴 clamp 到 `dims-1`）。
    fn sample_coords(&self, point: Vec3) -> ([usize; 3], [f32; 3], [usize; 3]) {
        let local = [
            (point.x - self.origin.x) / self.cell,
            (point.y - self.origin.y) / self.cell,
            (point.z - self.origin.z) / self.cell,
        ];
        let mut cell0 = [0usize; 3];
        let mut frac = [0.0f32; 3];
        let mut hi = [0usize; 3];
        for axis in 0..3 {
            let max_i = self.dims[axis] - 1;
            let g = local[axis].clamp(0.0, max_i as f32);
            let base = g.floor() as usize;
            cell0[axis] = base.min(max_i);
            hi[axis] = (base + 1).min(max_i);
            frac[axis] = g - base as f32;
        }
        (cell0, frac, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_bad_dims_and_cell() {
        assert!(WindGrid::new([1, 4, 4], Vec3::ZERO, 1.0).is_err());
        assert!(WindGrid::new([33, 4, 4], Vec3::ZERO, 1.0).is_err());
        assert!(WindGrid::new([4, 4, 4], Vec3::ZERO, 0.0).is_err());
        assert!(WindGrid::new([4, 4, 4], Vec3::new(f32::NAN, 0.0, 0.0), 1.0).is_err());
        let grid = WindGrid::new([4, 4, 4], Vec3::ZERO, 1.0).unwrap();
        assert_eq!(grid.cell_count(), 64);
        assert_eq!(grid.dims(), [4, 4, 4]);
    }

    #[test]
    fn params_and_injection_reject_out_of_range() {
        let mut grid = WindGrid::new([4, 4, 4], Vec3::ZERO, 1.0).unwrap();
        assert!(grid.set_params(0.2, 0.0, 0.0, 0.0, 1).is_err()); // diffusion > 0.16
        assert!(grid.set_params(0.1, 0.0, 0.0, 0.0, 0).is_err()); // iterations < 1
        assert!(grid.set_params(0.1, 0.0, 0.0, 0.0, 9).is_err()); // iterations > 8
        assert!(grid.set_params(f32::NAN, 0.0, 0.0, 0.0, 1).is_err());
        assert!(grid.add_velocity(4, 0, 0, Vec3::ZERO).is_err()); // oob
        assert!(
            grid.add_velocity(0, 0, 0, Vec3::new(f32::INFINITY, 0.0, 0.0))
                .is_err()
        );
        assert!(grid.set_temperature(0, 4, 0, 1.0).is_err()); // oob
        assert!(grid.set_temperature(0, 0, 0, f32::NAN).is_err());
        // 拒绝后场未被半改。
        assert_eq!(grid.velocity_at(0, 0, 0), Vec3::ZERO);
    }

    #[test]
    fn trilinear_hits_nodes_and_midpoints_exactly() {
        let mut grid = WindGrid::new([2, 2, 2], Vec3::ZERO, 1.0).unwrap();
        // 只沿 x 铺 0→10 的速度梯度，其余分量为零。
        for y in 0..2 {
            for z in 0..2 {
                grid.add_velocity(0, y, z, Vec3::new(0.0, 0.0, 0.0))
                    .unwrap();
                grid.add_velocity(1, y, z, Vec3::new(10.0, 0.0, 0.0))
                    .unwrap();
            }
        }
        assert_eq!(grid.sample_velocity(Vec3::new(0.0, 0.0, 0.0)).x, 0.0);
        assert_eq!(grid.sample_velocity(Vec3::new(1.0, 0.0, 0.0)).x, 10.0);
        assert!((grid.sample_velocity(Vec3::new(0.5, 0.0, 0.0)).x - 5.0).abs() < 1e-6);
        assert!((grid.sample_velocity(Vec3::new(0.25, 0.0, 0.0)).x - 2.5).abs() < 1e-6);
        // 盒外 clamp-to-edge。
        assert_eq!(grid.sample_velocity(Vec3::new(-9.0, 0.0, 0.0)).x, 0.0);
        assert_eq!(grid.sample_velocity(Vec3::new(9.0, 0.0, 0.0)).x, 10.0);
    }

    #[test]
    fn sample_accel_adds_buoyancy_from_trilinear_temperature() {
        let mut grid = WindGrid::new([2, 2, 2], Vec3::ZERO, 1.0).unwrap();
        grid.set_params(0.0, 0.0, 0.0, 3.0, 1).unwrap();
        // 温度只沿 x：节点 0 为 0、节点 1 为 4，中点插值应为 2。
        for y in 0..2 {
            for z in 0..2 {
                grid.set_temperature(1, y, z, 4.0).unwrap();
            }
        }
        let a = grid.sample_accel(Vec3::new(0.5, 0.0, 0.0));
        assert!(a.x.abs() < 1e-6 && a.z.abs() < 1e-6);
        assert!(
            (a.y - 6.0).abs() < 1e-5,
            "buoyancy 3 × temp 2 = 6, got {}",
            a.y
        );
    }

    #[test]
    fn diffusion_spreads_a_hot_center_and_conserves_total() {
        let mut grid = WindGrid::new([5, 5, 5], Vec3::ZERO, 1.0).unwrap();
        grid.set_params(0.1, 0.0, 0.0, 0.0, 1).unwrap();
        grid.set_temperature(2, 2, 2, 100.0).unwrap();
        let total_before: f32 = (0..5)
            .flat_map(|x| (0..5).flat_map(move |y| (0..5).map(move |z| (x, y, z))))
            .map(|(x, y, z)| grid.temperature_at(x, y, z))
            .sum();
        assert_eq!(grid.temperature_at(1, 2, 2), 0.0);
        for _ in 0..20 {
            grid.step(1.0 / 60.0);
        }
        // 相邻格被扩散加热，中心降温。
        assert!(grid.temperature_at(1, 2, 2) > 0.0, "热量应扩散到邻居");
        assert!(grid.temperature_at(2, 2, 2) < 100.0, "中心应降温");
        let total_after: f32 = (0..5)
            .flat_map(|x| (0..5).flat_map(move |y| (0..5).map(move |z| (x, y, z))))
            .map(|(x, y, z)| grid.temperature_at(x, y, z))
            .sum();
        // Neumann 边界（越界邻居不计）下总量守恒。
        assert!(
            (total_after - total_before).abs() < 1e-2,
            "扩散应守恒总热量：{total_before} -> {total_after}"
        );
    }

    /// 反面对照：关掉扩散（diffusion=0）时，同一热中心的邻居一步都不该被加热。
    /// 与上一个测试成对——它证明"邻居变热"确由扩散给出，不是别的通道漏热。
    #[test]
    fn diffusion_off_leaves_neighbors_cold() {
        let mut grid = WindGrid::new([5, 5, 5], Vec3::ZERO, 1.0).unwrap();
        grid.set_params(0.0, 0.0, 0.0, 0.0, 1).unwrap();
        grid.set_temperature(2, 2, 2, 100.0).unwrap();
        for _ in 0..20 {
            grid.step(1.0 / 60.0);
        }
        assert_eq!(grid.temperature_at(1, 2, 2), 0.0);
        assert_eq!(grid.temperature_at(2, 2, 2), 100.0);
    }

    #[test]
    fn temperature_buoyancy_lifts_velocity_and_decays() {
        let mut grid = WindGrid::new([3, 3, 3], Vec3::ZERO, 1.0).unwrap();
        // buoyancy=6，温度衰减 1/秒（dt=0.5 → damp 0.5），无扩散、无速度衰减。
        grid.set_params(0.0, 0.0, 1.0, 6.0, 1).unwrap();
        grid.set_temperature(1, 1, 1, 10.0).unwrap();
        grid.step(0.5);
        // 抬升先于温度衰减：velocity.y = 6 * 10 * 0.5 = 30；温度随后衰减到 5。
        assert!((grid.velocity_at(1, 1, 1).y - 30.0).abs() < 1e-4);
        assert!((grid.temperature_at(1, 1, 1) - 5.0).abs() < 1e-4);
    }

    /// P3-R235：世界点 → 最近节点的映射。注入路径全靠它，语义必须钉死：
    /// 域内取最近节点（偏差 ≤ 半格）、域外一律 `None`（不静默落到边缘节点）。
    #[test]
    fn node_of_maps_to_nearest_node_and_rejects_outside() {
        let grid = WindGrid::new([4, 4, 4], Vec3::ZERO, 2.0).unwrap();
        // 节点 0 在原点，节点 3 在 6.0。
        assert_eq!(grid.node_of(Vec3::ZERO), Some([0, 0, 0]));
        assert_eq!(grid.node_of(Vec3::new(6.0, 6.0, 6.0)), Some([3, 3, 3]));
        // 半格之内取最近节点：0.9 归 0（偏 0.45 格）、1.1 归 1（偏 0.55 格→就近是 1）。
        assert_eq!(grid.node_of(Vec3::new(0.9, 0.0, 0.0)), Some([0, 0, 0]));
        assert_eq!(grid.node_of(Vec3::new(1.1, 0.0, 0.0)), Some([1, 0, 0]));
        // 正好半格（1.0，cell=2.0）⇒ 两节点等距，断向上（`f32::round` 的规则）。
        assert_eq!(grid.node_of(Vec3::new(1.0, 0.0, 0.0)), Some([1, 0, 0]));
        // 域外 ⇒ None（含负侧与越过上界）。
        assert_eq!(grid.node_of(Vec3::new(-0.1, 0.0, 0.0)), None);
        assert_eq!(grid.node_of(Vec3::new(6.1, 0.0, 0.0)), None);
        assert_eq!(grid.node_of(Vec3::new(0.0, -3.0, 0.0)), None);
        // NaN 不得当成域内。
        assert_eq!(grid.node_of(Vec3::new(f32::NAN, 0.0, 0.0)), None);
    }

    #[test]
    fn non_positive_dt_is_a_noop() {
        let mut grid = WindGrid::new([3, 3, 3], Vec3::ZERO, 1.0).unwrap();
        grid.set_params(0.1, 1.0, 1.0, 6.0, 4).unwrap();
        grid.set_temperature(1, 1, 1, 10.0).unwrap();
        let before = grid.clone();
        grid.step(0.0);
        grid.step(-1.0);
        grid.step(f32::NAN);
        assert_eq!(grid, before);
    }

    /// P3-R235：网格步进的**每步墙钟曲线**（Myrmex 侧接线前定网格分辨率的唯一依据）。
    ///
    /// 帧预算是 GPU 侧死的（`gpu_pass_frame` p50 ≈ 10.6 ms / 11.6 ms 帧），CPU 只剩 ≈1 ms
    /// 余量；网格步进是纯 CPU，于是"这张场每帧要多少钱"必须先量出来再定分辨率。
    ///
    /// 计时用 `Instant`（同 `physics3d::cells_collider_count_cost_curve` 的做法）：
    /// 每臂固定步数取平均，并把**开/关扩散**分成两臂——扩散那两臂会多一次整场拷贝，
    /// 这是步进里最贵的部分，也是"要不要开扩散"这个开关的价钱。
    ///
    /// 上限断言放宽到 5 ms/步：它不是性能门（机器差异大），只是"哪天步进写成 O(n²) 就红"
    /// 的兜底。
    #[test]
    fn wind_grid_step_cost_curve() {
        const STEPS: usize = 60;
        // (名字, 维度, 每格世界尺寸)：世界是 ±96 ⇒ cell=6 覆盖 192；cell=2 只覆盖 64。
        const SHAPES: [(&str, [usize; 3], f32); 6] = [
            ("game_32x4x32_cell6.4", [32, 4, 32], 6.4),
            ("world_32x8x32_cell6", [32, 8, 32], 6.0),
            ("half_16x8x16_cell6", [16, 8, 16], 6.0),
            ("near_24x16x24_cell2", [24, 16, 24], 2.0),
            ("near_32x16x32_cell2", [32, 16, 32], 2.0),
            ("tall_32x24x32_cell2", [32, 24, 32], 2.0),
        ];
        println!("-- wind grid step cost (us/step) --");
        for (name, dims, cell) in SHAPES {
            let cells = dims[0] * dims[1] * dims[2];
            let mut grid = WindGrid::new(dims, Vec3::ZERO, cell).unwrap();
            grid.set_params(0.12, 0.5, 1.0, 3.0, 1).unwrap();
            // 铺一份非零场：全零会让扩散退化成"加 0"，测不出访存与算术的真实量级。
            for z in 0..dims[2] {
                for y in 0..dims[1] {
                    for x in 0..dims[0] {
                        grid.add_velocity(x, y, z, Vec3::new(1.0, 0.5, -1.0))
                            .unwrap();
                        grid.set_temperature(x, y, z, 1.0).unwrap();
                    }
                }
            }
            // 臂 A：开扩散（Jacobi 迭代 1 次 ⇒ 速度 + 温度各一次整场拷贝与遍历）。
            let start = std::time::Instant::now();
            for _ in 0..STEPS {
                grid.step(1.0 / 60.0);
            }
            let with_diffusion = start.elapsed().as_secs_f64() * 1e6 / STEPS as f64;
            // 臂 B：关扩散（只剩浮力抬升 + 逐格衰减）。
            let mut plain = grid.clone();
            plain.set_params(0.0, 0.5, 1.0, 3.0, 1).unwrap();
            let start = std::time::Instant::now();
            for _ in 0..STEPS {
                plain.step(1.0 / 60.0);
            }
            let without_diffusion = start.elapsed().as_secs_f64() * 1e6 / STEPS as f64;
            println!(
                "{name}\tcells={cells}\tdiffuse={with_diffusion:.1}\tplain={without_diffusion:.1}"
            );
            assert!(
                with_diffusion < 5000.0,
                "{name}: 步进 {with_diffusion:.1} us/步 超上限，网格实现疑似退化"
            );
        }
    }
}
