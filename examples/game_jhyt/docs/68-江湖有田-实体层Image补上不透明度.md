# 68-江湖有田-实体层 `Image` 补上不透明度（M64a）

上一批（[67-江湖有田-门面搬进 prefab 与换场景死句柄](67-江湖有田-门面搬进prefab与换场景死句柄.md)，
M63b/c/d）把 `title_menu` 搬进了 `.prefab`。搬之前量的账是：**面板五屏（pause / shop / inventory /
quest / shipping）现在搬不动**，卡点不在行数（行数有 `set_visible` 兜），而在实体层 `Image`
**画不出半透明**——而每一屏的底和分隔线都是半透明的。

于是路线定了两步（用户在"先搬能搬的"与"先补引擎能力再搬数据"里点了后者），第一步就是本批：
给实体层 `Image` 补 alpha。数据形状用户在两条里选了 **RGBA 单字段**（`color=r,g,b[,a]`），
不新增 `;alpha=` 键（理由见 §2）。

里程碑号取 M64a：M63 之后阶段内容门没有再占号。

---

## 1. 卡住的是哪几行

四块屏共用同一条分隔线（2 px 高、`alpha = 0.22`），它的**全部存在感**都来自那个 0.22：

| 屏 | 调用点 | 写出来的颜色 |
| -- | ------ | ------------ |
| pause | `scripts/ui/pause_menu.lua:599` | `ui.panel(:box, x,y,w,h, 0.08,0.09,0.12,`**`0.96`**`)` 面板底色 |
| pause | `scripts/ui/pause_menu.lua:602`-612 | `:rule` = 共享常量 `RULE_COLOR = { 0.72, 0.75, 0.82, `**`0.22`**` }`（同文件 591 行） |
| shop | `scripts/systems/shop.lua:697` / `:700` | `:box` **0.94** / `:rule` `0.72,0.75,0.82,`**`0.22`** |
| inventory | `scripts/ui/inventory_ui.lua:398` | 同一条 `:rule` |
| quest | `scripts/ui/quest_ui.lua:244` | 同一条 `:rule` |
| shipping | `scripts/systems/shipping.lua:453` | 同一条 `:rule` |

`ui.panel` 的签名是 `(id, x, y, w, h, r, g, b, a)`（`script/api/ui_panel.rs:72`）—— 第九个参数
就是 alpha。而 65 号 §5 划的那条
硬边界写的正是：实体层 `UiImage.color` 是 `[f32; 3]`，**没有 alpha 通道**。所以这些行搬进
`.prefab` 之后不会是"稍微淡一点"，而是那条 2 px 的分隔线变成一条**实心横杠**、面板底变成不透明。

顺带说清一件看着像现成答案的东西：**`Quad` 不顶用**。`Quad` 确实有 RGBA（schema 侧 `Vec4`，
编码端固定写四段，见 `rustlua-render` 的 `parse_color`），但它是**世界通道**的可绘制体
（`runtime_scene/frame_data.rs:638` 起：提取时降级成 `Sprite`，走相机、`sorting_layer`、`order`），
不吃 `UiLayout`、不进 `ui_rects_by_name` 的命中表。面板要的是屏幕空间版面 + 点得中，那是
`push_scene_ui` 那一层，两者不是同一条通道。

## 2. 数据形状：`color=r,g,b[,a]`，第四段可省

| 形状 | 读 | 写 |
| ---- | -- | -- |
| `color=0.949,0.918,0.847` | `a = 1.0` | alpha=1 时**只写三段** |
| `color=0.000,0.000,0.000,0.550` | `a = 0.55` | alpha≠1 时才出现第四段 |
| `color=1,1` / `color=1,1,1,1,1` | 拒（`InvalidValue`） | — |

三段是**存量数据的形状**：仓库里带 `color=` 的 `Image` 行有 120 条，**全部三段**，一条四段的都没有。
口径（可复现）：

```
find examples -name '*.prefab' -o -name '*.scene' \
  | xargs grep -hE '^[a-z]+[[:space:]]+[0-9]+[[:space:]]+Image[[:space:]]'   # → 120 行
```

写出端跟着省，是为了不让"编辑器一保存"就把这 120 行整片重排成四段（§6 记这条买不到什么）。

**为什么不是 `;alpha=<f32>` 独立键**（用户改选了 RGBA 单字段，被否的那条留个记录）：
它能让 120 行字节不动，但代价是把一个颜色拆成两个键 —— 取色器要跨两个字段读写（`schema_inspector`
现在按"一个字段一个 widget"出行），`property_path` 的逐分量比较要特判，作者串里
`color=1,1,1;alpha=0.5` 与 `color=1,1,1,0.5` 谁是规范形也要另立一条规则。单字段这边这些都不用付。

**为什么必须新增 `FieldKind::Color`**：`Vec3` / `Vec4` 的段数校验是"**恰好** N 段逗号分隔数"
（报错串就是 `expected {N} comma-separated numbers, found {M}`），所以标 `Vec3` 进不来半透明行、
标 `Vec4` 会把 120 行判成非法（§5 的 RC-2 两条都实测过）。也不能顺手放宽 `Vec4`：
`anchors` / `rotation` / `bounds` 的段数是要害信息，而 `Quad.color` 的编码端本来就固定写四段，
它属于"恰好 4"那一族。值类型这边不需要新形状（`FieldValue::Tuple(Vec<f32>)` 本来就不判段数）。

## 3. alpha 落在哪儿：顶点，不是 draw

`SceneDraw` 没有 opacity 字段（只有 `texture/start/count/sdf/bitmap/vector/render_layer`），
`OverlayVertex.color` 是 `[f32; 4]` —— 所以实体层的 alpha 只有逐顶点这一条路：

```
.prefab 的 Image 载荷  color=r,g,b[,a]
  → UiImage::from_component_str → UiImage.color: [f32; 4]        (rustlua-ui/src/ui.rs)
  → push_scene_ui 的实体转换：color=[r,g,b] + alpha=color[3]      (mesh_ui.rs:479-488)
  → push_image_quad：let color = [r, g, b, image.alpha]           (mesh_ui.rs:651-656)
      ├─ 九宫格路径：9 块切片共用同一个 color（含 alpha）
      └─ 单四边形路径：6 个顶点同一个 color
  → OverlayVertex.color → 顶点缓冲
```

两处形状要写下来，因为它们决定了门怎么断言：

1. **同纹理的两块会并成一个批**。门里那两块 `__white__` 的矩形在 `mesh.draws` 里是**一条**
   `count = 12`，不是两条 `count = 6`（`texture_id` 相同 ⇒ 提取端合并）。所以门按
   `mesh.vertices` 的逐顶点颜色读数、按 rgb 分组，**不**按 draw 找四边形——写第一版时就是
   `assert_eq!(quads.len(), 2)` 拿到空表才发现的。
2. **`ui.grid` 的格子贴图这一层没动**。`UiGridCell.color` 本来就是 `[f32; 4]`，但格子贴图那块
   四边形**一直**是不透明发射（`cell.color[3]` 只作用于同一格先推的那块底色四边形）。
   本批只补实体层 `Image`，`mesh_ui.rs:311-324` 那里显式写 `alpha: 1.0` 把旧观感原样钉住，
   免得顺手改了背包格子。

## 4. 连带四处对齐（都是"字段形状变了"直接要求的，不是清理）

| 位置 | 改动 | 不改会怎样 |
| ---- | ---- | ---------- |
| `rustlua-ecs/component_schema.rs` | 新增 `FieldKind::Color`，`IMAGE_FIELDS` 的 `color` 用它 | 编辑器/schema 判 120 行（或半透明行）非法 |
| `rustlua-ecs/property_path.rs:430`-437 | `Color` 进"逐分量比较"那一族 | `Image.color` 退化成逐字比较：`0.2` 与 `0.20` 记成一次真编辑（假 Undo） |
| `editor/panels/schema_inspector.rs` | `field_widget` 加 `Color => Color { parts: 4 }` | 编译不过（这个 `match` 故意不留 wildcard，新增 `FieldKind` 变体必须在这里登记——取色器写回时按 `parts` 决定省不省第四段，正好对上 §2 的写出规则） |
| `game/examples/jhyt_m63_title_prefab.rs` | 捕获/比对 helper 拆成 `rgb()`（给 `Text`）+ `tint()`（给 `Image`，同 §2 口径） | 门面那张 68 行黄金表要重导（`tint()` 跟着省第四段，所以**一字节没动**） |

`script/api/ui_panel.rs` 里 `ui.image` 的构造补了 `alpha: 1.0` 一行 —— **Lua 签名不动**
（第 10 个参数仍是 `slice`），动态层 `ui.image` 本来就没有颜色参数。

## 5. 门：四条新断言面，四条反面对照全跑过

| # | 门 | 钉住什么 |
| - | -- | -------- |
| 1 | `engine` `scene_ui_image_alpha_reaches_the_vertex`（`tests/ui_layout.rs`） | 第四段真落到顶点 alpha，且 rgb 不被动；同趟钉"三段仍读成不透明" |
| 2 | `rustlua-ui` `image_color_accepts_three_or_four_segments` | 3⇒`a=1`、4⇒`a` 照收；2 段与 5 段拒 |
| 3 | `rustlua-ui` `image_color_omits_alpha_only_when_opaque` | 写出端只在 alpha≠1 时出现第四段 |
| 4 | `rustlua-ecs` `field_rules_match_engine_validators` 的 4 条 `Image` 样本 | schema 与引擎解析器同口径 |
| 4b | 同 crate `typed_equality_ignores_formatting_only_differences` 的 3 条 `Image.color` 断言 | 逐分量比较真的走到（不是退回逐字） |

门 1 为什么必须有：发射端把 alpha 硬写成 `1.0` 这种回归，矩形全对、画面全实。实测口径见下表 RC-1。

| RC | 破坏 | 红在哪 | 实得红话（逐字） |
| -- | ---- | ------ | ---------------- |
| RC-1 | `push_image_quad` 的 `color[3]` 改回硬写 `1.0` | 门 1 | `第四段必须落到顶点 alpha，rgb 不许被动过：[[0.2, 0.3, 0.4, 1.0], [0.2, 0.3, 0.4, 1.0], [0.2, 0.3, 0.4, 1.0], [0.2, 0.3, 0.4, 1.0], [0.2, 0.3, 0.4, 1.0], [0.2, 0.3, 0.4, 1.0]]` |
| RC-2a | `IMAGE_FIELDS` 的 `color` 标成 `Vec4` | 门 4 | `schema 对 "texture=a.png;color=1,1,1;nine_slice=8,8,16,16" 的判定必须与引擎一致（reason=Some("expected 4 comma-separated numbers, found 3")）` |
| RC-2b | 标成 `Vec3` | 门 4 | `schema 对 "texture=a.png;color=1,1,1,1" 的判定必须与引擎一致（reason=Some("expected 3 comma-separated numbers, found 4")）` |
| RC-3 | `property_path` 的 `matches!` 漏掉 `Color` | 门 4b | `assertion failed: field_text_equal("Image", "color", "0.2,0.3,0.4,0.55", "0.20,0.30,0.40,0.550")` |
| RC-4 | 写出端改成"永远四段" | 门 3 | `left: "texture=assets/ui/panel.png;color=0.949,0.918,0.847,1.000"` / `right: "…;color=0.949,0.918,0.847"` |

RC-1 的那句"没有任何门会红"是量过的，不是修辞：把发射端改回 `1.0` 跑
`cargo test -p engine --lib`，**159 条里只有门 1 红**（另一条红是既有无关红，见 §6 最后一段）。
四条破坏跑完全部回退（`grep -n "black_box\|RC-" `本批文件` 零命中`），回退后 §7 那套复绿。

## 6. 账：省第四段这条规则只有 1 处守护

RC-4 跑出来的结论跟预期不一样，要写下来。**三条"看着应该会红"的既有数据门实测全绿**：

| 我以为会红的门 | 为什么没红 |
| -------------- | ---------- |
| `cargo run -p editor --example migrate_project -- ../examples/game_jhyt ../examples/game_api`（dry-run，实得 `扫描 27 个文件，改动 0 个`） | 它只改写 `Transform3D` / 2D 旧 `Transform` / 相机改名（工具头注释 §"做三件事"），`Image` 载荷**根本不经它** |
| `editor::schema_audit::workspace_example_project_has_no_schema_drift` | 它判的是"载荷合不合法、组件名注册没有"，不判"是不是规范形" |
| `editor::document::sample_project_prefab_document_roundtrips_cleanly` | 断的是 `Sprite` 载荷 + 实体数，那张预设里根本没有 `Image` |

所以"alpha=1 只写三段"现在**只有门 3 一处**守护。买不到的部分：谁把写出端改成永远四段，
120 行会在编辑器一保存后整片重排，而这只会出现在 `git diff` 里。要把这条钉到数据上，
得给 `schema_audit` 加"逐字段比较存储串与编码串"的判定 —— 那是另一批，而且没那么便宜：
仓库里现存的 `Quad` 行就不是它自己编码器的字节输出（`examples/game_api/scenes/api_input.scene:5`
写的是 `color=0.06,0.07,0.10,1.000`，而 `Quad` 的编码端用 `{}` 精度，同一份值会写成
`0.06,0.07,0.1,1`），同名 `color` 在两个组件下有两套规范形，这条门要先逐组件定口径才能加。

其它买不到的部分：

- **Text 仍然没有 alpha**（`Text.color` 还是三段）。门 1 只钉 `Image`。
- **没有运行期写 alpha 的口子**：`prefab.*` 这七个函数改得了文字、矩形、显隐，改不了颜色。
  所以"每帧变的淡入淡出"照旧留在 `ui.*` 动态层 —— §1 那条边界本批只放开了一半（**作者值**能进
  预设，**运行期值**不能）。这一半要在真迁第一屏时重新判一次。
- **九宫格切片的 alpha 覆盖 9 块**（§3 那条链共用一个 `color`）：这是 `push_image_quad` 本来的
  形状，但门 1 用的是非切片路径，切片路径的 alpha 没有断言。
- **web / 微信没跑活体**。web 侧读第 4 段的代码早就在（`plugins/webgl/web-runtime.js:200` 的
  `parseColor` 保留全部逗号段 + `:632` 的 `ctx.globalAlpha = entity.image.color[3] === undefined ? 1 : …`），
  所以**推断**是零改动；没重打包、没对拍，这条与 64/65 号欠的是同一笔。微信走 wasm/native
  同一条 Rust 发射路径，同样未实测。
- 每帧成本没量：这次只是多带一个 `f32`（`DynamicUiImage.alpha`）与多读一位，无新增分配。
- **一条既有无关红**（不是本批造成，本批只是撞见）：`engine::sample_project::
  sample_project_demos_cover_every_lua_api` 红 `prefab.spawn/despawn/node/root/set_rect/set_text/
  set_visible` 七个 API 在 `examples/game_api` 没有演示。这七个函数是 M61/M62 加的，当时没往
  示例工程补演示，所以这红从那时起就在。它不在 pre-commit 的跑批里（hook 只跑 fmt / clippy /
  diff --check），所以没挡住提交，但它欠在**我自己的 M61/M62 账上**，排在 §8 第二条。

## 7. 跑过什么

```
cargo test -p rustlua-ui -p rustlua-ecs                     # 136 + 64 passed
cargo test -p engine --lib                                  # 158 passed; 1 failed（§6 那条既有红）
cargo test -p engine --test component_schema_parity         # 1 passed
cargo test -p editor --lib                                  # 180 passed
cargo run  -q -p game --example jhyt_m61_prefab_runtime     # ok（上一批没被碰坏）
cargo run  -q -p game --example jhyt_m62_prefab_write       # ok
cargo run  -q -p game --example jhyt_m63_title_prefab       # ok（68 行黄金表一字节没重导）
cargo clippy --workspace --all-targets -- -D warnings       # exit 0（fast 档，不带 --all-features）
RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps    # exit 0（18 个 crate，零告警）
cargo fmt --all -- --check                                  # clean
node scripts/gen-lua-api-docs.js --check                    # lua_api_docs_ok（ui.md 的 绑定 :NN 重生成，+3 行）
node scripts/check-web-api-overrides.mjs                    # web_api_overrides_ok overrides=56
```

`lua_api/doc/ui.md` 的改动是**纯行号**（本批在 `ui_panel.rs` 里加了 3 行注释 ⇒ 该页 15 条 `绑定 …:NN`
各自 +3）。生成器同时动了另外 5 页（`light` / `mesh` / `quad` / `sprite` / `world_text`）——
那 5 页红的是别的会话在途的 `renderable.rs`（+24 行），**不跟本批提交**。

没跑：`verify.ps1` 全量门与 `--all-features` clippy（工作区里还有另两路在途改动不属本批：
编辑器面板那一路、微信工具链那一路）。

## 8. 下一批

1. **M64b：实体层的裁剪 / 滚动**。这是五屏剩下的第二个卡点：shop / quest / shipping 三屏的列表
   走 `nav.scroll_*`（`ui.scroll_extents` / `scroll_to_visible` / `scroll_offset` / `scroll_close`，
   例如 `scripts/ui/quest_ui.lua:357`），实体层既没有裁剪矩形也没有滚动偏移。
   顺带要一起判的还有 `prefab.set_color`（§6 那条"运行期值"边界）。
2. **补 `prefab.*` 七个 API 在 `examples/game_api` 的演示**，把 §6 最后那条既有红清掉
   （它挡着 `cargo test -p engine --lib` 的干净读数）。
3. 然后回到 #13：面板五屏逐屏搬，一屏一笔、各配自己的黄金门。
