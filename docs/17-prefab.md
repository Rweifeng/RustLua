# M4-04 预制体

预制体是可重复实例化的场景实体子树。它保存实体结构、组件值、资产 ID（由工程相对路径派生，
不引入 GUID / `.meta`）和元数据；实例化时会为每个实例分配新的实体 ID，避免多个实例互相覆盖。

## 文件格式（`.prefab`，version 1）

逐行记录，未知记录一律拒绝；加载与保存都走 `Prefab::validate()`：

```text
prefab_version=1
asset_id=11111111-1111-4111-8111-111111111111
name=SlimeGreen
root=1
meta	<键>	<值>
entity	<id>	<名字>	[父 id]
component	<id>	<组件名>	<组件串>
resource	<id>	<键>	<资产 UUID>
entity_meta	<id>	<键>	<值>
```

根实体行不写空父段（3 段）；`entity_meta` 同时承载编辑器元数据与下面几类谱系键。

## 实例覆盖

实例覆盖使用 `(源实体 ID, 字段)` 键保存：

- `name`：覆盖实体名称；
- `component:<名称>`：覆盖整条组件串；
- `property:<组件>.<字段>`：逐字段覆盖（组件有 schema 时优先这条，其余字段不被冻结）；
- `metadata:<名称>`：覆盖实体元数据（`prefab.*` 保留键除外）。

源预制体更新策略采用显式重建：调用 `PrefabInstance::rebuild` 创建新实例并重新应用原有覆盖值。
当前不做隐式热更新，避免编辑器修改被静默覆盖。

## 嵌套：外层文件里的内层快照

外层文件的内层节点带着 `prefab.source`（内层文件路径）、`prefab.source_id`（内层实体编号）与
内层实例根的 `prefab.root`，再加一份取值快照；场景实例另由 `prefab.outer_id` 记录"我在外层
文件里是几号实体"。文件根永远属于这张预制体自己（`Prefab::from_scene` 清根上四个标记）。

## 变体谱系（仅编辑器消费）

- `prefab.variant_base` / `prefab.variant_base_stamp`：基础路径与上次同步时的文件签名；
- `prefab.override.<实体 id>.<字段>`：变体相对基础的显式差异（字段语法同上）。

变体文件本身仍是完整快照，运行时不需要基础。保存时记录由「基础 vs 快照」重新 diff；基础在
同步后动过则拒绝保存，必须经「资源 ▸ 从基础同步变体」。v1 只做字段级，结构差异明确拒绝。

## 内层 → 外层同步记录

- `prefab.nested_override.<外层实体 id>.<字段>`：外层相对内层的显式字段差异；
- `prefab.nested_stamp.<内层路径>`：这份快照来自哪个内层版本。

记录只在写回外层时由 diff 产生；内层在上次同步后动过则拒绝写回（否则内层新值会被钉成外层
覆盖）。「资源 ▸ 从内层预制体同步」= 新内层 + 旧记录，保留外层实体编号与父子关系；一次只
同步直接挂在这一层的实例，v1 只支持字段级。语义详见 [docs/25](25-editor-guide.md) §23.6/§23.7。

```powershell
cargo run -p game_runner --example prefab_demo --release
```
