# M5 编辑器 MVP 下一阶段实施计划

- 项目：P2 Rust + Lua 游戏引擎
- 当前基线：`v0.4-scripting`
- 当前提交：`b181343`
- 状态：READY

## 1. 当前实现判断

M0-M4 已完成：窗口、wgpu、Lua、ECS、2D 渲染、资源数据库、导入缓存、场景格式、预制体、Lua API、热重载和脚本调试均已有实现，并已创建 `v0.4-scripting` 标签。

当前空缺：

- `editor/` 为空，尚无编辑器入口；
- `tools/` 为空，尚无资源构建和场景检查工具；
- `engine/tests/` 为空，尚无跨 crate 集成测试；
- 场景虽可保存/加载，但尚无编辑器状态、dirty、命令历史、恢复和运行态隔离；
- `game/` 只有最小 Lua 入口，尚无完整编辑器验收场景。

下一阶段采用垂直切片，不直接实现完整 IDE，先闭环“打开项目 → 编辑场景 → 绑定脚本 → Play/Stop → 保存恢复”。

## 2. 目标与非目标

### 目标

1. 独立 `editor` crate 和可启动窗口；
2. 打开并检查项目目录；
3. 创建、打开、保存和恢复场景；
4. 场景树选择、创建、删除、重命名实体；
5. Inspector 编辑第一批组件；
6. 资源浏览和刷新；
7. Lua 绑定、Play/Stop 和 Console 诊断；
8. Undo/Redo、自动保存和崩溃恢复；
9. 完成可重复的 M5 综合验收场景。

### 不做

不做蓝图、多用户协同、跨平台编辑器发布、复杂材质和动画曲线、完整 DCC 导入、现有场景格式重写，也不把编辑器逻辑塞入 `game_runner`。

## 3. 技术路线

- 新增独立 `editor` crate，第一版采用 `egui` + `eframe`；
- 编辑器通过 `engine_core` 公共 API 访问场景、资源和脚本；
- 编辑器维护 `ProjectState`、`SceneDocument`、`AssetBrowserState`、`CommandHistory`、`PlaySession`、`ConsoleState`；
- Play 使用编辑前快照、独立 World 和 Lua Runtime，Stop 不得污染编辑器文档；
- 场景、资源数据库和恢复文件使用临时文件校验后原子替换；
- GUI 必须配套无窗口 CLI/单元测试，避免只依赖人工点击验收。

## 4. 任务分解与验收

### M5-01 编辑器启动与项目打开

**实施**：新增 crate 和入口；支持 `--project`、`--check`；检查 `engine/`、`game/`、`assets/`、`docs/`；合法项目显示状态，非法路径返回清晰非零错误；记录 UI 依赖决策。

**验收**：窗口可启动；`cargo run -p editor -- --project C:\GitHub\p2 --check` 成功；不存在路径失败且含路径和原因；Debug/Release 构建及 M4 回归通过。

### M5-02 项目与场景文档模型

**实施**：定义项目路径、当前场景、dirty 和最近文件状态；复用 `engine_core::Scene`；支持新建、打开、保存、另存为；临时文件校验后原子替换；保存失败保留原文件。

**验收**：保存/加载内容一致；非法场景不破坏原文件；dirty 准确；重复保存可复现；临时目录测试通过。

### M5-03 场景树与实体编辑

**实施**：显示名称、ID、父子关系；单选；创建、删除、重命名、设置父节点；选择同步 Inspector；所有编辑进入命令系统并标记 dirty。

**验收**：编辑、重载后树一致；无效实体拒绝；选择与 Inspector 同步；删除子树行为有测试。

### M5-04 Inspector 第一批组件

**实施**：支持名称、Transform、Sprite 引用、Lua 脚本字段；范围校验、错误提示、默认值；非法输入不写模型；保存/重载保持一致。

**验收**：Transform 修改可见；资源引用可校验；非法值不污染文件；至少 20 个字段编辑测试。

### M5-05 资源浏览器与刷新

**实施**：扫描 `assets/`，显示路径、类型、导入状态、AssetId；名称/扩展名筛选；手动刷新和外部变化提示；复用 `ResourceDatabase`；失败原因可见；先实现资源引用拖放。

**验收**：新增/修改资源刷新可见；筛选准确；失败状态可见；数据库重启后稳定。

### M5-06 Lua 绑定与 Console

**实施**：选择 `game/scripts` Lua 文件并写入场景字段；Play 按绑定加载；复用 Lua Runtime、热重载、ScriptDebugger；Console 显示路径、行号、阶段、调用栈，支持过滤和清空。

**验收**：合法脚本运行；语法/运行时错误可定位；错误不崩溃；脚本刷新后结果变化可见。

### M5-07 Play/Stop 运行态隔离

**实施**：Play 前快照；从快照建立独立运行态；Stop 销毁运行态并恢复编辑态；运行时禁止写回文档；固定“未保存时保存或使用内存快照”策略。

**验收**：运行时移动实体/修改变量/触发脚本后 Stop，编辑场景与 Play 前一致；连续 Play/Stop 100 次无泄漏。

### M5-08 Undo/Redo 命令系统

**实施**：可逆命令模型；属性、创建、删除、重命名、父子关系均可撤销；新操作清空 redo；连续 Transform 输入合并；关闭场景释放历史。

**验收**：每类操作撤销/重做正确；边界不 panic；100 次随机操作往返后序列化结果与初始一致。

### M5-09 自动保存与崩溃恢复

**实施**：按间隔或编辑触发恢复文件；保存场景版本、原路径、时间戳、校验；启动提示恢复/丢弃；恢复失败不覆盖主文件；成功保存清理无用恢复文件。

**验收**：模拟崩溃恢复最近版本；损坏/版本不符不覆盖主文件；提示可解释；临时文件可清理。

### M5-10 编辑器 MVP 综合验收

**流程**：创建/打开项目；创建场景；放置玩家和障碍物；改 Transform；绑定 Lua；保存；Play；Stop；修改资源并刷新；验证 Undo/Redo 与恢复；打包运行 Demo。

**验收**：十步全部通过；无 P0/P1；全量质量门通过；证据归档到 `artifacts/logs/m5/`、`artifacts/screenshots/m5/`；创建 `v0.5-editor-mvp`。

## 5. 依赖顺序

```text
M5-01 → M5-02 → M5-03 → M5-04
                         ├→ M5-05
                         ├→ M5-06 → M5-07
                         └→ M5-08 → M5-09 → M5-10
```

每个任务单独提交，完成后从 `tasks/active/` 移到 `tasks/done/`。M5-01/02 未稳定前不做复杂拖放和 Gizmo。

## 6. 统一质量门

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
git diff --check
cargo run -p editor -- --project C:\GitHub\p2 --check
```

GUI 无法自动运行时，必须用无窗口状态模型测试替代，且最终保留人工视觉验收证据。

## 7. 风险与应对

| 风险 | 等级 | 应对 |
|---|---|---|
| egui/eframe 与 wgpu/winit 冲突 | P1 | 独立 crate、锁版本、先做最小窗口、保留回滚入口 |
| 运行态污染编辑态 | P1 | 快照、独立 World/VM、Play/Stop 100 次测试 |
| 场景格式不足 | P1 | 兼容扩展、版本和迁移测试，不破坏 M4 文件 |
| 原子保存导致数据丢失 | P0 | 临时文件、校验、替换前备份、故障注入 |
| GUI 测试不稳定 | P2 | 状态模型优先，CLI/单元测试覆盖 |
| 中文乱码回归 | P2 | UTF-8 无 BOM，提交前检查和 `git diff --check` |

## 8. M5 通过条件

M5-01 至 M5-10 全部 PASS；Debug/Release、fmt、Clippy、workspace 测试通过；综合场景、恢复故障注入、Play/Stop 100 次回归通过；无 P0/P1；证据完整；创建 `v0.5-editor-mvp`。

## 9. 第一执行任务

立即执行 `M5-01 编辑器启动与项目打开`。开始条件：工作区干净、`v0.4-scripting` 存在、M4 回归可复现、UI 依赖决策已记录。