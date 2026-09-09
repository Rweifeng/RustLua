# M8-01 Math3D / Transform3D / Camera3D

## 状态

ACCEPTED

## 目标

为 M8 建立纯 CPU 的 3D 数学、实体变换和透视相机基础，固定右手系、+Y 向上、相机朝 `-Z` 的实现边界，为后续 Mesh、深度和 glTF 工作提供可测试 API。

## 背景

M8-00 已确认 `v0.7` 基线并记录 ADR-0006/0007。此前 `engine_core` 只有 2D 数学和 Transform，3D 渲染尚未开始。

## 不做什么

- 不接入 wgpu、Mesh、材质、glTF 或场景格式；
- 不修改现有 2D Transform、Camera2D 和渲染路径；
- 不实现实体 ECS 的 Transform3D 组件注册或 Lua API。

## 技术方案

- `math3d`：提供 `Vec3`、`Quat`、列主序 `Mat4`、矩阵乘法、点变换和带主元选取的矩阵求逆；
- `transform3d`：提供位置、四元数旋转、非零缩放和模型矩阵 `T * R * S`；
- `camera3d`：提供透视投影、LookAt 视图矩阵、世界/NDC 往返和点级视锥分类；
- 所有输入参数校验有限性、零长度向量、投影范围和齐次坐标，错误显式返回，不 panic；
- 公开矩阵组合实现 `std::ops::Mul`，避免自定义 `mul` API 与标准 trait 混淆。

## 修改范围

- `engine/crates/engine_core/src/math3d.rs`
- `engine/crates/engine_core/src/transform3d.rs`
- `engine/crates/engine_core/src/camera3d.rs`
- `engine/crates/engine_core/src/lib.rs`

## 验收标准

- 单位矩阵、右手叉积、四元数绕 Y 旋转、矩阵组合和逆矩阵均有单测；
- Transform3D 正确生成 `T * R * S`，零缩放被拒绝；
- Camera3D 拒绝非法 FOV、宽高比、near/far、LookAt 和非有限输入；
- 前方点能通过透视投影和 NDC 往返，前后点可正确进行视锥分类；
- workspace fmt、Clippy、测试、Debug/Release 构建全部通过。

## 测试命令

```powershell
Push-Location engine
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
Pop-Location
git diff --check
```

## 验收证据

- `artifacts/logs/m8-01-test.log`
- `artifacts/logs/m8-01-release-build.log`
- `cargo test -p engine_core camera3d`：3 passed
- `cargo test -p engine_core transform3d`：3 passed
- `cargo test -p engine_core math3d`：4 passed

## 风险

- 投影矩阵的深度符号容易与坐标系不一致；已通过前方点视锥测试和 NDC 往返测试锁定为相机朝 `-Z`、深度范围 `[0,1]`；
- 矩阵内部布局若被渲染层直接依赖会扩大耦合；后续 GPU 上传前必须补充明确的布局转换和字节布局测试。

## 已知限制

当前只提供点级视锥分类，没有包围盒/包围球分类；这属于 M8-08。Camera3D 尚未接入 wgpu 或窗口。

## AI 输出

- 已完成 Math3D、Transform3D、Camera3D 公共模块和 10 个新增单元测试；
- 修复了初版透视投影的深度符号错误，并处理 Clippy 与 Rust 借用检查；
- 未修改现有 2D API 或渲染器。
