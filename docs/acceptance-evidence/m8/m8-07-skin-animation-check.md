# M8-07 骨骼动画证据

日期：2026-08-22  
GPU：NVIDIA GeForce RTX 3060 Laptop GPU  
质量门：workspace fmt / Clippy(`-D warnings`) / test / Debug+Release build / git diff --check

## glTF skin/animation 导入（gltf_asset.rs，`IMPORTER_VERSION=gltf-m8-07`）

- 新增 `GltfSkin`、`GltfChannel`（path/times/values/step）、`GltfAnimation`；primitive 增加 JOINTS_0/WEIGHTS_0；节点 bind 变换改为 TRS。
- 支持 LINEAR/STEP；cubic spline 与 morph target 明确拒绝；JOINTS/WEIGHTS 必须成对。
- 单测：
  - `imports_skinned_animated_quad`：自制四边形（`gen_m8_skin_glb` 生成）2 关节、1 个 "Bend" rotation 通道、关键帧与顶点权重断言。
  - `imports_cc0_kaykit_warrior_skin_and_animations`：KayKit 战士 1 skin、95 剪辑、蒙皮 primitive 与关键帧有序校验。

## 蒙皮网格与动画运行时

- `MeshVertex` 扩展为 64 字节（joints `[u32;4]` + weights `[f32;4]`）；非蒙皮顶点用恒等骨（`[0;4]` + `[1,0,0,0]`）。`MeshPrimitive.skinned` 标记；`MAX_BONES=128`。
- `animation3d.rs`：`Animation3D` 组件 + `Animation3dDescriptor`（`mesh=<path>;clip=<name>;looped=true|false`）。
  - 单测：`clip_advances_loops_and_clamps`、`pause_freezes_time`、`switching_clip_restarts_and_rejects_unknown`、`joint_palette_is_identity_at_bind_pose`、`rejects_non_skinned_asset`、`descriptor_round_trips`（共 6 项）。

## Lua

- `instantiate_scene` 解析 `Animation3D` 组件（project_root 先于实例化设置，使组件可加载 glb）。
- 新增 `animation3d` 模块：`get/set/list/paused/set_paused/time`。
- 单测：`lua_3d_animation_queries_and_switches_clips` —— 查询当前剪辑、列表、暂停冻结时间验证。

## GPU 蒙皮（game_runner）

- MESH3D 与 SHADOW shader 增加 `joint_palette` uniform（128×mat4）与蒙皮顶点分支（`material.surface.z` 标记）；材质 bind group 增加 palette 绑定，静态网格绑恒等 palette。
- 每帧从 `Animation3D.joint_matrices()` 取调色板随材质上传；宿主每帧 `advance` 3D 动画；场景切换重置世界，无状态残留。

## 真实窗口证据（game_runner，加载 game/scenes/main.scene）

`artifacts/logs/game.log`（09:40 运行）：

- `[scene] loaded name=main entities=13` —— 加入 SkeletonWarrior 后加载成功；
- `[render] 3D mesh cached path=assets/models/kaykit-warrior.glb` —— CC0 战士蒙皮网格按场景缓存；
- `demo3d: warrior clip -> 'Running_A'`（每 6 秒）—— Lua `animation3d.set` 切换骨骼动画剪辑生效；
- `demo3d: switched camera to ...` 仍正常；`lifecycle Running` 无 GPU/pipeline 校验错误（仅 Vulkan 加载器无害注册表告警）。

## 资产与许可

- `game/assets/models/m8-skin-quad.glb`：自制（`cargo run -p engine_core --example gen_m8_skin_glb` 可再生成）。
- `game/assets/models/kaykit-warrior.glb`：KayKit Character Pack : Skeletons 1.0（CC0 1.0），已登记 `docs/THIRD_PARTY.md`（来源 GitHub + kaylousberg.com）。

## 质量门

- `cargo fmt --all --check` 通过；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 无告警；
- `cargo test --workspace --all-features` 全部通过（exit 0，engine_core 186 项）；
- `cargo build --workspace` / `--release` 通过；`git diff --check` 通过；
- `game_runner --project game --check` / `assetc --check` 通过。
