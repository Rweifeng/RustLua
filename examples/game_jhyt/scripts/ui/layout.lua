-- layout.lua：《江湖有田》设计分辨率与全局面板几何（M32，2026-09-27 转横屏）。
--
-- 为什么要有这个文件：改造前所有面板几何都散在各自模块里手写坐标
-- （`hud.lua` 的 `PANEL = {996,8,276,300}`、`shop.lua` 的 `PANEL = {320,140,640,440}` …），
-- 各处面板互相"不许重叠"这件事只存在于注释里。改版式时这类散落常量是最贵的部分：
-- 改一处漏一处，表现是"某两个面板叠在一起"，而没有任何报错。
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
--   * 结论：横屏把窗口定成 1280×720 时两者 1:1 对齐；本文件里的坐标同时就是屏幕像素。
--     `M.scale()` 是给"窗口不等于设计尺寸"的兜底（例如编辑器预览、无头验收里设了别的视口）：
--     面板按真实视口定位，文字字号按同一系数放大缩小。
local M = {}

-- 设计分辨率（横屏）。改这里 = 改整个游戏的设计画布（同时要改 scenes/*.scene 的
-- `meta ui_reference_width/height`、`tools/gen_*_scene.py` 的 REF_W/REF_H 与
-- `project.toml` 的 window_width/height，四处一致；`M.audit()` 只守本文件内部自洽）。
M.DESIGN_W = 1280
M.DESIGN_H = 720

-- 常用边距 / 内边距（左右各留 24 是"手指也不挡内容"的边距）。
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
-- 横屏版面（1280×720，左世界 + 右侧信息栏）：
--   ① 左侧世界：地图与角色铺满 0..1000（右侧 280 为信息栏，面板叠在世界右缘之上）
--   ② 顶部：scene 的 HudTitle/HudHint（左上角，不遮世界右侧）
--   ③ 右侧信息栏：状态面板 → 资源条 → 引导行 → 对话面板 → 目标行 → 反馈行 → 键位提示
--
-- 每个矩形都是 `{x, y, w, h}`，原点在左上，单位 = 设计像素（= 1280×720 屏幕像素）。
--
-- **同屏组**（`M.CONTEXT`）：并非所有面板都会同时出现。同组内的
-- 面板**必须互不重叠**；不同组的面板由状态机保证永不同屏，因此几何上允许叠。
-- 分组写在这里而不是靠注释，`M.audit()` 才守得住真正的不变量。
M.PANEL = {
  -- ① 状态面板（HUD）：右侧信息栏顶部，4 行状态；三条资源条画在它正下方。
  hud_info = { x = 1000, y = 8, w = 280, h = 124 },
  -- ② 标题门面的 3 块木牌按钮（与 main.scene 的 Btn* 矩形、title_menu 的 OPTIONS 同源）。
  title_btn = { x = 460, y = 380, w = 360, h = 64 },
  -- ③ 暂停菜单（主菜单页与设置页共用同一面板）。
  pause = { x = 160, y = 40, w = 720, h = 640 },
  -- ④ 背包面板：单列 10 行 + 详情 + 消息 + 提示。
  inventory = { x = 160, y = 40, w = 720, h = 640 },
  -- ⑤ 任务日志：单列 7 行 + 发布者 + 目标 + 说明 + 提示。
  quest = { x = 160, y = 40, w = 720, h = 640 },
  -- ⑥ 商店：单列 9 行 + 页签 + 数量 + 消息 + 提示。
  shop = { x = 160, y = 40, w = 720, h = 640 },
  -- ⑦ 对话面板：底部居中通栏（240..1040 / 460..680）。
  --    右侧 280 信息栏太窄放不下台词与选项，改到底部居中后横向空间从 280 增到 800，
  --    正文一行可排更多字；与常驻的右侧状态面板（1000..1280）不重叠。
  dialogue = { x = 240, y = 450, w = 800, h = 240 },
}

-- ---------- 触屏控件（虚拟摇杆 + 虚拟按钮） ----------
--
-- 为什么放这里：虚拟摇杆原来是 `ui/virtual_pad.lua` 里手写的一大堆散坐标
-- （交互 1180,600 / 攻击 1098,552 / 背包 1085,600 / 任务 1132,518 / 暂停 1180,680）。
-- 那些坐标**同时踩了两个坑**，而且都不报错：
--   ① 右缘越界：交互/暂停在 x=1180、半径 46/26 → 右缘 1226/1206，窗口一旦不是
--      1280 宽（桌面缩放、非 1:1 预览）就被切掉；
--   ② 压住 HUD 文字：「交互/背包」的 x 范围压在右栏目标行（y 566..590）上，
--      「任务」压在资源条/招式栏（y 234..282）上——实机截图里目标文案被两个圆盖住。
--
-- 因此几何收进本文件（与 `M.PANEL` 同源），由 `M.audit_touch()` 断言三条承诺：
--   * 每个控件都是设计画布内的**整圆**（不越界、不被切）；
--   * 控件之间互不重叠（相交即误触）；
--   * 每个控件都不与右栏同屏元素（状态面板 / 资源条 / 招式栏 / 目标行 / 反馈行 /
--     底部键位提示）的包围盒相交——手指不和文案抢地方。
--
-- 落位（设计画布 1280×720，横屏）：
--   * 摇杆在**世界左下角**（左拇指）。世界区 0..1000 内没有 HUD 文字，只有底部
--     键位提示行（y 648..678）且它只占左侧文字宽度（约 x≤690）；
--   * 动作按钮簇在**世界右下角**（右拇指），按 L 形排布：直角交点在**交互**（最大），
--     竖臂沿 x≈1170 向上串「攻击 → 背包」，横臂沿 y=635 向左串「任务 → 暂停」。
--     七日目标/动作反馈两行已挪到**左上角**（右栏腾空），L 整体贴住屏幕右下角：
--     右缘 1229、下缘 689，仍在设计画布 1280×720 内。
M.TOUCH = {
  -- 摇杆：底圆 52 + 外圈 4 = 56（按钮簇最近处距摇杆圆心 776，不会误触）。
  stick = { cx = 150, cy = 585, base_r = 52, knob_r = 21 },
  -- 拾取半径比视觉半径大 30：手指落在圈外一点也能推杆（触屏手感）。
  stick_grab_extra = 30,
  -- 方向阈值：占最大偏移的比例（0.35 ≈ 推过 1/3 才算该方向）。
  stick_deadzone = 0.35,
  -- 虚拟按钮。size = 直径；半径一律由 `size / 2` 推导（不再两处手抄半径/直径）。
  -- 触控目标直径都 ≥ 58，两两圆距 ≥ 6px，满足"最小可点面积"与"不误触"。
  --
  -- 七日目标/动作反馈已挪到左上角，右栏不再限制按钮 x 上界；唯一约束是画布
  -- 1280×720 与底部键位提示行（hud_keys y 648..678、x 24..696，横臂整体在其右侧）。
  -- L 形贴右下角：交互在直角交点（最大，最贴右拇指），竖臂向上串「攻击 → 背包」，
  -- 横臂向左串「任务 → 暂停」，两两圆距 ≥ 6px 不误触。尺寸与上一版一致。
  buttons = {
    { key = "keye", label = "交互", x = 1175, y = 635, size = 108 },
    { key = "keyj", label = "攻击", x = 1165, y = 480, size = 84 },
    { key = "keyi", label = "背包", x = 1175, y = 360, size = 72 },
    { key = "keyl", label = "任务", x = 1005, y = 635, size = 80 },
    { key = "keyp", label = "暂停", x = 855, y = 635, size = 68 },
  },
  -- 面板导航十字键（DPad + 中央确定）：只在**面板模式**显示（背包/任务/暂停/商店/对话），
  -- 与玩法按钮簇（L 形）不同屏，因此 `audit_touch` 按 group 分组：同组才查互不重叠，
  -- 越界与"不压右栏文字/键位提示"对全部圆生效。中央 ok 直径 80，四方向直径 64，
  -- 两两圆心距 84..90（≥ 半径和 + 12）；右缘 1276、下缘 707 都在设计画布内。
  -- 位置：屏幕右下角（右拇指），与 L 形按钮同区域——面板打开时按钮簇隐藏，DPad 顶替。
  dpad = {
    keys = {
      { dir = "up", label = "上", x = 1160, y = 495, size = 64 },
      { dir = "down", label = "下", x = 1160, y = 675, size = 64 },
      { dir = "left", label = "左", x = 1076, y = 585, size = 64 },
      { dir = "right", label = "右", x = 1244, y = 585, size = 64 },
      { key = "confirm", label = "确定", x = 1160, y = 585, size = 80 },
    },
  },
  -- 关闭按钮（×）：只在面板模式显示，位置 = **当前面板矩形右上角内侧**，不写死——
  -- 由 virtual_pad 按 `rect.x + rect.w - dx, rect.y + dy` 推算（modal 面板与对话面板
  -- 矩形不同）。audit_touch 用 inventory（modal 代表）与 dialogue 两个位置做
  -- 越界 / 不压右栏文字 / 与 DPad 不重叠检查（quest/pause/shop 与 inventory 同矩形）。
  close = { dx = 56, dy = 64, size = 44 },
  -- 按钮配色（语义 → RGB；绘制端只读这里，不再各自硬编码）。
  button_color = {
    primary = { 0.25, 0.55, 0.35 },   -- 交互 / DPad 确定
    combat = { 0.65, 0.30, 0.25 },    -- 攻击
    panel = { 0.30, 0.40, 0.60 },     -- 背包
    log = { 0.45, 0.40, 0.65 },       -- 任务
    system = { 0.40, 0.40, 0.42 },    -- 暂停 / DPad 方向键
  },
}

-- "会与触屏控件抢地方"的 UI 元素包围盒（`{x,y,w,h}`）。这些多数不是 `M.PANEL`
-- 成员（资源条 / 招式栏 / 文字行都不是面板），但**必须**参与触屏控件的重叠检查，
-- 否则又会回到"圆盖住目标文案、没人报错"的老路。
--
-- 它们是 `hud.lua` / `ROW` 的副本：写死在这里是刻意的——校验器要能在 HUD 挪位置时
-- **报警**，而不是跟着自动漂移（那正是原bug静默的原因）。配套的 `audit_touch()` 会
-- 断言 `hud_info` 与 `M.PANEL.hud_info` 仍然一致。
M.TOUCH_KEEP_CLEAR = {
  hud_info = M.PANEL.hud_info,                          -- 状态面板（1000,8 280×124）
  hud_bars = { x = 1012, y = 138, w = 160, h = 72 },    -- 三条资源条（18px 行 × 3）
  hud_skill_bar = { x = 1012, y = 234, w = 260, h = 48 }, -- 招式快捷栏（5 × 48 卡）
  hud_goals = { x = 16, y = 8, w = 760, h = 26 },       -- 七日目标行（左上角，ROW.goals_y）
  hud_feedback = { x = 16, y = 36, w = 520, h = 24 },     -- 动作反馈行（左上角，ROW.feedback_y）
  hud_keys = { x = 24, y = 648, w = 672, h = 30 },      -- 底部键位提示（11 项单行）
}

-- 版面常量：与面板矩形同源（同一份 720×1280 版面），但不是矩形，故不参与
-- "越界 / 互不重叠"检查。放这里是为了"改版式只改一个文件"。
M.METRIC = {
  -- 门面三块木牌按钮的纵向间隔（`title_menu` 按它递进排 OPTIONS；
  -- 与 main.scene 按钮 y 380/460/540 对齐，step = 64 + 16 = 80）。
  title_btn_gap = 16,
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

-- 信息栏 / 门面文字行（HUD 右栏与标题门面共用）。
M.ROW = {
  top_x = 16,           -- 顶部说明行（标题门面）
  top_y = 8,
  bottom_y = 690,       -- 右侧信息栏最后一行（游戏内键位提示 / 标题门面版本行）
  goals_y = 8,           -- 七日目标行（左上角，贴顶边）
  feedback_y = 36,      -- 当前动作反馈行（目标行之下 28px）
}

-- 面板内部的行偏移（全部以面板左上角为原点）。
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

-- ---------- 触屏控件自检 ----------

-- 圆（`{cx,cy,r}`）的包围盒。
local function circle_box(circle)
  return {
    x = circle.cx - circle.r,
    y = circle.cy - circle.r,
    w = circle.r * 2,
    h = circle.r * 2,
  }
end

local function boxes_overlap(a, b)
  return a.x < b.x + b.w and b.x < a.x + a.w and a.y < b.y + b.h and b.y < a.y + a.h
end

local function circles_overlap(a, b)
  local dx, dy = a.cx - b.cx, a.cy - b.cy
  local rr = a.r + b.r
  return dx * dx + dy * dy < rr * rr
end

-- 返回 (是否通过, 问题列表)。检查（对应文件头 `M.TOUCH` 的三条承诺）：
--   ① 每个控件（摇杆底圆 + 外圈、每个按钮的视觉圆）是设计画布内的**整圆**，
--      `cx - r >= 0`、`cx + r <= DESIGN_W`、y 同理 —— 越界等于被窗口切掉；
--   ② 控件两两不重叠（相交即误触）；
--   ③ 控件不与 `M.TOUCH_KEEP_CLEAR` 里右栏文字/资源条/键位提示的包围盒重叠。
function M.audit_touch()
  local issues = {}
  local touch = M.TOUCH
  if type(touch) ~= "table" then
    return false, { "layout.TOUCH 必须是表" }
  end

  -- 收集控件圆：摇杆外圈（底圆 + 4）、每个按钮（半径 = size/2）与每个十字键。
  -- group 标注控件所属场景：玩法（摇杆/按钮簇）与面板（十字键）不同屏，跨组不做重叠检查。
  local circles = {}
  local stick = touch.stick
  if type(stick) ~= "table" then
    issues[#issues + 1] = "layout.TOUCH.stick 必须是表"
  else
    local base = tonumber(stick.base_r) or 0
    circles[#circles + 1] = { name = "touch.stick", cx = stick.cx, cy = stick.cy, r = base + 4, group = "gameplay" }
  end
  for index, btn in ipairs(touch.buttons or {}) do
    local size = tonumber(btn.size) or 0
    local name = string.format("touch.buttons[%d](%s)", index, tostring(btn.label or btn.key))
    if size <= 0 then
      issues[#issues + 1] = "layout." .. name .. " 的 size 必须为正（当前 "
        .. tostring(btn.size) .. "）：半径由 size/2 推导，不再另写一份"
    end
    circles[#circles + 1] = {
      name = name,
      cx = btn.x,
      cy = btn.y,
      r = size / 2,
      group = "gameplay",
    }
  end
  local dpad = touch.dpad
  if type(dpad) ~= "table" or type(dpad.keys) ~= "table" then
    issues[#issues + 1] = "layout.TOUCH.dpad 必须是 { keys = { { dir/key, label, x, y, size } } } 表"
  else
    for index, key in ipairs(dpad.keys) do
      local size = tonumber(key.size) or 0
      local name = string.format("touch.dpad.keys[%d](%s)", index, tostring(key.dir or key.key or key.label))
      if size <= 0 then
        issues[#issues + 1] = "layout." .. name .. " 的 size 必须为正（当前 "
          .. tostring(key.size) .. "）：半径由 size/2 推导"
      end
      circles[#circles + 1] = {
        name = name,
        cx = key.x,
        cy = key.y,
        r = size / 2,
        group = "panel",
      }
    end
  end
  -- 关闭按钮：跟随面板矩形，audit 采样 inventory（modal 组代表，quest/pause/shop 同矩形）
  -- 与 dialogue 两个位置；与 DPad 同屏（panel 组），必须都不重叠、不越界、不压右栏文字。
  local close = touch.close
  if type(close) ~= "table" then
    issues[#issues + 1] = "layout.TOUCH.close 必须是 { dx, dy, size } 表"
  else
    local c_size = tonumber(close.size) or 0
    local close_samples = {}
    if type(M.PANEL.inventory) == "table" then
      close_samples[#close_samples + 1] = { name = "inventory(modal)", rect = M.PANEL.inventory }
    end
    if type(M.PANEL.dialogue) == "table" then
      close_samples[#close_samples + 1] = { name = "dialogue", rect = M.PANEL.dialogue }
    end
    for _, sample in ipairs(close_samples) do
      circles[#circles + 1] = {
        name = string.format("touch.close(%s)", sample.name),
        cx = sample.rect.x + sample.rect.w - (tonumber(close.dx) or 0),
        cy = sample.rect.y + (tonumber(close.dy) or 0),
        r = c_size / 2,
        group = "panel",
      }
    end
  end

  -- ① 整圆在设计画布内。
  for _, circle in ipairs(circles) do
    if type(circle.cx) ~= "number" or type(circle.cy) ~= "number" then
      issues[#issues + 1] = "layout." .. circle.name .. " 的 cx/cy 必须是数值"
    elseif circle.cx - circle.r < 0 or circle.cx + circle.r > M.DESIGN_W
        or circle.cy - circle.r < 0 or circle.cy + circle.r > M.DESIGN_H then
      issues[#issues + 1] = string.format(
        "layout.%s 越出设计画布 %dx%d（会被窗口切掉）：cx=%.0f cy=%.0f r=%.0f",
        circle.name, M.DESIGN_W, M.DESIGN_H, circle.cx, circle.cy, circle.r
      )
    end
  end

  -- ② 控件互不重叠（只对**同屏**的控件：玩法组与面板组不同屏，允许几何上叠）。
  for i = 1, #circles do
    for j = i + 1, #circles do
      local a, b = circles[i], circles[j]
      if (a.group or "gameplay") ~= (b.group or "gameplay") then
        -- 跨组：玩法控件只在玩法态、十字键只在面板态，永不共屏，跳过重叠检查。
      elseif type(a.cx) == "number" and type(b.cx) == "number" and circles_overlap(a, b) then
        issues[#issues + 1] = string.format(
          "layout.%s 与 layout.%s 重叠（半径和 %.0f > 圆心距 %.1f，会误触）",
          a.name, b.name, a.r + b.r, math.sqrt((a.cx - b.cx) ^ 2 + (a.cy - b.cy) ^ 2)
        )
      end
    end
  end

  -- ③ 不压右栏文字 / 资源条 / 键位提示。
  local keep = {}
  local keep_names = {}
  for name in pairs(M.TOUCH_KEEP_CLEAR or {}) do
    keep_names[#keep_names + 1] = name
  end
  table.sort(keep_names)
  for _, name in ipairs(keep_names) do
    keep[#keep + 1] = { name = name, box = M.TOUCH_KEEP_CLEAR[name] }
  end
  -- 副本一致性：`hud_info` 必须是 `M.PANEL.hud_info` 同一张表——否则"校验器盯着的框"
  -- 与"HUD 真正画的地方"就分叉了，那正是这类静默重叠的成因。
  if M.TOUCH_KEEP_CLEAR and M.TOUCH_KEEP_CLEAR.hud_info ~= M.PANEL.hud_info then
    issues[#issues + 1] =
      "layout.TOUCH_KEEP_CLEAR.hud_info 与 layout.PANEL.hud_info 不是同一张表："
      .. "副本已漂移，重叠检查会盯错框"
  end
  for _, circle in ipairs(circles) do
    if type(circle.cx) == "number" then
      local box = circle_box(circle)
      for _, target in ipairs(keep) do
        if type(target.box) == "table" and target.box.x
            and boxes_overlap(box, target.box) then
          issues[#issues + 1] = string.format(
            "layout.%s 压住 UI 元素 %s（包围盒 %.0f,%.0f %.0fx%.0f 相交）：触屏控件只允许落在世界区",
            circle.name, target.name,
            target.box.x, target.box.y, target.box.w, target.box.h
          )
        end
      end
    end
  end

  return #issues == 0, issues
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
  -- ⑤ 触屏控件（摇杆 + 虚拟按钮）：整圆不越界、互不重叠、不压右栏文字与键位提示。
  local ok_touch, touch_issues = M.audit_touch()
  if not ok_touch then
    for _, issue in ipairs(touch_issues or {}) do
      issues[#issues + 1] = issue
    end
  end
  return #issues == 0, issues
end

return M
