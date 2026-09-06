# game_showcase Demo 总览

`game_showcase` 是默认能力展示工程。启动后先进入 `scenes/index.scene`，每个菜单按钮对应一个聚焦 Demo；按 `Escape` 返回入口。

## 运行

在仓库根目录执行：

```powershell
.\scripts\run-demo.ps1
.\scripts\run-demo.ps1 -Release
```

也可以直接指定工程：

```powershell
cargo run --manifest-path engine/Cargo.toml -p game -- --project game_showcase
```

## Demo 地图

| 场景 | 能力 |
|---|---|
| `main` / `arena` | 经典 2D 玩法、Tilemap、物理、动画、存档、HUD |
| `production` / `menu` | 2D 生产工作流与完整玩法闭环 |
| `demo3d` | 3D Mesh、相机、骨骼动画与实例化 |
| `game3d` | 3D 玩法垂直切片、轨道相机、拾取与胜负 |
| `ui_test` | UI 事件与基础控件 |
| `ui_layout` | Canvas、Anchor/Offset、Container 与按钮状态 |
| `ui_dynamic` | 动态 Panel/Grid/Tooltip/Modal/拖拽 |
| `tilemap` | 运行时 Tilemap 创建、编辑、碰撞与元数据 |
| `effects` | 粒子、音效与背景音乐 |
| `gameplay_api` | Clock/Grid/Tag/Interact/Item/Inv/Shop/Map |
| `data_inventory` | JSON 数据、Schema、背包与容器 |
| `persistence` | Save Blob/Slot、地图状态与事件总线 |
| `resources` | Asset Load/Release 与运行时 Sprite |
| `stress` | 大量 Sprite 的 ECS/变换/剔除压力 |
| `physics3d` | 3D 刚体、碰撞事件与重置 |

单项 Demo 的实现集中在 `game_showcase/scripts/demo_*.lua`。完整玩法示例由 `production_*` 与经典玩法模块承担，`route.lua` 负责场景到模块的分发。
