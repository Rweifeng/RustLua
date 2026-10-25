-- 入口菜单的卡片网格几何：**生成器与 Lua 面板共用的唯一真源**。
--
-- `scripts/docgen/gen-api-showcase-index.ps1` 读它来铺 `scenes/index.scene` 的卡片，
-- `scripts/demo_index.lua` require 它来画焦点环。两处各抄一份数字时，改一边就会让
-- "高亮框"和"卡片"错位——所以这里只允许有一份。
--
-- 字段全部是扁平数字（PowerShell 侧用正则取值，嵌套表会让解析变脆）。
-- `*_dy` 是卡片**内部**的相对位置（左上角为原点）：卡片文字是卡片的子实体，
-- `UiLayout` 的锚点矩形就是父实体的矩形，因此偏移按"相对父矩形中心"书写。
return {
    columns = 3,
    left = 48,
    top = 100,
    card_w = 288,
    card_h = 96,
    col_w = 296,
    row_h = 102,
    pad_x = 16,
    title_dy = 10,
    label_dy = 42,
    meta_dy = 68,
    font_title = 24,
    font_label = 16,
    font_meta = 15,
}
