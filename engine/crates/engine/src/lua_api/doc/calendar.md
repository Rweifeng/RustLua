# Lua 模块 `calendar`

> 运行期 API 面基线里本模块有 3 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `calendar.configure(options: mlua::Table)`

> 绑定：`engine/crates/rustlua-gameplay/src/clock_lua.rs:134`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

改写日历的体裁尺寸：`options.days_per_season`（每季天数）与 `options.seasons_per_year`（每年季数）。
默认 28 × 4 是农场体裁默认值，不是引擎语义；只改季节/年的派生口径与 `calendar.set` 的上界，
不改当前绝对天与分钟。

- 参数：`options`（table）——`days_per_season`、`seasons_per_year`，都必须是 `>= 1` 的整数，
  且乘积不超过 `u32::MAX`。
- 返回：无。
- 失败：缺字段、非正整数或乘积溢出时报 Lua 错误，且不改动任何配置。
- 示例：

  ```lua
  calendar.configure({ days_per_season = 7, seasons_per_year = 3 })
  calendar.set(8, 1, 1, 0)  -- 报错：day must be in 1..=7
  ```

## `calendar.now()`

> 绑定：`engine/crates/rustlua-gameplay/src/clock_lua.rs:120`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `calendar.set(day: i64, season: i64, year: i64, minute: i64)`

> 绑定：`engine/crates/rustlua-gameplay/src/clock_lua.rs:159`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
