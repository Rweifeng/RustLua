-- layout.lua：《江湖有田》竖版设计分辨率与全局面板几何（M32）。
--
-- 为什么要有这个文件：改造前所有面板几何都散在各自模块里手写 1280×720 的坐标
-- （`hud.lua` 的 `PANEL = {996,8,276,300}`、`shop.lua` 的 `PANEL = {320,140,640,440}` …），
-- 六处面板互相"不许重叠"这件事只存在于注释里。横屏改竖版时这类散落常量是最贵的部分：
-- 改一处漏一处，表现是"某两个面板在竖屏下叠在一起"，而没有任何报错。
--
-- 于是把**设计分辨率 + 全部面板矩形 + 行宽预算**收进这一个模块（唯一定义处），
-- 判定规则也写在这里：`M.audit()` 检查每个矩形都落在设计画布内、且各面板互不重叠。
-- 验收（`jhyt_m19_ui` / `jhyt_m30_cjk_text`）与运行时读的是同一份几何，不存在"注释说一套、
-- 代码做另一套"。
--
-- 坐标口径（坑，写清楚）：
--   * 引擎的 **场景 UI**（scene 文件里的 `UiLayout`）在 Canvas 空间，按
--     `viewport / reference` 缩放；参考分辨率由 `meta ui_reference_width/height` 声明。
--   * 引擎的 **Lua 动态 UI**（`ui.panel` / `ui.image` / `ui.text_at`）恒为 Screen 空间
--     （`core/script.rs` 里 `space: UiLayoutSpace::Screen`），**不随参考分辨率缩放**。
--   * 结论：竖版把窗口定成 720×1280 时两者 1:1 对齐；本文件里的坐标同时就是屏幕像素。
--     `M.scale()` 是给"窗口不等于设计尺寸"的兜底（例如编辑器预览、无头验收里设了别的视口）：
--     面板按真实视口定位，文字字号按同一系数放大缩小。
local M = {}

-- 竖版设计分辨率。改这里 = 改整个游戏的设计画布（同时要改 scenes/*.scene 的
-- `meta ui_reference_width/height`、`tools/gen_*_scene.py` 的 REF_W/REF_H 与
-- `project.toml` 的 window_width/height，四处一致；`M.audit()` 只守本文件内部自洽）。
M.DESIGN_W = 720
M.DESIGN_H = 1280

-- 常用边距 / 内边距（竖屏单列，左右各留 24 是"手指也不挡内容"的边距）。
M.MARGIN = 24
M.PAD = 24

-- 视口（屏幕像素）。无头验收与编辑器可以设成别的尺寸；发布窗口 = 设计尺寸。
local viewport = { w = M.DESIGN_W, h = M.DESIGN_H }

-- 设置当前视口（由 main.lua 在启动与场景切换时同步）。
function M.set_viewport(width, height)
  local w = tonumber(width)
  local h = tonumber(height)
  if w and h and w > 0 and h > 0 then
    viewport.w = w
    viewport.h = h
  end
  return true
end

function M.viewport()
  return { w = viewport.w, h = viewport.h }
end

-- 视口相对设计分辨率的缩放系数（x/y 各一轴，与引擎 Canvas 的 `viewport / reference` 同口径）。
function M.scale()
  return viewport.w / M.DESIGN_W, viewport.h / M.DESIGN_H
end

-- 字号按同一系数缩放（屏幕像素空间下，字号不跟着缩就会"面板放大了、字还是 14px"）。
function M.font(size)
  local _, sy = M.scale()
  return tonumber(size) * sy
end

-- ---------- 面板矩形（唯一定义处） ----------
--
-- 竖屏版面（自上而下）：
--   ① 顶部地图标题/操作提示：scene 的 HudTitle/HudHint（y 20..72）
--   ② 中部世界：完整留给地图与角色
--   ③ 底部信息区：状态面板 → 资源条 → 目标行 → 当前提示 → 键位提示
--
-- 每个矩形都是 `{x, y, w, h}`，原点在左上，单位 = 设计像素（= 720×1280 屏幕像素）。
--
-- **同屏组**（`M.CONTEXT`）：竖屏只有一块屏幕，但并非所有面板都会同时出现。同组内的
-- 面板**必须互不重叠**；不同组的面板由状态机保证永不同屏，因此几何上允许叠。
-- 分组写在这里而不是靠注释，`M.audit()` 才守得住真正的不变量。
M.PANEL = {
  -- ① 状态面板（HUD）：底部通栏，5 行状态；三条资源条画在它正上方。
  hud_info = { x = 24, y = 1020, w = 672, h = 124 },
  -- ② 标题门面的 3 块木牌按钮（与 main.scene 的 Btn* 矩形、title_menu 的 OPTIONS 同源）。
  title_btn = { x = 180, y = 620, w = 360, h = 64 },
  -- ③ 暂停菜单（主菜单页与设置页共用同一面板）。
  pause = { x = 64, y = 330, w = 592, h = 620 },
  -- ④ 背包面板：单列 10 行 + 详情 + 消息 + 提示。
  inventory = { x = 64, y = 240, w = 592, h = 700 },
  -- ⑤ 任务日志：单列 7 行 + 发布者 + 目标 + 说明 + 提示。
  quest = { x = 64, y = 240, w = 592, h = 720 },
  -- ⑥ 商店：单列 9 行 + 页签 + 数量 + 消息 + 提示。
  shop = { x = 64, y = 240, w = 592, h = 720 },
  -- ⑦ 对话面板：底部通栏、**在状态面板正上方**（800..1000 vs 1020..1144，留 20px 缝）。
  --    两块面板因此可以同屏：说话时照样看得见气血/铜钱/任务目标，这比"收起状态面板"
  --    更好——竖屏本来就窄，底部一块通栏放得下"对话框 + 状态行"，没必要二选一。
  dialogue = { x = 24, y = 800, w = 672, h = 200 },
}

-- 版面常量：与面板矩形同源（同一份 720×1280 版面），但不是矩形，故不参与
-- "越界 / 互不重叠"检查。放这里是为了"改版式只改一个文件"。
M.METRIC = {
  -- 门面三块木牌按钮的纵向间隔（`title_menu` 按它递进排 OPTIONS）。
  title_btn_gap = 24,
}

-- 同屏分组：组名 → 面板名列表。**同一组内的面板要求"互不重叠"**；组外的面板
-- 与它们可以同屏，但不在这一组的口径里（跨组关系由状态机与各自模块负责）。
M.CONTEXT = {
  -- 玩法态：HUD 常驻，对话面板在它正上方（800..1000 / 1020..1144）。
  gameplay = { "hud_info", "dialogue" },
  -- 四种模态面板：同一时刻只会开一个（Pause / Inventory / Quest / Shop），
  -- 因此它们共用中部版面、几何上本来就该叠在一起。
  modal = { "pause", "inventory", "quest", "shop" },
  -- 标题门面：只有三块按钮，没有 HUD。
  title = { "title_btn" },
}

-- 互斥组：组内面板**同一时刻只会出现一个**，因此它们之间不做重叠检查
-- （几何上叠在一起是有意的：同一块中部版面轮流被四种模态面板使用）。
-- 这与 `M.CONTEXT` 的分工是：`M.CONTEXT` 声明"谁可能同屏"（要求不重叠），
-- 这里声明"谁必然互斥"（豁免重叠检查）。一个面板必须恰好属于一个 `M.CONTEXT` 组。
M.EXCLUSIVE_GROUPS = { "modal" }

-- 顶部 / 底部通栏文字行（HUD 与标题门面共用"最下面一行"的位置）。
M.ROW = {
  top_x = 16,           -- 顶部说明行（标题门面）
  top_y = 8,
  bottom_y = 1246,      -- 底部最后一行（游戏内键位提示 / 标题门面版本行）
  goals_y = 1178,       -- 七日目标行
  feedback_y = 1204,    -- 当前动作反馈行
}

-- 面板内部的行偏移（竖版单列版面，全部以面板左上角为原点）。
M.BOX = {
  title_dx = 24,
  title_dy = 16,
  title_font = 22.0,
  corner_y = 20,      -- 右上角角标（状态/容量/金币）的基线偏移
  corner_font = 16.0,
  list_dx = 24,       -- 列表行缩进
  list_dy = 60,       -- 列表首行
  detail_dy = -96,    -- 详情行（距面板下沿，背包专用）
  msg_dy = -64,       -- 消息行（距面板下沿）
  hint_dy = -32,      -- 提示行（距面板下沿）
  hint_font = 14.0,
}

-- 面板内可用文字宽度：列表 / 消息 / 提示统一口径（不再各写一份 `w - 48`）。
function M.list_width(box)
  return box.w - M.PAD * 2
end

-- 通栏文字行宽预算：设计宽 - 两侧边距。
function M.row_width()
  return M.DESIGN_W - M.MARGIN * 2
end

-- **允许重叠**的面板对：改为"同屏分组"口径后不再需要白名单——
-- 只有 `M.CONTEXT` 里同一组的面板才会被检查，跨组面板由状态机保证不同屏。
-- 这里保留一个空表白名单通道（供将来"确实要同屏叠"的少数情况显式登记理由），
-- 但默认是空的：没有白名单时，同组内任何重叠都报错。
local OVERLAP_ALLOWED = {}

local function overlap_key(a, b)
  return (a < b) and (a .. "|" .. b) or (b .. "|" .. a)
end

-- ---------- 自检（改几何后的第一道网） ----------

-- 返回 (是否通过, 问题列表)。检查：
--   ① 每个面板矩形都在设计画布内（`x >= 0`、`y >= 0`、`x+w <= 宽度`、`y+h <= 高度`）；
--   ② `M.CONTEXT` 里同组面板两两不重叠（**互斥组** `M.EXCLUSIVE_GROUPS` 豁免）；
--   ③ 分组自洽：每个面板恰好属于一个组、组里不写不存在的面板名、豁免组名必须真实存在；
--   ④ 设计宽高为正整数。
function M.audit()
  local issues = {}
  if type(M.DESIGN_W) ~= "number" or M.DESIGN_W <= 0 or M.DESIGN_W % 1 ~= 0 then
    issues[#issues + 1] = "layout.DESIGN_W 必须是正整数：" .. tostring(M.DESIGN_W)
  end
  if type(M.DESIGN_H) ~= "number" or M.DESIGN_H <= 0 or M.DESIGN_H % 1 ~= 0 then
    issues[#issues + 1] = "layout.DESIGN_H 必须是正整数：" .. tostring(M.DESIGN_H)
  end
  -- ① 矩形合法 + 落在设计画布内。
  local names = {}
  for name in pairs(M.PANEL) do
    names[#names + 1] = name
  end
  table.sort(names)
  for _, name in ipairs(names) do
    local box = M.PANEL[name]
    if type(box) ~= "table" or type(box.x) ~= "number" or type(box.y) ~= "number"
        or type(box.w) ~= "number" or type(box.h) ~= "number" then
      issues[#issues + 1] = "layout.PANEL." .. name .. " 必须是 {x,y,w,h} 数值表"
    elseif box.w <= 0 or box.h <= 0 then
      issues[#issues + 1] = "layout.PANEL." .. name .. " 宽高必须为正："
        .. tostring(box.w) .. "x" .. tostring(box.h)
    elseif box.x < 0 or box.y < 0 or box.x + box.w > M.DESIGN_W or box.y + box.h > M.DESIGN_H then
      issues[#issues + 1] = string.format(
        "layout.PANEL.%s 越出设计画布 %dx%d：x=%s y=%s w=%s h=%s",
        name, M.DESIGN_W, M.DESIGN_H, tostring(box.x), tostring(box.y), tostring(box.w), tostring(box.h)
      )
    end
  end
  -- ③ 分组自洽：每个面板必须恰好在一个组里（漏登记 = 这个面板从此不受重叠检查）。
  local in_context = {}
  local context_names = {}
  for context in pairs(M.CONTEXT) do
    context_names[#context_names + 1] = context
  end
  table.sort(context_names)
  for _, context in ipairs(context_names) do
    local members = M.CONTEXT[context]
    if type(members) ~= "table" or #members == 0 then
      issues[#issues + 1] = "layout.CONTEXT." .. context .. " 必须是非空面板名列表"
    else
      for _, name in ipairs(members) do
        if not M.PANEL[name] then
          issues[#issues + 1] = "layout.CONTEXT." .. context .. " 引用了不存在的面板：" .. tostring(name)
        elseif in_context[name] then
          issues[#issues + 1] = "面板 " .. tostring(name) .. " 同时出现在多个同屏组（" .. in_context[name]
            .. " / " .. context .. "）：分组口径必须唯一"
        else
          in_context[name] = context
        end
      end
    end
  end
  for _, name in ipairs(names) do
    if not in_context[name] then
      issues[#issues + 1] = "layout.PANEL." .. name .. " 未登记同屏组：不登记就不会被重叠检查覆盖"
    end
  end
  -- ② 同组内两两不重叠——但**互斥组**（`M.EXCLUSIVE_GROUPS`）豁免：组内面板同一时刻
  --    只会开一个（四种模态面板共用中部版面），几何上叠在一起是有意的。
  local exclusive = {}
  for _, context in ipairs(M.EXCLUSIVE_GROUPS or {}) do
    exclusive[context] = true
    if not M.CONTEXT[context] then
      issues[#issues + 1] = "layout.EXCLUSIVE_GROUPS 引用了不存在的分组：" .. tostring(context)
    end
  end
  for _, context in ipairs(context_names) do
    local members = M.CONTEXT[context]
    if type(members) == "table" and not exclusive[context] then
      for i = 1, #members do
        for j = i + 1, #members do
          local a, b = M.PANEL[members[i]], M.PANEL[members[j]]
          if type(a) == "table" and type(b) == "table" and a.x and b.x then
            local overlap = a.x < b.x + b.w and b.x < a.x + a.w and a.y < b.y + b.h and b.y < a.y + a.h
            if overlap and not OVERLAP_ALLOWED[overlap_key(members[i], members[j])] then
              issues[#issues + 1] = "layout.PANEL." .. members[i] .. " 与 " .. members[j]
                .. " 重叠（同屏组 " .. context .. " 内不可叠；若确实互斥请改分组）"
            end
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
