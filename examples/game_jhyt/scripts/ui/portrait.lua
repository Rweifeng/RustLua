-- portrait.lua：立绘贴图（docs/48 portraits 接线，21 张）。
--
-- 设计：
--   * 7 角色（a_qi / gu_changfeng / shen_qingluo / situ_kui / zhao_tiesuanpan
--     + 主角两个形象 player_male / player_female）
--     × 3 表情（calm 平静 / angry 生气 / surprised 惊讶）= 21 张成品立绘，
--     全部列在下方映射表里（m29 接线事实：每张都被引用）。
--   * 对话节点可带 `mood = "angry"|"surprised"`（缺省/非法回退 calm）；
--     dialogue.lua 按 speaker + 节点 mood 调 texture_for 画到面板上方。
--   * 没有立绘的角色（如 npc_lin_xiaoman）返回 nil，对话面板不画（纯文字照旧）。
--   * 主角那三张不进对话（说话人永远是 NPC）：`ui/hud.lua` 按 `state.appearance` 取
--     calm 那张，填右上角 48×48 的头像框（docs/45 欠账 ②）。
local M = {}

-- npc persist_id（主角是 appearance id）→ 立绘文件名前缀（assets/art/portraits/ 目录）。
local PREFIX = {
  npc_a_qi = "a_qi",
  npc_gu_changfeng = "gu_changfeng",
  npc_shen_qingluo = "shen_qingluo",
  npc_situ_kui = "situ_kui",
  npc_zhao_tiesuanpan = "zhao_tiesuanpan",
  player_male = "player_male",
  player_female = "player_female"
}

local MOODS = { calm = true, angry = true, surprised = true }

-- 21 张全部以**字面路径**入表（m29 按"源码里出现完整文件名"判定引用；
-- 变量拼接的 `DIR .. prefix .. mood .. ".png"` 在语料里没有完整串，会漏判）。
local SHEETS = {
  "assets/art/portraits/a_qi_calm.png",
  "assets/art/portraits/a_qi_angry.png",
  "assets/art/portraits/a_qi_surprised.png",
  "assets/art/portraits/gu_changfeng_calm.png",
  "assets/art/portraits/gu_changfeng_angry.png",
  "assets/art/portraits/gu_changfeng_surprised.png",
  "assets/art/portraits/shen_qingluo_calm.png",
  "assets/art/portraits/shen_qingluo_angry.png",
  "assets/art/portraits/shen_qingluo_surprised.png",
  "assets/art/portraits/situ_kui_calm.png",
  "assets/art/portraits/situ_kui_angry.png",
  "assets/art/portraits/situ_kui_surprised.png",
  "assets/art/portraits/zhao_tiesuanpan_calm.png",
  "assets/art/portraits/zhao_tiesuanpan_angry.png",
  "assets/art/portraits/zhao_tiesuanpan_surprised.png",
  -- 主角（HUD 头像框用 calm；另两档表情先出图，等"表情随状态变"那批再接）。
  "assets/art/portraits/player_male_calm.png",
  "assets/art/portraits/player_male_angry.png",
  "assets/art/portraits/player_male_surprised.png",
  "assets/art/portraits/player_female_calm.png",
  "assets/art/portraits/player_female_angry.png",
  "assets/art/portraits/player_female_surprised.png",
}
-- 查询键：<npc 前缀>_<mood> → 字面路径。
local BY_KEY = {}
for _, path in ipairs(SHEETS) do
  local name = path:match("([^/]+)%.png$")
  if name then
    BY_KEY[name] = path
  end
end

-- 节点 mood → 合法表情名（缺失/非法回退 calm）。
local function mood_name(node)
  local mood = type(node) == "table" and node.mood or nil
  if mood and MOODS[mood] then
    return mood
  end
  return "calm"
end

-- 按 npc persist_id（主角传 `state.appearance`）+ 节点返回立绘贴图路径；无立绘的角色返回 nil。
function M.texture_for(npc_id, node)
  local prefix = PREFIX[npc_id]
  if not prefix then
    return nil
  end
  return BY_KEY[prefix .. "_" .. mood_name(node)]
end

-- 立绘显示尺寸（原图 128×128，面板上显示 96×96）。
function M.display_size()
  return 96, 96
end

-- 全部立绘路径（validate_data 登记用；数量 = 21）。
function M.all()
  local out = {}
  for _, path in ipairs(SHEETS) do
    out[#out + 1] = path
  end
  return out
end

-- 校验（M23 登记覆盖：带 validate_all 的模块必须在 validate_data 登记）。
function M.validate_all()
  local issues = {}
  if #SHEETS ~= 21 then
    issues[#issues + 1] = 'ui/portrait.lua: SHEETS 数量应为 21，实际 ' .. tostring(#SHEETS)
  end
  local seen = {}
  for _, path in ipairs(SHEETS) do
    if type(path) ~= 'string' or not path:find('^assets/art/portraits/.*%.png$') then
      issues[#issues + 1] = 'ui/portrait.lua: 非法立绘路径 ' .. tostring(path)
    end
    if seen[path] then
      issues[#issues + 1] = 'ui/portrait.lua: 重复路径 ' .. tostring(path)
    end
    seen[path] = true
  end
  for id, prefix in pairs(PREFIX) do
    if type(prefix) ~= 'string' or prefix == '' then
      issues[#issues + 1] = 'ui/portrait.lua: PREFIX[' .. tostring(id) .. '] 非法'
    end
  end
  return #issues == 0, issues
end

return M
