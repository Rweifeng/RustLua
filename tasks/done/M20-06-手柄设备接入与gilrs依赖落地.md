# 任务：手柄设备接入与 gilrs 依赖落地

- 任务 ID：M20-06
- 状态：已完成（设备层与 headless 验收通过；依赖方案按离线环境改为零新增依赖的 XInput，实机插拔未验证）
- 关联里程碑：M20
- 创建日期：2026-09-10
- 前置依赖：M20-02 的抽象输入层（`core/input.rs` 的 `InputSnapshot` 手柄喂入口、`ActionMap`）

## 目标

让引擎在 Windows 桌面真正拿到手柄设备状态：把「设备轮询」封装成平台无关、可 headless 测试的一层，向上只暴露既定抽象接口（按钮索引 + 轴值），不改动 `core/input.rs` 的公共形状。

## 影响文件

- `engine/crates/engine/Cargo.toml`（`[target.'cfg(windows)'.dependencies] windows-sys`，新增 `Win32_UI_Input_XboxController` 特性；**未引入 gilrs**，原因见实现记录）
- `engine/crates/engine/src/host/gamepad.rs`（新增：后端抽象、XInput 后端、Fake 后端、轮询器 + 8 项单测）
- `engine/crates/engine/src/host/mod.rs`（导出）
- `engine/crates/engine/src/host/runtime_scene.rs`（`run_script_steps` 合并设备帧；`set_gamepad_backend`/`gamepad_connected`/`gamepad_backend_name`）
- `docs/33-手柄设备接入与输入契约.md`（新增）

## 实现记录

- **抽象与契约**（`host/gamepad.rs`）：`GamepadBackend { poll() -> GamepadFrame; name() }`；
  `GamepadFrame { buttons: [bool;16], axes: [f32;6], connected }`；按钮 0..15 / 轴 0..5 的索引契约
  以 `button::*` / `axis::*` 常量固化并与 `ActionMap::with_defaults`（`GamepadButton(0)`=确认、
  `(1)`=取消、轴 1 双向 `ui_vertical`）一致；Y 轴**上为正**与 WASD `vertical` 同义。
- **Windows 后端（零新增依赖）**：`XInputBackend` 经 `windows-sys`（已在依赖树中，仅新增
  `Win32_UI_Input_XboxController` 特性）调用 `XInputGetState` 采样 0..3 号用户，取第一个已连接设备；
  `wButtons` → 契约按钮（纯函数 `buttons_from_flags`），摇杆 `-32768..32767`→`-1..1`、
  扳机 `0..255`→`0..1`，再各自过死区（摇杆 0.15、扳机 0.05，死区外按比例重映射）；
  扳机模拟量 ≥0.5 同步置位数字按钮 6/7。无设备/无驱动静默降级。
- **Fake 后端 + 轮询器**：`FakeBackend` 按序返回脚本化帧；`GamepadPoller::poll_into(&mut InputSnapshot)`
  只在按钮**变化**时喂 `gamepad_button_event`（按下/松开边沿由 `InputSnapshot` 保留到逻辑帧消费，
  快速点按不丢），轴每帧写模拟量，**断连时释放按钮并清零轴**（不粘键），连接/断开打日志。
- **宿主接入**：`RuntimeScene::run_script_steps` 在克隆宿主输入后调用 `gamepad.poll_into(...)`，
  正式 game / 编辑器 Play / headless 共用同一路径；`set_gamepad_backend` 供 headless 验收注入
  Fake 后端，`gamepad_connected()`/`gamepad_backend_name()` 供诊断。
- **依赖方案偏离（重要）**：原计划落地 `gilrs`。实测本机离线环境**无法解析**该依赖——
  `cargo metadata --offline` 报 `no matching package named gilrs found`，尽管
  `gilrs-0.11.2.crate`/`gilrs-core-0.6.8.crate` 已缓存、`~/.cargo/config.toml` 已配置 USTC 镜像替换；
  用一次性探针工程验证 sparse 与 git 两种镜像配置均失败（索引缓存不完整）。为不阻塞任务目标，
  改为零新增依赖的 XInput 实现，并**保留后端抽象**：联网后新增 `GilrsBackend` 即可无缝替换
  （上层 `GamepadPoller`/`RuntimeScene`/`ActionMap`/Lua 无需改动）。详见 `docs/33` 第 4 节。

## 验收方式

- `cargo test -p engine gamepad`：FakeBackend 驱动的单元测试全绿，覆盖快速点按不丢、死区边界、
  轴方向与 `ActionMap` 轴绑定方向一致、断连/失焦清空、无设备不报错。
- `cargo fmt --all -- --check`、`cargo clippy --workspace …`、`cargo build -p game`、`cargo build -p editor` 全绿。
- 手工步骤：接入 Xbox 兼容手柄，运行 `cargo run -p game -- <project>`，日志打印连接状态与按钮变化
  （debug 级别），插拔手柄各一次不 panic、不粘键。
- `scripts/verify.ps1` 通过（若本轮涉及全量）。

## 验收证据

- [x] 相关测试通过：`cargo test -p engine --lib host::gamepad --offline` → **8 passed / 0 failed**
  （死区边界与重映射、按钮位→契约索引、轴方向与扳机按钮、快速按下/松开边沿保留 + 动作 press/release、
  轴驱动动作绑定与合成轴、断连与失焦清空、无设备不产生输入不 panic、平台默认后端名称 `xinput`）
- [x] Debug 构建通过：`cargo check -p engine --offline` → exit 0（含真实 XInput 链接路径：
  `platform_default_backend_is_available` 构造 `XInputBackend` 并实际调用 `poll()`）；
  `cargo build -p game -p editor --offline` → exit 0（windows-sys 特性新增不影响下游 crate）；
  `cargo run -p game --example jhyt_smoke --offline` → `jhyt_smoke_ok`（手柄接线零回归）
- [x] `cargo clippy -p engine --all-targets --all-features --offline -- -D warnings` → **exit 0**（本任务改动零告警）
- [ ] `cargo fmt --all -- --check`：本轮验证时被并行会话在途文件阻塞（`jhyt_m19_slice.rs` 写入中曾非 UTF-8），
      与 M20-06 改动无关；收尾统一门禁复跑（本任务新增文件已单独 `cargo fmt -p engine`）
- [ ] 手工步骤（实机手柄插拔）：**本环境无手柄设备，未执行**；已提供精确步骤与预期日志（见 `docs/33` 第 5 节）
- [x] 文档与 CHANGELOG 已更新（`docs/33-手柄设备接入与输入契约.md`；CHANGELOG v0.15 段由本轮收尾统一补）

## 未完成项 / 已知限制

- **gilrs 依赖未落地**（离线无法解析；联网后 `cargo add gilrs@0.11` + 新增 `GilrsBackend` 即可，
  抽象层已就绪）。因此跨平台（Linux/macOS）设备轮询暂缺，非 Windows 平台走 `UnsupportedBackend`（无设备）。
- **实机手柄插拔未验证**（本环境无设备）；`FakeBackend` 已覆盖同等逻辑（边沿/死区/断连清理）。
- 不支持浏览器 Gamepad API（WebGL/WASM 目标手柄不可用）。
- 不支持震动（`XInputSetState` 可用但未接入）。
- 手柄按键编号依赖 XInput 固定映射，未提供用户自定义编号表；死区当前为常量（可配置死区属 M20-08）。
