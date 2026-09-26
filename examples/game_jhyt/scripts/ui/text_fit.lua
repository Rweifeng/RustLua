-- ui/text_fit.lua：按**真实字宽**裁剪与换行（M30 / 阶段五 P2）。
--
-- 为什么需要它：中文排版不能按"字数 × 系数"估宽。同一串字里 CJK 是满宽（1.0em）、
-- 拉丁字母约半宽（0.5em）、`W`/`M` 更宽、全角标点又不同，还有 `ui.text_at` 的
-- 字间距（`TextStyle::default().spacing = 1.0`，每个字符后加 1px）。
-- 此前 HUD 用的是"CJK 记 2 单位、其余记 1 单位"的启发式（`hud.clip`），
-- 遇到长数字（`金币 99999 文`）、宽拉丁串或中英混排仍会溢出面板。
--
-- 引擎能力：`ui.measure_text(text, size)` 返回**与渲染同口径**的
-- `{width, height, lines, glyphs, missing}`（同字体、同 advance、同字间距，
-- 见 `core::text::measure_text`，单测 `measure_matches_layout_size` 钉住一致性）。
-- 本模块只做"够不够宽 / 怎么断"的策略，不做数学近似。
--
-- 口径与保证：
--   * `M.width(text, size)` == `ui.measure_text(text, size).width`（同一条求和路径，
--     验收样例逐条对比两者）；
--   * `M.fit(...)` 结果宽度 **≤ max_width**（省略号也计入）；
--   * `M.wrap(...)` 每一行宽度 **≤ max_width**；标点禁则用"与前一字成组"实现
--     （不会出现标点落在行首，也不会因此超宽）。
local M = {}

-- 宽度比较容差（px）。为什么需要：引擎按 f32 累加整串推进宽度（`ui.measure_text`），
-- 本模块按字符逐个取宽度再用 Lua 的 f64 求和，两条路径在几百像素量级上会有
-- ~1e-5 px 的浮点差异（f32 在 592px 处的精度约 6e-5）。0.05px（1/20 像素）远高于
-- 这种噪声、又远低于肉眼可见，因此用来判定"放得下/放不下"。
M.EPSILON = 0.05

local ELLIPSIS = "…"

-- 行首禁则：这些字符不允许出现在行首，必须与前一字符同组（中文排版惯例）。
local NO_LINE_START = {
  ["，"] = true,
  ["。"] = true,
  ["、"] = true,
  ["；"] = true,
  ["："] = true,
  ["？"] = true,
  ["！"] = true,
  ["）"] = true,
  ["」"] = true,
  ["』"] = true,
  ["》"] = true,
  ["〉"] = true,
  ["】"] = true,
  ["…"] = true,
  ["·"] = true,
  [","] = true,
  ["."] = true,
  ["!"] = true,
  ["?"] = true,
  [":"] = true,
  [";"] = true,
  [")"] = true,
  ["]"] = true,
  ["}"] = true,
}

-- 缓存：`字符 @ 字号 → 宽度`、`整串 @ 字号 → 宽度`。文案是常量/数据，缓存命中率极高。
-- `reset()` 在开新档与热重载时清空（字号不会变，但重载后引擎状态可能重建）。
local char_cache = {}
local text_cache = {}

function M.reset()
  char_cache = {}
  text_cache = {}
  return true
end

local function key(text, size)
  return tostring(size) .. "\0" .. text
end

--- 单个字符的推进宽度（px，含字间距）：拆到字符级才能"按宽度断行"。
function M.char_width(char, size)
  local k = key(char, size)
  local cached = char_cache[k]
  if cached then
    return cached
  end
  local measured = ui.measure_text(char, size)
  local width = measured.width
  char_cache[k] = width
  return width
end

--- 整串宽度（px）：等于逐字符宽度之和（换行不计宽，与渲染一致）。
function M.width(text, size)
  local s = tostring(text or "")
  local k = key(s, size)
  local cached = text_cache[k]
  if cached then
    return cached
  end
  local total = 0
  for _, code in utf8.codes(s) do
    local char = utf8.char(code)
    if char ~= "\n" then
      total = total + M.char_width(char, size)
    end
  end
  text_cache[k] = total
  return total
end

--- 是否放得下（边界算放得下：`width == max_width` 不算溢出）。
function M.fits(text, size, max_width)
  return M.width(text, size) <= max_width + M.EPSILON
end

--- 截断到 `max_width` 内，超出部分换成 `…`（省略号宽度计入预算）。
--- 整串本来放得下就原样返回（不做任何改动）。
function M.fit(text, size, max_width)
  local s = tostring(text or "")
  if max_width <= 0 then
    return ""
  end
  if M.fits(s, size, max_width) then
    return s
  end
  local budget = max_width - M.char_width(ELLIPSIS, size)
  if budget <= 0 then
    return ""
  end
  local out = {}
  local used = 0
  for _, code in utf8.codes(s) do
    local char = utf8.char(code)
    local width = M.char_width(char, size)
    if used + width > budget then
      break
    end
    out[#out + 1] = char
    used = used + width
  end
  return table.concat(out) .. ELLIPSIS
end

--- 按宽度断行：CJK 逐字可断、连续 ASCII 视为一个词（不拆单词）、
--- 行首禁则字符与前一字成组。返回行表（至少一行）。
--- `max_lines` 给了就限制行数；超过时丢弃其余部分，且最后一行**以省略号收尾**
--- （"还有下文"必须在画面上看得出来）。
function M.wrap(text, size, max_width, max_lines)
  local s = tostring(text or "")
  if max_width <= 0 then
    return { "" }
  end
  -- 1) 分组：把"不可拆"的片段合成 token。
  --    * 空格并入当前词（词内分隔）：「J 攻击」「1/2/3/4/5 直选」这类带空格短标签
  --      整体换行，不会被断成两半（jhyt_m20_gamepad 面板提示断言）；
  --    * 行首禁则标点（，。· 等）贴到当前词尾并**结束当前词**（标点是词边界）：
  --      「W/S 或 方向键 选择 ·」是一个 token，标点不落行首；
  --    * 无空格的长中文按标点分句成 token；单 token 超宽由第 2 步硬断兜底。
  local tokens = {}
  local word = nil            -- 当前词缓冲（内部可含空格）
  local function flush()
    if word and word ~= "" then
      tokens[#tokens + 1] = word
    end
    word = nil
  end
  for _, code in utf8.codes(s) do
    local char = utf8.char(code)
    if char == "\n" then
      flush()
      tokens[#tokens + 1] = "\n"
    elseif char == " " then
      -- 空格并入当前词（无当前词时成为下一个词的词首空格）。
      word = (word or "") .. " "
    elseif char:match("^[%w%._%-%+%%/]$") or not NO_LINE_START[char] then
      -- ASCII / CJK / 其他普通字符：并入当前词。
      word = (word or "") .. char
    else
      -- 行首禁则标点：贴到当前词尾并结束当前词。
      if word then
        word = word .. char
        flush()
      elseif #tokens > 0 and tokens[#tokens] ~= "\n" then
        tokens[#tokens] = tokens[#tokens] .. char
      else
        tokens[#tokens + 1] = char
      end
    end
  end
  flush()

  -- 2) 贪心装行：token 整体放不下就换行；单个 token 比整行还宽时按字符硬断。
  local lines = {}
  local current = ""
  local current_width = 0
  local function push(line)
    lines[#lines + 1] = line
  end
  for _, token in ipairs(tokens) do
    if token == "\n" then
      push(current)
      current = ""
      current_width = 0
    else
      local token_width = M.width(token, size)
      if current_width > 0 and current_width + token_width > max_width + M.EPSILON then
        -- 行尾不留空格。
        push(current:gsub("%s+$", ""))
        current = token == " " and "" or token
        current_width = token == " " and 0 or token_width
      else
        current = current .. token
        current_width = current_width + token_width
      end
      -- 单个 token 超宽（例如一整串没有空格的英文/长数字）：按字符硬断。
      while current_width > max_width + M.EPSILON and utf8.len(current) and utf8.len(current) > 1 do
        local piece = {}
        local used = 0
        local cut = 0
        for _, code in utf8.codes(current) do
          local char = utf8.char(code)
          local width = M.char_width(char, size)
          if used + width > max_width + M.EPSILON then
            break
          end
          piece[#piece + 1] = char
          used = used + width
          cut = cut + 1
        end
        if cut == 0 then
          break
        end
        push(table.concat(piece))
        local rest = {}
        local index = 0
        for _, code in utf8.codes(current) do
          index = index + 1
          if index > cut then
            rest[#rest + 1] = utf8.char(code)
          end
        end
        current = table.concat(rest)
        current_width = M.width(current, size)
      end
    end
  end
  push(current:gsub("%s+$", ""))

  if max_lines and max_lines > 0 and #lines > max_lines then
    local kept = {}
    for index = 1, max_lines do
      kept[index] = lines[index]
    end
    -- 因为行数上限而截断时，最后一行**必须以省略号收尾**：读者要看得出"还有下文"
    -- （不放省略号的话，被截掉的半句话看起来就是完整文案）。
    local last = kept[max_lines]
    if last:sub(-#ELLIPSIS) == ELLIPSIS then
      last = last:sub(1, #last - #ELLIPSIS)
    end
    kept[max_lines] = M.fit(last .. ELLIPSIS, size, max_width)
    return kept
  end
  return lines
end

--- 行表里最宽的一行（验收/调试用）。
function M.max_line_width(lines, size)
  local widest = 0
  for _, line in ipairs(lines) do
    widest = math.max(widest, M.width(line, size))
  end
  return widest
end

--- 单字符宽度表（诊断用：看某串为什么这么宽）。
function M.char_widths(text, size)
  local out = {}
  for _, code in utf8.codes(tostring(text or "")) do
    local char = utf8.char(code)
    out[#out + 1] = { char = char, width = M.char_width(char, size) }
  end
  return out
end

return M
