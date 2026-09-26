# 任务：WP9 编辑器、Lua、迁移与发布收口

对应 [docs/40](../../docs/40-引擎全面重构计划.md) §3 WP9。状态：**已完成并收口（2026-09-26）——
Lua 统一变换 API、统一 Transform 面板、统一 Camera 面板（按 `Projection` + 生效姿态）、
screen→world 换算、迁移命令（数据层 + CLI）、组件改名弃用告警全部落地；迁移「写回」经核实
无需施加（见「收口说明」）。**

## 已做

### Inspector 统一 Transform 面板

docs/40 §3 WP9 实施要求"一个 Transform 面板按 2D/3D 编辑模式展示"，而编辑器此前是两个各自独立的
折叠块（`Transform` 与 `Transform3D`）。现在合并为一个「变换（Transform）」面板：

- 只有一种变换 → 按拥有者显示，段内标明 `2D` / `3D`（此时模式开关没有意义）；
- 两种都有 → 顶部出现「编辑：2D / 3D」模式开关，并**始终**标出"空间：2D / 3D / 2D + 3D"，
  以免"被切走的那一种"被误以为不存在；
- 两种都没有 → 面板不显示。

解析逻辑抽成纯函数 `TransformSpace::resolve(has_2d, has_3d, mode)`，两条性质用单测钉住：只有一种
变换时**不听模式**（否则切到 3D 会让 2D-only 实体的面板空掉）、两者都没有时不显示。旧的
`inspector_transform3d_open` 折叠标志删除。docs/25 已同步。

**Camera 面板按 Projection 展示（WP5 相机收敛后已完成）**：Camera3D 面板除可编辑字段外，
多了一段**只读摘要**——① 投影用统一的 `Projection::from_camera3d` 表述（"透视（垂直 FOV 60.0°，
近 0.100，远 100.000；深度 0..1，前向 -Z）"）；② `pose=transform` 时给出**生效**姿态（位置 +
视线 -Z），或"缺 `Transform3D` / 组件串非法 ⇒ 渲染回退到组件姿态"的**警告**。两条都从组件串
原文经引擎解析器得到，与运行时同一条换算（`Camera3D::with_pose`），因此面板显示的是渲染器
真正会用的值，而不是"其实不生效"的组件值。2D 相机不是场景组件（WP1 的 26 源场景普查已确认
没有 `Camera2D` 组件，它由 Lua 请求/默认回退提供），所以面板只展示 `Camera3D`——这一点没有
变化，只是现在它说的是统一 `Projection` 的话。测试：
`camera_panel_summary_shows_projection_and_effective_pose`（无 pose / 缺 Transform3D 警告 /
绕 Y 90° 后生效姿态为 -X / 非法 Transform3D 串仍警告）。

### Lua 统一 2D/3D 变换 API

问题：Lua 侧 `transform`（2D `Transform`）与 `transform3d`（`Transform3D`）是两套互不通用的
接口，3D 实体调用 `transform.*` 只会拿到 `entity has no transform`——脚本必须自己知道实体属于
哪个空间。

现在 `transform` 表按实体**实际拥有**的组件分派（`space` 字段是新增的，2D 取值一字未变）：

| API | 2D | 3D |
|---|---|---|
| `get(id)` | `{space='2d', x, y, rotation, scale_x, scale_y}` | `{space='3d', x, y, z, rx, ry, rz, rw, scale_x, scale_y, scale_z}` |
| `set(id,x,y[,rotation,sx,sy])` | 全量（含标量 rotation） | 只改 XY，**Z 保留**；带 rotation 报错 |
| `move(id,dx,dy)` | XY 增量 | XY 增量 |
| `space(id)` | `'2d'` | `'3d'`，都没有则 `nil` |
| `attach(id,'2d'\|'3d')` | 缺组件才新建，返回是否新建 | 同左，并把另一侧的 XY 带过来 |

**刻意的三处决定**：

1. **3D 下拒绝标量 `rotation`**：2D 的 rotation 是绕 Z 的弧度，3D 的 rotation 是四元数。把标量
   静默当成绕 Z 旋转属于语义近似——与本项目"不支持就报诊断、不静默改变结果"的一贯做法冲突。
   报错信息直接指向 `transform3d.set`。
2. **拒绝时不留半写状态**：`set` 在 3D 分支先校验再写，因此 `pcall` 失败的调用不会把位置改掉
   一半（有测试锁住）。
3. **`attach` 不覆盖、且桥接 XY**：已有该组件时返回 `false` 而非重新插入（`transform3d.attach`
   的覆盖行为作为 facade 保持不动）；新建 3D 组件时把 2D 的 XY 写进去，否则实体会瞬移到原点。

`transform3d` / `camera` 表保持原样，作为空间专属 facade（docs/40 WP9 明确要求保留）。

测试：`lua_transform_api_dispatches_between_2d_and_3d`、
`lua_transform_api_refuses_ambiguous_rotation_and_unknown_space`。docs/18 已同步。

## 收口说明（2026-09-26）

- **迁移命令「写回」未施加是正确决定，不是遗漏**：数据层与 CLI 已完成（见下），仓库 40 个场景
  仅 `game_api/scenes/index.scene` 一处父子关系（UI 子实体，父子变换都是单位阵）——迁移与否世界
  位置都不变，写回只会留下无意义的 `.spatial-local` 标记。CLI 三命令（`--migrate-spatial` /
  `--migrate-spatial-write` / `--restore-spatial`）随时可用；将来任何场景出现真实局部空间父子关系，
  直接按 dry-run → write → restore 流程施加即可。
- **组件改名弃用告警**：✅ 已做（上一轮）——`editor/src/schema_audit.rs` 增 `COMPONENT_RENAMES`
  改名表（Transform3D→Transform / Camera2D→Camera / Camera3D→Camera）、汇总行
  `renamed_components=N`、明细行 `component_renamed legacy=.. current=.. count=.. hint=migrate_project`；
  未注册组件先查改名表再进 `unknown`；`editor --check` 对 `game_jhyt` 与 `game_api` 仍
  `schema_errors=0`。证据：`schema_audit` 6 单测全绿、clippy `-D warnings` 干净。

**WP9 无剩余活动项。**

### WP2 空间层级迁移命令（已完成）

`core/scene_migration.rs` + 编辑器 CLI：

| 命令 | 行为 |
| --- | --- |
| `editor --project <p> --migrate-spatial` | dry-run：逐场景报告 `entities/converted/refused/skipped`，不写盘 |
| `editor --project <p> --migrate-spatial-write` | 写回：先备份 `<file>.bak`，再写局部坐标与迁移标记 `<file>.spatial-local` |
| `editor --project <p> --restore-spatial` | 一键回滚：拷回 `.bak`、删备份与标记 |

要点：**迁移不可重复施加**（数据迁移后已是局部坐标，再乘一次父变换会把子树挪走），因此标记
文件存在即跳过，命令可反复运行；无法无损建立继承的实体会被**拒绝并清掉 `parent`**（世界位置
不变）且计入 `refused`，便于脚本当质量门。备份只在缺失时写入，因此永远保留"最原始"那一份。
