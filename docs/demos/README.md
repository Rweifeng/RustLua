# 示例工程 Demo 总览

仓库里有**两个**示例工程，职责不重叠：

| 工程 | 定位 | 入口 |
|---|---|---|
| `examples/game_api` | **API 演示工程**：每个 Lua API 都有一处可运行演示（43 个模块表 / 319 个函数 / 30 个运行时组件），每个场景是分步 Lab | `scenes/index.scene`（菜单）→ ESC 返回 |
| `examples/game_jhyt` | 玩法样板（生活模拟 / 生产向） | 见 [`examples/game_jhyt/docs/`](../../examples/game_jhyt/docs/) |

启动后先进入 `scenes/index.scene`，每张卡片对应一个聚焦演示；按 `Escape`（或手柄 B）返回入口菜单。

## 运行

在仓库根目录执行：

```powershell
.\scripts\dev\run-demo.ps1
.\scripts\dev\run-demo.ps1 -Release
```

也可以直接指定工程：

```powershell
cargo run --manifest-path engine/Cargo.toml -p game -- --project examples/game_api
```

## `game_api` 的演示地图

| 场景 | 覆盖的 API |
|---|---|
| `main` | 2D 综合：`transform` / `sprite` / `quad` / `animation` / `particles` / `world_text` / `visibility` / `physics` |
| `unified` | 一份场景表达全部渲染类别 + `camera` / `mesh` / `light` |
| `api_core` | `log` / `time` / `entity` / `event` / `tag` / `health` / `asset` / `scene` / `save` / `app` / `data` |
| `api_gameplay` | `clock` / `calendar` / `day` / `interact` / `item` / `inv` / `container` / `shop` / `map` |
| `api_input` | `input`（物理键 / 命名动作 / 手柄 / 绑定持久化 / 导航手感） |
| `api_audio` | `audio`（总线 / 播放 / 音量设置的工程级持久化） |
| `api_ui` | `ui` 全部 45 个（面板 / 网格 / tooltip / modal / 拖拽 / 焦点环 / 滚动 / 度量 / 读回） |
| `ui_test` | 场景 UI 组件与 `UiEvents` 事件（含九宫格 `Image`、`Container` 五种 kind、`ButtonStates`） |
| `api_camera` | `camera2d` / `camera3d` / 拾取链 |
| `api_3d` | `transform3d` / `animation3d`（骨骼动画）/ `physics3d` |
| `api_tilemap` | `tilemap`（运行期创建、格子元数据、A* 寻路）/ `grid` |
| `api_script` | 每实体 `Script` 组件的 `on_spawn` / `on_update` 与实体稳定 ID |
| `api_prefab` | 预制体实例在运行期就是普通实体 |
| `demo3d` / `mixed` | **渲染 golden 固定夹具**（不是演示，内容不能改） |

演示实现集中在 `examples/game_api/scripts/demo_*.lua`，清单真源是
`examples/game_api/scripts/demos.lua`，`router.lua` 负责场景到模块的分发。
写法与硬规则见 [`examples/game_api/README.md`](../../examples/game_api/README.md)，
2.0 的框架、步骤模型与自检契约见 [docs/74](../74-game_api演示工程2.0.md)，
门的清单与历史边界见 [docs/59](../59-示例工程game_api与API覆盖率门.md)。
