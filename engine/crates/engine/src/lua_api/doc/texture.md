# Lua 模块 `texture`

> 运行期 API 面基线里本模块有 3 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

程序化贴图（M82-B2）：按规格表在 CPU 侧确定性生成像素（棋盘/噪声/渐变/条纹），返回整型句柄，
零外部素材文件。这是**维度无关基底**（2D/3D 构建都登记）。宿主每帧把新生成的贴图上传到渲染器，
消费侧按哨兵键 `__texture__?<id>` 取样——`sprite.set_texture(entity, id)` 用现有 2D 精灵通道即可显示。
键由 id 唯一决定，同 id 重生成**原地替换**（渲染器按 key 复用槽位），因此每帧重生成同一张不增长纹理数。
`texture.generate` 的像素跨平台逐字节一致（整数噪声）；`texture.from_text` 用工程字体把文本画成同类独立
贴图（覆盖率来自 ab_glyph，同机同字体确定，跨平台不作整数级承诺）。

## `texture.free(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/texture_lua.rs:165`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

释放一个程序化贴图句柄：从运行期注册表移除该贴图，此后再消费该 id 会退白兜底（同文件贴图缺失）。
显式释放是"句柄生命周期"的唯一收缩点；稳定键复用是先于它的一层防泄漏（见模块说明）。

- 参数：`id`——`texture.generate` / `texture.from_text` 返回的整型句柄。
- 返回：`boolean`——该 id 是否存在并被移除（重复释放返回 `false`）。
- 失败：`id` 不是合法的非负整数时抛错。
- 示例：`texture.free(id)`

## `texture.from_text(text: String, opts: Option<Table>)`

> 绑定：`engine/crates/rustlua-render/src/texture_lua.rs:261`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把一段文本用工程字体栅格化成一张独立 RGBA 贴图（容器编号/限重标签那种"画一次贴一张"），返回句柄；
与 `texture.generate` 走同一条消费路径（宿主上传后按哨兵键 `__texture__?<id>` 取样）。

- 参数：`text`——要画的文本（`\n` 换行）；`opts`（可选表）——`size`（字号 px，`8..=256`，缺省 `24`）、
  `padding`（内边距 `0..=64`，缺省 `4`）、`color`（文字 `{r,g,b[,a]}`，缺省白）、`bg`（背景 `{r,g,b[,a]}`，
  缺省透明）、`max_width`（贴图宽度上限，给定则按字符折行）、`font`（相对工程根的字体路径，缺省
  `assets/fonts/NotoSansSC-VF.ttf`）。
- 返回：`integer` 句柄（自增）。
- 失败：空文本、字号/内边距越界、贴图超过 `1024`（给 `max_width` 折行）、无工程根或字体读不到/解析失败时抛错。
- 示例：`local id = texture.from_text('B2-42', {size=12, color={255,220,120}})`

## `texture.generate(spec: Table)`

> 绑定：`engine/crates/rustlua-render/src/texture_lua.rs:152`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

按规格表生成一张 `size×size` 的 RGBA8 贴图，登记并返回句柄。生成是纯 CPU、seed 确定的：同 spec
同字节，可直接当渲染资源用（宿主上传后 sprite/quad 即按哨兵键取样）。

- 参数：`spec` 表——
  - `kind`（必填）：`checker` | `noise` | `gradient` | `stripes`。
  - `size`：方形边长，`4..=1024`，缺省 `64`。
  - `seed`：噪声种子（仅 `noise` 敏感），缺省 `0`。
  - `scale`：频率（格数/条数），`1..=256`，缺省 `8`。
  - `colors`：调色板，`2..=8` 个 `{r,g,b}` 或 `{r,g,b,a}`（通道 `0..=255`），缺省黑白。
- 返回：`integer` 句柄（自增，从 `0` 起）。
- 失败：未知 `kind`、`size`/`scale` 越界、颜色数或通道不合法时抛错（不静默成功）。
- 示例：`local id = texture.generate{kind='checker', size=32, scale=4, colors={{255,80,80},{80,80,255}}}`
