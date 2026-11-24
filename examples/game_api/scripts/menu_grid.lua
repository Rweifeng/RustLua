-- 入口菜单的卡片网格几何：**生成器与 Lua 面板共用的唯一真源**。
--
-- `scripts/docgen/gen-api-showcase-index.ps1` 读它来铺 `scenes/index.scene` 的卡片，
-- `scripts/demo_index.lua` require 它来画焦点环。两处各抄一份数字时，改一边就会让
-- "高亮框"和"卡片"错位——所以这里只允许有一份。
--
-- 字段全部是扁平数字（PowerShell 侧用正则取值，嵌套表会让解析变脆）。
-- `*_dy` 是卡片**内部**的相对位置（左上角为原点）：卡片文字是卡片的子实体，
-- `UiLayout` 的锚点矩形就是父实体的矩形，因此偏移按"相对父矩形中心"书写。
--
-- 尺寸来历：3 列 × 6 行必须装得下 16 个演示（M96 新增 `api_fx` 后从 15 变 16）。
-- 页脚从 y=610 起，可用高度只有 610-100=510；6 行要求 `top + 6*row_h <= 610`。
-- 取 top=92 / row_h=84 / card_h=76：末行 512..588，离页脚还有 22px 余量。
return {
    columns = 3,
    left = 48,
    top = 92,
    card_w = 288,
    card_h = 76,
    col_w = 296,
    row_h = 84,
    pad_x = 16,
    title_dy = 6,
    label_dy = 32,
    meta_dy = 54,
    font_title = 21,
    font_label = 14,
    font_meta = 13,
}
