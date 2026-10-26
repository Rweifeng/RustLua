# 江湖有田 · 门面 UI 换肤：去 Logo 与日间竹编按钮（2026-10-25）

## 0. 背景

用户验收开始界面后反馈：深木 Logo 牌匾、深木按钮与已换上的明亮日间田园像素背景不搭。
本轮要求 **去掉 Logo**、**替换按钮资源**（普通态 + 悬停态），并重新打包 PC 供验收。

## 1. 素材设计（画布 cws_59676043223468802）

- **按钮普通态** `ui_menu_btn_day_9s.png`（256×72，9-slice 18）：浅暖竹编木牌，
  深绿回纹四角 + 黄铜铆钉，浅米底 → 与日间背景同温。
- **按钮悬停态** `ui_menu_btn_day_hover_9s.png`（256×72，9-slice 18）：同造型，
  暖金描边 + 暖金回纹 + 左上暖光高光（选中态）。
- 生成源图 3861×1086（纯白底，无场景元素），NEAREST 缩放到 256×72 入库
  `assets/art/ui/menu/`；画布资产已持久化（normal `MUUIvTB5-4P0reAQatG-0201`、
  hover `i1QO89sH-4P0reAQatG-0201`，校验重渲 `HrR2UUCH-4PJh3Suk1i-0201`）。
- 设计 Brief 已升 v3（item `f8nSWrvx-4PFaczE6eQ-0201`）。

## 2. 引擎落地

| 位置 | 改动 |
|---|---|
| `assets/prefabs/ui/title_menu.prefab` | 删除 `Logo` 实体（原 entity 5，含 Image/Transform/UiLayout/entity_meta）；全部实体重编号连续（5..17）；三块按钮 Image 纹理 `ui_menu_btn_wuxia_9s.png` → `ui_menu_btn_day_9s.png`（9-slice 18 不变） |
| `scripts/ui/skin.lua` | `menu_btn_wuxia` / `menu_btn_wuxia_hover` 的 path 改指两张日间纹理（key 名不变，`title_menu.lua` 选中态调用零改动）；删除无调用者的 `title_logo` key |
| `assets/art/ui/manifest.json` | menu 模块 `files` + 明细各登记 2 张新纹理（9-slice 18） |
| `bundles.json` | 无需改动：`art:ui` 组按目录 glob 递归包含 `assets/art/ui/` |
| `docs/48` | §1 基准更新 1235 = 1065 + 145 + 25；§2 决定表 +5 行（3 下架：`ui_title_logo.png` / `ui_menu_btn_wuxia_9s.png` / `ui_menu_btn_wuxia_hover_9s.png`；2 接线：两张日间按钮） |

## 3. 门禁结果

| 门 | 结果 |
|---|---|
| m63 黄金对账（`jhyt_m63_title_prefab`） | 全绿：probe/节点 17→16、黄金表按 `--capture` 重建（64 行）、层序期望 10→9 段（实测） |
| m29 资源接线（`jhyt_m29_season_weather`） | 全绿：`dev=145 orphans=25 decisions=40`（常量 1066/22 → 1065/25） |
| m19_ui | 全绿（`jhyt_m19_ui_ok`） |
| m31 发布门 + PC 打包 | 见发布记录（打包后执行） |

## 4. 资产口径变化（docs/48 同步）

- 已引用 −3 +2 = −1 → **1065**：`ui_title_logo.png`、`ui_menu_btn_wuxia_9s.png`、
  `ui_menu_btn_wuxia_hover_9s.png` 失去引用（下架登记，文件暂留仓库）；
  `ui_menu_btn_day_9s.png`、`ui_menu_btn_day_hover_9s.png` 接线。
- 开发产物 145 不变；内容缺口 22 → **25**；`assets/` 总数 1233 → **1235**。

## 5. 验收指引

- 启动 `builds\pc\game.exe --pack pack --title game_jhyt`。
- 期望画面：日间田园背景、无 Logo 牌匾、三块浅色竹编按钮垂直排列（选中态金色描边）。
- 回归：进入游戏 → 暂停/背包/商店面板皮肤不受影响（仅 menu 段纹理换 path）。

## 6. v2 视觉迭代：按钮换暖陶棕（2026-10-26）

用户验收 v1 包后反馈"按钮和文字搭配、样式、配色太丑"：米白竹编按钮配引擎浅色文字
（`0.949,0.918,0.847`）对比度低，9-slice 小尺寸上竹纹/回纹/铆钉细节过密显脏。

**方案**：按钮改**明亮暖陶棕**（与田园暖色同温、配引擎固定浅色文字高对比），描边深棕、
四角简洁回纹、内部极浅布纹；悬停态**金琥珀亮底 + 金棕描边 + 左上暖光**（与选中高亮暖金呼应）。
引擎动态文字颜色为固定值（`ui.text_at` 无颜色参数、`TextStyle::default` 白色系），
故对比度靠按钮底色承担，不改引擎与脚本。

| 位置 | 改动 |
|---|---|
| `assets/art/ui/menu/ui_menu_btn_day_9s.png` | 覆盖为 v2 陶棕 normal（256×72，9-slice 18） |
| `assets/art/ui/menu/ui_menu_btn_day_hover_9s.png` | 覆盖为 v2 金琥珀 hover（256×72，9-slice 18） |
| `skin.lua` / `manifest.json` / 黄金表 | 引用路径不变（同文件名覆盖，零接线改动） |
| 孤儿文件 `assets/art/ui/menu/ui_menu_day_hover_9s.png` | **删除**：v1 落地时复制错名产生的无引用副本（与登记名 `ui_menu_btn_day_hover_9s.png` 同字节，运行时从未被引用） |

- v2 素材源图（画布 cws_59676043223468802，3861×1086）：normal `HupRs7aS-4PBHnJ6ETa-0201`、
  hover `GU7XQNNK-4PKjqC08gs-0201`（纯白底、无文字、NEAREST 缩 256×72 入库）。
- 门禁：m63 `jhyt_m63_title_prefab_ok`、m29 `orphans=25 decisions=40`（删孤儿后回到
  v1 钉的 1235 = 1065 + 145 + 25，常量无需改）、m19_ui 全绿。
- 验收期望：三块暖陶棕按钮 + 米白文字清晰高对比，选中态金琥珀高亮；不再有浅底浅字的"脏"感。
