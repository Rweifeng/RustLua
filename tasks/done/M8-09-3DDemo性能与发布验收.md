# M8-09 3D Demo、性能与发布验收

## 状态

ACCEPTED

## 目标

制作可移动角色、静态场景、动态光源、基础材质、阴影、一个动画和调试 overlay；接入 assetc package、Release 构建、目录隔离和含空格路径测试；完成 M8 验收矩阵与证据；创建 `v0.8-3d-foundation` 标签。

## 验收目标

- Release 发布包可运行；
- 角色移动、深度遮挡、光照、阴影、动画和相机全部可复现；
- 至少记录 100/1000 Mesh 实例、场景加载、CPU/GPU 帧耗时和 Draw Call；
- 无 P0/P1；
- 创建 `v0.8-3d-foundation` 标签。

## 实现

### 3D Demo（demo3d.lua + main.scene）

- 骷髅战士自动绕场行走（transform3d 驱动）+ 骨骼动画剪辑循环（Idle/Running/攻击）；
- 8×8 实例网格（相同 triangle 触发实例化+剔除），数量由 `P2_INSTANCE_COUNT` 环境变量控制（默认 64）；
- 相机每 4 秒切换、三角形摆动、方向光与阴影（M8-05 已就绪）；
- 调试 overlay 显示 fps/frame/update/lua（M1-07 已有）。

### 性能基线

- 以 `P2_INSTANCE_COUNT=100` 与 `=1000` 运行真实窗口，采集：
  - 实例数与 Draw Call（`[render] 3D baseline draws/instances`）；
  - 场景加载耗时（`[scene] loaded` 时间戳）；
  - CPU/GPU 帧耗时（`[perf] fps/frame_ms/update_ms/lua_ms`）。

### 发布

- `cargo build --workspace --release`；
- `assetc package` 打包到含空格路径目录；
- `game_runner --pack <dir> --check` 校验含空格路径；运行打包游戏确认；
- 目录隔离：`.cache` 相对项目根，不污染 CWD。

### 标签与验收

- 验收矩阵增加 M8-A09 行；证据归档 `docs/acceptance-evidence/m8/m8-09-demo-perf-release.md`；
- 无 P0/P1 检查；
- `git tag v0.8-3d-foundation`。

## 质量门

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
git diff --check
cargo run -p assetc --check
cargo run -p game_runner -- --project C:\GitHub\p2\game --check
```

## 当前验证

- Demo：战士绕场行走+骨骼动画剪辑切换、相机切换、三角形摆动、8×8/可调实例网格、方向光阴影、调试 overlay；
- 性能基线（debug）：64 实例 12 绘制/48–55 可见 ~127fps；100 实例 12 绘制/70 可见 ~130fps；1000 实例 12 绘制/367 可见 ~47fps（frame ~21ms / update ~1.7ms）；Release 64 实例 ~330fps；
- `package.ps1 -SelfTest -Encrypt`：Release 包（35 文件 14.2MB）+ 含空格路径 + 目录隔离自测通过；发布包独立运行 3D 场景正常；
- 无 P0/P1；workspace 质量门通过；
- `v0.8-3d-foundation` 标签已创建。

## 下一步

无。

## 备注

- `P2_INSTANCE_COUNT` 环境变量控制实例网格数量（默认 64），供性能基线复现。
