# M14-02 资源变更 / 断链 / 修复回归

## 状态

IN PROGRESS。自动化基线与 GUI 实机验收已完成；如需补“删除资源再恢复原文件”附加链路，可在本文继续追加。

## 本次新增回归

### assetc

文件：`engine/crates/assetc/src/lib.rs`

- `sample_project_rename_breaks_dependencies_until_consumers_are_updated`
  - 复制真实样板工程 `game_test_2d_production/` 到临时目录；
  - 先执行一次 `import`，确认样板工程健康；
  - 将 `assets/textures/slime.png` 重命名为 `assets/textures/slime-renamed.png`；
  - 执行 `check_dependencies`，断言以下真实消费者进入断链：
    - `assets/prefabs/slime_green.prefab`
    - `assets/prefabs/slime_red.prefab`
    - `scenes/production.scene`
  - 断言断链原因是 `缺失`；
  - 再把两个 Prefab 和生产场景中的纹理引用更新到新路径，重新 `import`；
  - 断言 `check_dependencies` 清空，样板工程恢复健康。

### editor

文件：`engine/crates/editor/src/resource_browser_state.rs`

- `sample_project_resource_rename_marks_consumers_invalid_and_recovers_after_fix`
  - 复制真实样板工程到临时目录；
  - 通过 `ResourceBrowserState::refresh()` 建立资源数据库；
  - 将 `assets/textures/slime.png` 改名；
  - 断言资源浏览器中的以下条目进入 `Invalid`：
    - `assets/prefabs/slime_green.prefab`
    - `assets/prefabs/slime_red.prefab`
    - `scenes/production.scene`
  - 修改消费者引用为 `assets/textures/slime-renamed.png`；
  - 再次刷新，断言上述条目的 `Invalid` 状态撤销。

## GUI 实机验收

日期：2026-08-26

### 实机结论

- PASS：Scene View 刷新/回读相关提示改为输出到控制台，不再覆盖 Scene View 上方区域；
- PASS：资源面板选中资源后，`Delete` 键可直接删除对应资源文件；
- PASS：层级面板或 Scene View 选中节点后，`Delete` 键可删除当前实体节点；
- PASS：在 `game_test_2d_production/` 上执行资源改名后，相关 Prefab / Scene 会进入断链状态；修复引用并刷新后，断链状态撤销。

### 通过标准

- 断链在 GUI 中可见，不是静默失败；
- Scene View 提示不遮挡主视图，诊断信息可从控制台读取；
- `Delete` 键遵循当前活动选区：资源面板删资源，层级/Scene View 删节点；
- 修复引用后无需手工清库或重建工程，状态可恢复。

## 命令行删除 / 恢复链路

日期：2026-08-26

### 执行方式

在临时副本上执行，避免影响正式样板工程 `game_test_2d_production/`：

- `cargo run -q -p editor -- --check --project <temp>`
- `cargo run -q -p assetc -- import <temp>`
- `cargo run -q -p assetc -- --check-deps <temp>`
- 删除 `assets/textures/slime.png`
- 再次执行 `cargo run -q -p assetc -- --check-deps <temp>`
- 恢复 `assets/textures/slime.png`
- 再次执行 `cargo run -q -p assetc -- import <temp>`
- 最后执行 `cargo run -q -p assetc -- --check-deps <temp>`

### 命令行结果

- 基线：
  - `editor --check` 通过；
  - `assetc import` 成功导入 25 项；
  - `assetc --check-deps` 返回 `assetc_check_deps_ok`。
- 删除 `assets/textures/slime.png` 后：
  - `assetc --check-deps` 返回 `assetc_check_deps_found=2`；
  - 断链链路为：
    - `assets/prefabs/slime_green.prefab -> assets/textures/slime.png (缺失)`
    - `scenes/production.scene -> assets/prefabs/slime_green.prefab -> assets/textures/slime.png (缺失)`
- 恢复原文件后：
  - `assetc import` 成功，`slime.png` 回到可导入集合；
  - `assetc --check-deps` 恢复为 `assetc_check_deps_ok`；
  - 末次 `editor --check` 继续通过。

## 当前覆盖范围

- 已覆盖：真实样板工程上的资源改名、断链传播、修复引用、状态恢复；
- 已覆盖：真实样板工程上的资源删除、断链传播、恢复原文件、状态撤销；
- 待覆盖：
  - GUI 层资源浏览器 / Inspector / 场景视图的截图或录屏归档；
  - 与 `M14-03` 串联的 Play/Stop/发布前检查联动回归。

## 当前结论

`M14-02` 已具备真实样板工程上的自动化、GUI 和命令行三层回归基线。资源路径发生变化或资源文件被删除时，`assetc` 和编辑器资源浏览器都会对断链消费者给出一致反馈；恢复原文件或修复引用并重新导入后，断链状态可稳定撤销，编辑器内的提示与删除快捷键行为也符合预期。
