-- skin.lua：UI 换肤贴图注册表与绘制辅助（docs/48 ui 接线，69 张）。
--
-- 设计（对齐 assets/art/ui/manifest.json）：
--   * 全部贴图路径在此集中登记（m29 接线事实：每张都被引用）；面板按
--     `skin.panel(id, x, y, w, h, key)` / `skin.icon(id, x, y, w, h, key)`
--     绘制，key 查本表。9-slice 贴图的边距（像素）随表携带，直接喂
--     `ui.image` 第 10 参（引擎 L5994 扩展：`slice`）。
--   * 69 张 = assets/art/ui/ 全部 78 张 − 2 张开发预览（_preview*）− 6 张
--     已接线（ui_bar_fill_hp/neili/stamina、ui_bar_frame_9s、ui_weather_rain/sun）
--     − 1 张明确不接（ui_weather_cloud 已改名 `_ui_weather_cloud.png` 归开发产物：
--     天气取值闭集只有 sunny|rainy，接一个永远取不到的图标属于自欺，docs/48 §2）。
--   * 每个 key 的九宫格边距来自 manifest.json `nine_slice`（[left,right,top,bottom]）。
local M = {}

local DIR = "assets/art/ui/"

-- key → { path, slice=nil|"l,r,t,b" }。slice 非 nil 表示 9-slice（边距像素）。
M.tex = {
  -- common 共用控件（22 张）
  panel_main = { path = DIR .. "common/ui_panel_main_9s.png", slice = "16,16,16,16" },
  panel_paper = { path = DIR .. "common/ui_panel_paper_9s.png", slice = "16,16,16,16" },
  panel_confirm = { path = DIR .. "common/ui_panel_confirm_9s.png", slice = "12,12,12,12" },
  btn_primary = { path = DIR .. "common/ui_btn_primary_9s.png", slice = "8,8,8,8" },
  btn_primary_pressed = { path = DIR .. "common/ui_btn_primary_pressed_9s.png", slice = "8,8,8,8" },
  btn_secondary = { path = DIR .. "common/ui_btn_secondary_9s.png", slice = "8,8,8,8" },
  btn_danger = { path = DIR .. "common/ui_btn_danger_9s.png", slice = "8,8,8,8" },
  slot = { path = DIR .. "common/ui_slot_9s.png", slice = "6,6,6,6" },
  slot_selected = { path = DIR .. "common/ui_slot_selected_9s.png", slice = "6,6,6,6" },
  input = { path = DIR .. "common/ui_input_9s.png", slice = "6,6,6,6" },
  tab_on = { path = DIR .. "common/ui_tab_on_9s.png", slice = "6,6,6,6" },
  tab_off = { path = DIR .. "common/ui_tab_off_9s.png", slice = "6,6,6,6" },
  key_hint = { path = DIR .. "common/ui_key_hint_9s.png", slice = "6,6,6,6" },
  checkbox_off = { path = DIR .. "common/ui_checkbox_off.png" },
  checkbox_on = { path = DIR .. "common/ui_checkbox_on.png" },
  slider_track = { path = DIR .. "common/ui_slider_track.png" },
  slider_handle = { path = DIR .. "common/ui_slider_handle.png" },
  dropdown_arrow = { path = DIR .. "common/ui_dropdown_arrow.png" },
  scroll_arrow_up = { path = DIR .. "common/ui_scroll_arrow_up.png" },
  scroll_arrow_down = { path = DIR .. "common/ui_scroll_arrow_down.png" },
  btn_close = { path = DIR .. "common/ui_btn_close.png" },
  btn_close_hover = { path = DIR .. "common/ui_btn_close_hover.png" },
  divider = { path = DIR .. "common/ui_divider.png" },
  icon_seal = { path = DIR .. "common/ui_icon_seal.png" },
  -- menu 主入口门面（14 张）
  menu_vignette = { path = DIR .. "menu/ui_menu_vignette.png" },
  menu_ink_band = { path = DIR .. "menu/ui_menu_ink_band.png" },
  menu_btn_wuxia = { path = DIR .. "menu/ui_menu_btn_wuxia_9s.png", slice = "18,18,18,18" },
  menu_btn_wuxia_hover = { path = DIR .. "menu/ui_menu_btn_wuxia_hover_9s.png", slice = "18,18,18,18" },
  menu_btn_wuxia_pressed = { path = DIR .. "menu/ui_menu_btn_wuxia_pressed_9s.png", slice = "18,18,18,18" },
  menu_panel_scroll = { path = DIR .. "menu/ui_menu_panel_scroll_9s.png", slice = "20,20,20,20" },
  menu_ornament_corner = { path = DIR .. "menu/ui_menu_ornament_corner.png" },
  menu_ornament_corner_tr = { path = DIR .. "menu/ui_menu_ornament_corner_tr.png" },
  menu_ornament_corner_bl = { path = DIR .. "menu/ui_menu_ornament_corner_bl.png" },
  menu_ornament_corner_br = { path = DIR .. "menu/ui_menu_ornament_corner_br.png" },
  menu_divider_bamboo = { path = DIR .. "menu/ui_menu_divider_bamboo.png" },
  menu_lantern = { path = DIR .. "menu/ui_menu_lantern.png" },
  menu_seal_jianghu = { path = DIR .. "menu/ui_menu_seal_jianghu.png" },
  -- hud 主界面（13 张；条 fill/frame 与晴/雨图标已单独接线）
  hud_icon_hp = { path = DIR .. "hud/ui_icon_hp.png" },
  hud_icon_stamina = { path = DIR .. "hud/ui_icon_stamina.png" },
  hud_icon_neili = { path = DIR .. "hud/ui_icon_neili.png" },
  hud_icon_coin = { path = DIR .. "hud/ui_icon_coin.png" },
  hud_status_poison = { path = DIR .. "hud/ui_status_poison.png" },
  hud_status_buff = { path = DIR .. "hud/ui_status_buff.png" },
  hud_status_bleed = { path = DIR .. "hud/ui_status_bleed.png" },
  hud_status_debuff = { path = DIR .. "hud/ui_status_debuff.png" },
  hud_status_sleep = { path = DIR .. "hud/ui_status_sleep.png" },
  hud_status_stealth = { path = DIR .. "hud/ui_status_stealth.png" },
  hud_status_weather = { path = DIR .. "hud/ui_status_weather.png" },
  hud_status_festival = { path = DIR .. "hud/ui_status_festival.png" },
  hud_avatar_frame = { path = DIR .. "hud/ui_hud_avatar_frame.png" },
  -- backpack 品质框（5 张）
  quality_frame_0 = { path = DIR .. "backpack/ui_quality_frame_0.png" },
  quality_frame_1 = { path = DIR .. "backpack/ui_quality_frame_1.png" },
  quality_frame_2 = { path = DIR .. "backpack/ui_quality_frame_2.png" },
  quality_frame_3 = { path = DIR .. "backpack/ui_quality_frame_3.png" },
  quality_frame_4 = { path = DIR .. "backpack/ui_quality_frame_4.png" },
  -- dialogue 对话（3 张）
  dialogue_continue = { path = DIR .. "dialogue/ui_dialogue_continue.png" },
  dialogue_marker = { path = DIR .. "dialogue/ui_dialogue_marker.png" },
  portrait_frame = { path = DIR .. "dialogue/ui_portrait_frame.png" },
  -- shop 商店（2 张）
  shop_step_minus = { path = DIR .. "shop/ui_btn_step_minus.png" },
  shop_step_plus = { path = DIR .. "shop/ui_btn_step_plus.png" },
  -- quest 任务（2 张）
  quest_bullet_todo = { path = DIR .. "quest/ui_quest_bullet_todo.png" },
  quest_bullet_done = { path = DIR .. "quest/ui_quest_bullet_done.png" },
  -- skills 武学（6 张）
  skill_card = { path = DIR .. "skills/ui_skill_card_9s.png", slice = "8,8,8,8" },
  skill_basic_blade = { path = DIR .. "skills/ui_skill_basic_blade.png" },
  skill_chase_wind = { path = DIR .. "skills/ui_skill_chase_wind.png" },
  skill_guard_heart = { path = DIR .. "skills/ui_skill_guard_heart.png" },
  skill_return_palm = { path = DIR .. "skills/ui_skill_return_palm.png" },
  skill_breath_qi = { path = DIR .. "skills/ui_skill_breath_qi.png" },
  -- build 建筑放置（2 张）
  build_ghost_ok = { path = DIR .. "build/ui_build_ghost_ok.png" },
  build_ghost_bad = { path = DIR .. "build/ui_build_ghost_bad.png" },
  -- save 存档（2 张）
  save_slot = { path = DIR .. "save/ui_save_slot_9s.png", slice = "8,8,8,8" },
  save_shot_placeholder = { path = DIR .. "save/ui_save_shot_placeholder.png" },
  -- settings 设置（2 张）
  toggle_on = { path = DIR .. "settings/ui_toggle_on_9s.png", slice = "8,8,8,8" },
  toggle_off = { path = DIR .. "settings/ui_toggle_off_9s.png", slice = "8,8,8,8" },
  -- logo 标题（1 张）
  title_logo = { path = DIR .. "logo/ui_title_logo.png" }
}

-- 画 9-slice 面板/控件：id 唯一；key 必须带 slice（普通贴图请用 icon）。
function M.panel(id, x, y, w, h, key)
  local t = M.tex[key]
  if not t or not t.slice then
    return
  end
  ui.image(id, x, y, w, h, t.path, 1.0, 1.0, 1.0, t.slice)
end

-- 画普通图标/贴图：key 无 slice 直接画（有 slice 也按原尺寸平铺——用 panel 更合适）。
function M.icon(id, x, y, w, h, key)
  local t = M.tex[key]
  if not t then
    return
  end
  ui.image(id, x, y, w, h, t.path, 1.0, 1.0, 1.0)
end

-- 全部注册键（validate_data 登记用；每键 path 必须真实指向 assets/art/ui/ 下的文件）。
function M.all()
  local out = {}
  for _, entry in pairs(M.tex) do
    if type(entry) == "table" and type(entry.path) == "string" then
      out[#out + 1] = entry.path
    end
  end
  return out
end

-- 校验（M23 登记覆盖：带 validate_all 的模块必须在 validate_data 登记）。
function M.validate_all()
  local issues = {}
  local seen = {}
  for key, entry in pairs(M.tex) do
    if type(entry) ~= "table" or type(entry.path) ~= "string" then
      issues[#issues + 1] = 'ui/skin.lua: ' .. tostring(key) .. '.path 缺失'
    elseif not entry.path:find('^assets/art/ui/.*%.png$') then
      issues[#issues + 1] = 'ui/skin.lua: ' .. tostring(key) .. ' 路径不在 assets/art/ui/：' .. tostring(entry.path)
    elseif seen[entry.path] then
      issues[#issues + 1] = 'ui/skin.lua: 重复路径 ' .. tostring(entry.path)
    else
      seen[entry.path] = true
    end
    if entry.slice and (type(entry.slice) ~= "string" or not entry.slice:match('^%d+,%d+,%d+,%d+$')) then
      issues[#issues + 1] = 'ui/skin.lua: ' .. tostring(key) .. '.slice 格式非法：' .. tostring(entry.slice)
    end
  end
  return #issues == 0, issues
end

return M
