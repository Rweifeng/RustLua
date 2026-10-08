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
--   ① 左侧世界：地图与角色铺满 0..980（右侧 300 为信息栏，面板叠在世界右缘之上）
--   ② 顶部：scene 的 HudTitle/HudHint（左上角，不遮世界右侧）
--   ③ 右侧信息栏：状态面板（6 行）→ 资源条 → 引导行 → 招式栏 → 对话面板
--      （底部键位通栏已撤下，操作说明收进暂停菜单的「帮助」页）
--
-- 每个矩形都是 `{x, y, w, h}`，原点在左上，单位 = 设计像素（= 1280×720 屏幕像素）。
--
-- **同屏组**（`M.CONTEXT`）：并非所有面板都会同时出现。同组内的
-- 面板**必须互不重叠**；不同组的面板由状态机保证永不同屏，因此几何上允许叠。
-- 分组写在这里而不是靠注释，`M.audit()` 才守得住真正的不变量。
M.PANEL = {
  -- ① 状态面板（HUD）：右侧信息栏顶部，**6 行**状态；三条资源条画在它正下方。
  --    宽 280 → 300、高 124 → 180 的来由是"每行都不许被裁"：18px 下最长的一行
  --    「第999天 冬 12:00 · 天气 晴」实测 241.1px，而 280 宽面板的文字预算只有
  --    242px（`PANEL.w - 24 - 色块 - 间距`），零余量；300 宽给到 262px，六行里最长
  --    的 241.1 也留 21px 余量（压力值 金钱 999999 / 第999天 / x999 同表实测）。
  --    高度 = 顶部 10 + 6 × 28 = 178 → 180。
  --    原第 4 行的"地图"不再进面板：地图名由场景自带的 `HudTitle`（左上角）显示，
  --    面板里重复一份正是当初挤爆一行、被迫加省略号的原因。
  hud_info = { x = 980, y = 8, w = 300, h = 180 },
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
--   * 每个控件都不与右栏同屏元素（状态面板 / 资源条 / 招式栏 / 目标行 / 反馈行）的
--     包围盒相交——手指不和文案抢地方。
--
-- 落位（设计画布 1280×720，横屏）：
--   * 摇杆在**世界左下角**（左拇指）。世界区 0..980 内没有 HUD 文字；
--   * 动作按钮簇在**世界右下角**（右拇指）：交互主键与摇杆同高（y=585）且同径（112），
--     其余四键 68 直径等分绕在它的左上象限（R=140，极角 90/120/150/180°），详见
--     `TOUCH.buttons` 上方的推导。整簇 x 1030..1260、y 411..641，四边都在画布内。
--     七日目标/动作反馈两行已挪到**左上角**（右栏腾空），底部通栏只到 x=680，
--     所以这一簇的左端离它还差 350。上端躲招式栏（包围盒 992,296 256×100，下缘 396）余 15
--     ——招式扩到 8 招改成两行之后这条从 67 收紧到 15，见下面 `TOUCH.buttons` 的三条余量。
M.TOUCH = {
  -- 摇杆：底圆 52 + 外圈 4 = 56（按钮簇最近处＝暂停圆心距摇杆圆心 894，不会误触）。
  -- `hole_r` / `knob_travel` 是**切图契约**，不是可调手感：
  --   hole_r = 45 → `tools/gen_ui_art.py` 的 `touch_stick_base` 透明孔半径（改了要重出图）；
  --   knob_travel = 25 → 杆头**视觉**偏移上限。`ui.image` 的多条记录在
  --   `__rustlua_export_ui` 里走 Lua `pairs`（web-runtime.js:1935），同表内没有绘制顺序
  --   保证，所以轨道与杆头两张图必须几何上不相交：杆头画出的半径是 18（42 画布留
  --   描边），18 + 25 = 43 < 45 ⇒ 永远在孔内，谁盖谁都一样。
  --   手指仍按 `base_r` 全行程推（方向阈值不变），只是杆头到孔边就停。
  stick = {
    cx = 150, cy = 585, base_r = 52, knob_r = 21,
    hole_r = 45, knob_travel = 25,
    art = "touch_stick_base", knob_art = "touch_stick_knob",
  },
  -- 拾取半径比视觉半径大 30：手指落在圈外一点也能推杆（触屏手感）。
  stick_grab_extra = 30,
  -- 方向阈值：占最大偏移的比例（0.35 ≈ 推过 1/3 才算该方向）。
  stick_deadzone = 0.35,
  -- 虚拟按钮。size = 直径；半径一律由 `size / 2` 推导（不再两处手抄半径/直径）。
  --
  -- **交互键与摇杆同高、同径；其余四键（68 直径）等分绕在它的左上象限**：以交互圆心为
  -- 圆心、半径 **140** 的弧上，极角 90°/120°/150°/180°（正上 → 正左，四等分那 90°）：
  --   攻击 (1204,445)、背包 (1134,464)、任务 (1083,515)、暂停 (1064,585)。
  -- 交互圆心 (1204,585) 的 `y = 585` 就是 `stick.cy`（左右拇指同一条水平线），直径 112
  -- = `touch_stick_base.png` 的出图直径（摇杆底盘 56×2），右缘留 20（1280−20−56）。
  -- 弦长公式（同弧相邻两键）= 2R·sin(步角/2)，四键三步把 90° 张角吃完 ⇒ 步角 30°
  -- ⇒ 弦长 = 0.518R。**"均匀 + 左上 90° + 直径 68"这三个前提锁死之后，R 只有一个下限**：
  -- 弦长 = 68 ⇒ **R = 131.4**（两圆相切，再小就相交、`audit_touch` ② 判红）。也就是说
  -- 这一档已经走到"再靠近就重叠"的门口，三档实测（间隙 = 弦长 − 68）：
  --   * R=160（上一版）⇒ 弦 82.7/83.4/82.7、间隙 14.7..15.4、弧离主键边缘 70；
  --   * R=140（这一版）⇒ 弦 72.5/72.1/72.5、**间隙 4.1..4.5**、弧离主键边缘 50；
  --   * R=125（反面对照，见下）⇒ 弦 64.3/64.4/65.3 ⇒ ② 三条邻键全部判红。
  -- 4 px 间隙在拇指下几乎等于贴着：设计 1280 宽 ≈ 844 CSS px ⇒ 4 设计 px ≈ 2.6 CSS px
  -- ≈ 0.5 mm。**下一档不能再动 R，只能动前提**：① 四键缩到 60（同 R=140 间隙回到 12.5，
  -- 但要重出切图，且 60 已经低于下面那条 44 pt 触点线）；② 张角放宽到 105°
  -- （90/125/160/195° @ R=150，弦 90.1..90.6、间隙 22）——代价是最末一键越过正左、落到
  -- 偏下 15°，那就不是"左上"了。上一版 2×2 方块（间隙 17.6 但只占一角）已按这条换掉。
  -- 68 的触点账：设计 1280 宽在横屏手机上约映射到 844 CSS px（iPhone 14 量级），
  -- 68 设计 px ⇒ 44.9 CSS px ≈ 7.8 mm，落在 iOS HIG 的 44 pt 最小触点上；本文件原先写的
  -- "78 px ≈ 9 mm"是舒适线不是下限，68 在它下面一档。
  -- 三条余量（动一格会先撞，按紧→松）：
  --   * 键间隙 4.1（上面那条折中的直接后果，本簇真正的紧约束）；
  --   * 招式栏：最上的攻击包围盒上缘 411 距 `TOUCH_KEEP_CLEAR.hud_skill_bar` 下缘 396 有 15
  --     （`audit_touch` ③ 查的是**包围盒**不是圆，所以按 411 而不是按圆顶算）。原本这一条是 67
  --     （招式栏只有 5 张、一行到 344 为止），2026-10-05 扩到 8 招改成两行之后收紧到 15——
  --     攻击键往下挪 15px 就会压到第二行卡片，临界 y=430（包围盒上缘 = y−34，429 已算相交）；
  --   * 画布：整簇 x 1030..1260、y 411..641，四边都宽（左距引导通栏 x ≤ 680 有 350）。
  -- 反向验过门看着这三条：R 收到 125 ⇒ ② 连报三条邻键重叠（`buttons[2](攻击) 与
  -- buttons[3](背包) 重叠（半径和 68 > 圆心距 64.3，会误触）` + 背包/任务 64.4 + 任务/暂停
  -- 65.3），门 exit 1；攻击单独挪到 (1160,530) ⇒ ② 报
  -- `buttons[1](交互) 与 buttons[2](攻击) 重叠（半径和 90 > 圆心距 70.4，会误触）`；攻击
  -- 下移到 y=425（包围盒上缘 391 < 招式栏下缘 396）⇒ ③ 报 `buttons[2](攻击) 压住 UI 元素
  -- hud_skill_bar（包围盒 992,296 256x100 相交）`。都恢复后绿灯。（③ 的临界在 y=430：
  -- 包围盒上缘 = y−34，429 已经算相交。）
  --
  -- `art` = `ui/skin.lua` 的贴图键，**直径必须等于 size**（1:1 出图，非整数缩放会糊边，
  -- 见 `tools/gen_ui_art.py` 的 TOUCH 节头）。改 size 要同步改生成器并重出切图。
  buttons = {
    { key = "keye", label = "交互", x = 1204, y = 585, size = 112, art = "touch_btn_interact" },
    { key = "keyj", label = "攻击", x = 1204, y = 445, size = 68, art = "touch_btn_attack" },
    { key = "keyi", label = "背包", x = 1134, y = 464, size = 68, art = "touch_btn_bag" },
    { key = "keyl", label = "任务", x = 1083, y = 515, size = 68, art = "touch_btn_quest" },
    { key = "keyp", label = "暂停", x = 1064, y = 585, size = 68, art = "touch_btn_pause" },
  },
  -- 「更多」键（M34-06 批 2）：招式栏第二行的**空位**——每行 5 格、玩家槽位 7 张，
  -- 第 3 格起是空的。画在那儿而不是再开一块地方：它是"卡片家族"的一员（同一张
  -- `skill_card` 底、同一套 48 格），玩家一眼归类；而右下半屏已经被摇杆 + 五键占满，
  -- 第六个圆就是"手指盖住画面"。它是**矩形**，`audit_touch` ①②③ 只审圆，所以
  -- 由第 ⑥ 条专门守着它整块落在 `TOUCH_KEEP_CLEAR.hud_skill_bar` 里（飘出格子 = 盖字）。
  more = { x = 1096, y = 348, size = 48, label = "更多", art = "skill_card" },
  -- 动作盘（点「更多」展开，再点收起）：把玩法态**只剩键盘能按**的 9 条动作收进来
  -- （睡觉 T / 建造 B / 放置 F / 运气 Q / 拆除 X / 升级 U / 送礼 G / 选种 C / 重开 R）。
  -- 落位在世界区右下（x 748..968、y 411..631）：左边离摇杆（外圈右缘 206）854、右边离
  -- 按钮簇（包围盒左缘 1030）62、上边离招式栏（下缘 396）15、下边离引导通栏（上缘 660）29
  -- ——`audit_touch` ①②③ 三条都按**包围盒**算，所以这四个数是门读数，不是估计。
  -- 3×3 = 9 格刚好吃完（`audit_touch` ⑦ 要求 `#items == #cols × #rows`，多一项少一项都红）。
  -- 直径 68 与四颗副键同档：设计 68px ≈ 44.9 CSS px ≈ 7.8 mm，在 iOS HIG 的 44pt 线上。
  action = {
    art = "touch_btn_action",
    size = 68,
    cols = { 782, 858, 934 },
    rows = { 445, 521, 597 },
    items = {
      { action = "sleep", label = "睡觉" },
      { action = "build", label = "建造" },
      { action = "place", label = "放置" },
      { action = "practice", label = "运气" },
      { action = "demolish", label = "拆除" },
      { action = "use", label = "升级" },
      { action = "gift", label = "送礼" },
      { action = "seed", label = "选种" },
      { action = "respawn", label = "重开" },
    },
  },
  -- 面板态动作条（M34-06 批 3）：面板里除了"移动选择 + 确定 + 关闭"，还有几条键鼠动作
  -- 在触屏上根本没有入口（E 装备 / C 选种 / F 换页 / +/- 数量与音量）。
  -- 落位：2×2 摆在**面板矩形（160..880）与十字键（1042..1276）之间**那块空档的下半截
  -- ——列 920/992、行 591/667，包围盒 x 886..1026、y 557..701（四边都在画布内）。
  -- 为什么不是原计划的"竖排三格从 y=515 起"：`audit_touch` ② 当场把它判红了 ——
  -- 对话面板的关闭键采样位 (984,514) 半径 22，与 (948,515) 半径 34 的圆心距只有 36，
  -- 半径和 56 > 36（`会误触`）。往下挪两行、改成 2×2 之后最近的一对是 (992,591)↔关闭键
  -- 圆心距 77，与十字键左键 (1076,585) 圆心距 84（半径和 66）——四个数都是门读数。
  -- 每个模式用的格数不同（背包 2 / 商店 3 / 暂停 2），所以 ⑧ 只要求"装得下 4 格"，
  -- 不像动作盘那样要求正好填满。
  panel_actions = {
    art = "touch_btn_action",
    size = 68,
    cols = { 920, 992 },
    rows = { 591, 667 },
    modes = {
      Inventory = {
        { action = "interact", label = "装备" },
        { action = "seed", label = "选种" },
      },
      Shop = {
        { action = "place", label = "换页" },
        { action = "volume_up", label = "数量+" },
        { action = "volume_down", label = "数量-" },
      },
      Pause = {
        { action = "volume_up", label = "音量+" },
        { action = "volume_down", label = "音量-" },
      },
    },
  },
  -- 面板导航十字键（DPad + 中央确定）：只在**面板模式**显示（背包/任务/暂停/商店/对话），
  -- 与玩法按钮簇（一排）不同屏，因此 `audit_touch` 按 group 分组：同组才查互不重叠，
  -- 越界与"不压右栏文字/资源条"对全部圆生效。中央 ok 直径 80，四方向直径 64，
  -- 两两圆心距 84..90（≥ 半径和 + 12）；右缘 1276、下缘 707 都在设计画布内。
  -- 位置：屏幕右下角（右拇指），与按钮排同区域（玩法排的 y 带与之重叠）——面板打开时
  -- 按钮簇隐藏、DPad 顶替，所以跨组不查重叠是**有前提的**：面板态必须真的隐藏玩法排。
  dpad = {
    keys = {
      { dir = "up", label = "上", x = 1160, y = 495, size = 64, art = "touch_dpad" },
      { dir = "down", label = "下", x = 1160, y = 675, size = 64, art = "touch_dpad" },
      { dir = "left", label = "左", x = 1076, y = 585, size = 64, art = "touch_dpad" },
      { dir = "right", label = "右", x = 1244, y = 585, size = 64, art = "touch_dpad" },
      { key = "confirm", label = "确定", x = 1160, y = 585, size = 80, art = "touch_confirm" },
    },
  },
  -- 关闭按钮（×）：只在面板模式显示，位置 = **当前面板矩形右上角内侧**，不写死——
  -- 由 virtual_pad 按 `rect.x + rect.w - dx, rect.y + dy` 推算（modal 面板与对话面板
  -- 矩形不同）。audit_touch 用 inventory（modal 代表）与 dialogue 两个位置做
  -- 越界 / 不压右栏文字 / 与 DPad 不重叠检查（quest/pause/shop 与 inventory 同矩形）。
  close = { dx = 56, dy = 64, size = 44, art = "touch_close" },
  -- 按钮配色（语义 → RGB；绘制端只读这里，不再各自硬编码）。
  --
  -- 这一层现在只是**底衬圆盘**的颜色（贴图缺失时的兜底 + 透过半透明漆面的一层底色），
  -- 所以一律取 docs/35 色板压暗一档：`ui.text` 在两个宿主都固定白色（web-runtime.js
  -- drawUi 里 `fillStyle = '#ffffff'`，没有颜色参数），底衬一旦偏亮，"交互/攻击"就糊。
  -- 切图上的漆圈色（竹青/朱砂/靛青/紫/云灰）在 `gen_ui_art.py` 的 touch 表里，两处
  -- 是同一语义的两种介质，不需要逐值相等，但**别把这里改亮**。
  button_color = {
    primary = { 0.30, 0.42, 0.25 },   -- 交互 / DPad 确定（竹青）
    combat = { 0.55, 0.22, 0.16 },    -- 攻击（朱砂）
    panel = { 0.24, 0.35, 0.52 },     -- 背包（靛青）
    log = { 0.42, 0.33, 0.55 },       -- 任务（紫）
    system = { 0.24, 0.27, 0.30 },    -- 暂停 / DPad 方向键 / 关闭（墨）
    action = { 0.42, 0.31, 0.23 },    -- 动作盘九键（木作棕 = gen_ui_art.py 的 WOOD_MID）
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
  hud_info = M.PANEL.hud_info,                          -- 状态面板（980,8 300×180，6 行）
  hud_bars = { x = 992, y = 194, w = 160, h = 72 },    -- 三条资源条（18px 行 × 3）
  hud_skill_bar = { x = 992, y = 296, w = 256, h = 100 }, -- 招式快捷栏（8 张 48 卡，每行 5 张共 2 行）
  hud_goals = { x = 16, y = 8, w = 760, h = 26 },       -- 七日目标行（左上角，ROW.goals_y）
  hud_feedback = { x = 16, y = 36, w = 520, h = 24 },     -- 动作反馈行（左上角，ROW.feedback_y）
  -- 新手引导行（底部通栏，ROW.tutorial_y）：与撤下的键位通栏同一位置——
  -- 这条是"当前该做什么"，只在引导十步内出现，之后整行不再绘制。
  hud_tutorial = { x = 16, y = 660, w = 664, h = 28 },
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

-- 门面 / HUD 左上角的文字行（标题门面与玩法 HUD 共用同一套顶部坐标）。
M.ROW = {
  top_x = 16,           -- 顶部说明行（标题门面）
  top_y = 8,
  goals_y = 8,           -- 七日目标行（左上角，贴顶边）
  feedback_y = 36,      -- 当前动作反馈行（目标行之下 28px）
  -- 新手引导行（屏幕底部通栏，字号 16 → 占 666..686）。放这里而不是右栏：
  -- 右栏行宽预算只有 276px，而引导文案最长一条实测 648px（`⑧ 交付：攒 5 株灵芝…`
  -- 加"引导 N/10："前缀，16px），塞进右栏必然出现省略号截断——指令被裁掉一半，
  -- 玩家照着按不出下一步。通栏预算 `row_width()`=1232px，十条引导全部整行放得下。
  tutorial_y = 666,
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

-- 顶部通栏行的**实际**预算：整屏宽会在右栏撞上资源条/招式栏（992..1252），
-- 而引导行最长 648px 用不到那一段。右界取状态面板左侧 14px，留得住最长文案
-- 又绝不与右栏元素叠字（`jhyt_m30_cjk_text` 逐条量 `clipped` 时按这个口径判）。
function M.top_row_width()
  return M.PANEL.hud_info.x - M.ROW.top_x - 14
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

-- 返回 (是否通过, 问题列表)。检查（对应文件头 `M.TOUCH` 的三条承诺 + 切图两条）：
--   ① 每个控件（摇杆底圆 + 外圈、每个按钮的视觉圆）是设计画布内的**整圆**，
--      `cx - r >= 0`、`cx + r <= DESIGN_W`、y 同理 —— 越界等于被窗口切掉；
--   ② 控件两两不重叠（相交即误触）；
--   ③ 控件不与 `M.TOUCH_KEEP_CLEAR` 里右栏文字/资源条/键位提示的包围盒重叠；
--   ④ 每个控件的 `art` 键在 `ui/skin.lua` 注册表里真实存在（拼错=静默不画）；
--   ⑤ 摇杆轨道孔半径与杆头行程自洽（杆头不得压到轨道切图）。
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
  -- 动作盘（M34-06 批 2）：`cols × rows` 的格子 + `items` 一一对应。格子里放不进更多项，
  -- 少一项就留一个空盘——所以 ⑦ 要求两边**数量相等**，而不是"至少能装下"。
  -- 归在 `group = "gameplay"`：它与摇杆、按钮簇、招式栏**同屏**，因此 ② 会真查它与
  -- 那 5 颗键和摇杆外圈是否重叠（右缘列 934 + 34 = 968 < 按钮簇包围盒左缘 1030）。
  local action = touch.action
  local action_slots = 0
  if type(action) == "table" then
    local a_size = tonumber(action.size) or 0
    local cols = action.cols or {}
    local rows = action.rows or {}
    action_slots = #cols * #rows
    for r = 1, #rows do
      for c = 1, #cols do
        local item = action.items and action.items[(r - 1) * #cols + c]
        local name = string.format(
          "touch.action[%d](%s)", (r - 1) * #cols + c,
          tostring(item and item.action or "empty")
        )
        if a_size <= 0 then
          issues[#issues + 1] = "layout.TOUCH.action.size 必须为正（当前 "
            .. tostring(action.size) .. "）：半径由 size/2 推导"
        end
        circles[#circles + 1] = {
          name = name, cx = cols[c], cy = rows[r], r = a_size / 2, group = "gameplay",
        }
      end
    end
  end
  -- 面板动作条（M34-06 批 3）：每个模式用同一张格阵的前几格，同一时刻只会出现一个模式。仍归 `group =
  -- "panel"`（要和十字键、关闭键真查重叠），另带一个 `mode` 标签：② 里"两边都有 mode
  -- 且不相等"才豁免 —— 背包/商店/暂停的同一格位置不算互触。
  local panel_actions = touch.panel_actions
  local panel_slots = 0
  if type(panel_actions) == "table" then
    local p_cols = panel_actions.cols or {}
    local p_rows = panel_actions.rows or {}
    panel_slots = #p_cols * #p_rows
    local p_size = tonumber(panel_actions.size) or 0
    if p_size <= 0 then
      issues[#issues + 1] = "layout.TOUCH.panel_actions.size 必须为正（当前 "
        .. tostring(panel_actions.size) .. "）：半径由 size/2 推导"
    end
    local mode_names = {}
    for name in pairs(panel_actions.modes or {}) do
      mode_names[#mode_names + 1] = name
    end
    table.sort(mode_names)
    for _, mode in ipairs(mode_names) do
      local index = 0
      for row, cy in ipairs(p_rows) do
        for col, cx in ipairs(p_cols) do
          index = index + 1
          local item = (panel_actions.modes or {})[mode][index]
          if item then
            circles[#circles + 1] = {
              name = string.format("touch.panel_actions.%s[%d](%s)", mode, index, tostring(item.action)),
              cx = cx, cy = cy, r = p_size / 2, group = "panel", mode = mode,
            }
          end
        end
      end
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
      elseif a.mode and b.mode and a.mode ~= b.mode then
        -- 面板动作条的同一格位置轮流给不同面板用（背包/商店/暂停），跨模式不同屏。
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

  -- ④ 每个控件的切图键必须在 `ui/skin.lua` 的注册表里真实存在。
  --    为什么值得守：`skin.icon` 对未知 key 是**静默 no-op**（它只能什么都不画），
  --    于是"改了个拼写"的表现是屏上只剩底衬圆盘、没有任何报错——正是这一族
  --    "画了但看不见"的老路。
  -- ⑤ 摇杆轨道/杆头的几何契约（`hole_r`、`knob_travel` 与 gen_ui_art.py 同源）。
  local art_keys = {}
  if type(stick) == "table" then
    art_keys[#art_keys + 1] = { name = "touch.stick.art", key = stick.art }
    art_keys[#art_keys + 1] = { name = "touch.stick.knob_art", key = stick.knob_art }
    local hole = tonumber(stick.hole_r) or 0
    local travel = tonumber(stick.knob_travel) or 0
    local outer = (tonumber(stick.base_r) or 0) + 4
    if not (hole > 0 and hole < outer) then
      issues[#issues + 1] = string.format(
        "layout.TOUCH.stick.hole_r=%.0f 必须在 0 与视觉外圈 %.0f 之间：轨道（切图里的漆圈）会退化或被切掉",
        hole, outer
      )
    end
    if travel > hole then
      issues[#issues + 1] = string.format(
        "layout.TOUCH.stick.knob_travel=%.0f 大于孔半径 hole_r=%.0f：杆头会压到轨道切图，"
        .. "而两张 ui.image 没有绘制顺序保证（会被随机埋掉一半）",
        travel, hole
      )
    end
  end
  for index, btn in ipairs(touch.buttons or {}) do
    art_keys[#art_keys + 1] = {
      name = string.format("touch.buttons[%d](%s).art", index, tostring(btn.label or btn.key)),
      key = btn.art,
    }
  end
  for index, key in ipairs((touch.dpad or {}).keys or {}) do
    art_keys[#art_keys + 1] = {
      name = string.format("touch.dpad.keys[%d](%s).art", index, tostring(key.dir or key.key or key.label)),
      key = key.art,
    }
  end
  if type(close) == "table" then
    art_keys[#art_keys + 1] = { name = "touch.close.art", key = close.art }
  end
  if type(action) == "table" then
    art_keys[#art_keys + 1] = { name = "touch.action.art", key = action.art }
  end
  if type(touch.more) == "table" then
    art_keys[#art_keys + 1] = { name = "touch.more.art", key = touch.more.art }
  end
  if type(panel_actions) == "table" then
    art_keys[#art_keys + 1] = { name = "touch.panel_actions.art", key = panel_actions.art }
  end
  local ok_skin, skin = pcall(require, "ui.skin")
  if ok_skin and type(skin) == "table" and type(skin.tex) == "table" then
    for _, entry in ipairs(art_keys) do
      if type(entry.key) ~= "string" or skin.tex[entry.key] == nil then
        issues[#issues + 1] = "layout." .. entry.name .. " 未在 ui/skin.lua 注册（"
          .. tostring(entry.key) .. "）：`skin.icon` 会静默 no-op，盘就没了"
      end
    end
  end

  -- ⑥ 「更多」键是**矩形**，上面三条只审圆，所以这里单独要求它整块落在招式栏的
  --    包围盒里：这张键借的就是招式栏第二行的空格子，飘出格子就等于盖住右栏文字。
  local more = touch.more
  if type(more) ~= "table" then
    issues[#issues + 1] = "layout.TOUCH.more 必须是 { x, y, size, label, art } 表"
  else
    local bar = (M.TOUCH_KEEP_CLEAR or {}).hud_skill_bar
    local size = tonumber(more.size) or 0
    if type(bar) ~= "table" then
      issues[#issues + 1] = "layout.TOUCH_KEEP_CLEAR.hud_skill_bar 缺失：⑥ 无从对照"
    elseif more.x < bar.x or more.y < bar.y
        or more.x + size > bar.x + bar.w or more.y + size > bar.y + bar.h then
      issues[#issues + 1] = string.format(
        "layout.TOUCH.more (%.0f,%.0f %.0fx%.0f) 不在招式栏包围盒 (%.0f,%.0f %.0fx%.0f) 内："
        .. "它会盖到右栏别处，而圆的三条检查都不认矩形",
        more.x or 0, more.y or 0, size, size, bar.x, bar.y, bar.w, bar.h
      )
    end
  end
  -- ⑦ 动作盘：`items` 与 `cols × rows` 必须一一对应（多一项没有格子、少一项空一格盘）。
  if type(action) == "table" then
    local count = #(action.items or {})
    if count ~= action_slots then
      issues[#issues + 1] = string.format(
        "layout.TOUCH.action 有 %d 格（cols×rows）但 %d 项：一一对应才没有空盘或漏项",
        action_slots, count
      )
    end
    local seen = {}
    for index, item in ipairs(action.items or {}) do
      local key = tostring(item.action)
      if key == "nil" or item.label == nil then
        issues[#issues + 1] = string.format(
          "layout.TOUCH.action.items[%d] 必须同时有 action 与 label", index)
      elseif seen[key] then
        issues[#issues + 1] = "layout.TOUCH.action.items 里动作 " .. key .. " 出现两次"
      end
      seen[key] = true
    end
  end

  -- ⑧ 面板动作条：每个模式的条目必须**装得下**（超过 `rows` 就没有格子，静默丢掉一项
  --    = 那条键鼠动作在触屏上又没有入口，正是这一批要消掉的状态）。
  if type(panel_actions) ~= "table" then
    issues[#issues + 1] = "layout.TOUCH.panel_actions 必须是 { art, size, cols, rows, modes } 表"
  else
    local mode_names = {}
    for name in pairs(panel_actions.modes or {}) do
      mode_names[#mode_names + 1] = name
    end
    table.sort(mode_names)
    if #mode_names == 0 then
      issues[#issues + 1] = "layout.TOUCH.panel_actions.modes 是空的：面板态没有任何动作入口"
    end
    for _, mode in ipairs(mode_names) do
      local count = #(panel_actions.modes[mode] or {})
      if count > panel_slots then
        issues[#issues + 1] = string.format(
          "layout.TOUCH.panel_actions.%s 有 %d 项但只有 %d 格（cols×rows）：多的那几项画不出来（静默丢动作）",
          mode, count, panel_slots
        )
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
