-- ui/ui_text.lua：**运行时文字的唯一出口**（M30 / 阶段五 P2）。
--
-- 为什么要有这一层：
--   1. 中文排版必须按引擎真实字宽来（`ui.measure_text`），不能按"字数 × 系数"估；
--   2. 排完还得能**被检查**——"这条文字有没有超出它所在的面板"是可机器验收的不变量，
--      但容器的可用宽度只有各面板自己知道。于是绘制时把
--      `(id, 文字, 字号, 可用宽度)` 一起登记下来，验收样例遍历登记表逐条量宽，
--      不必在验收里重抄一遍各处面板几何（抄一遍就会漂）。
--
-- 规则（由 `jhyt_m30_cjk_text` 静态扫描守住）：`scripts/**` 里**不允许**直接调
-- `ui.text_at`，一律走本模块，否则"画了但没人登记"的文字会从排版检查里漏掉
-- ——那正是"零报错的坏行为"（溢出看不见）。
--
-- `owner`（第一个参数）是"绘制者"名字：每个模块每次重绘先 `begin(owner)`，
-- 只清掉自己的旧登记；HUD 每帧重绘不会把面板的登记一起清空。
local text_fit = require("ui.text_fit")

local M = {}

-- owner → { id → { text, size, budget, x, y } }
local records = {}

--- 开始一轮绘制：清掉该 owner 的旧登记（同一 owner 的未重绘元素不该留到下一轮）。
function M.begin(owner)
  records[owner] = {}
  return true
end

--- 某个 owner 是否已经开始绘制。
function M.has(owner)
  return records[owner] ~= nil
end

--- 登记表（验收/调试只读遍历）：返回 owner → 记录表。
function M.recorded()
  return records
end

--- 全部登记条数（验收用计数器）。
function M.count()
  local total = 0
  for _, owned in pairs(records) do
    for _ in pairs(owned) do
      total = total + 1
    end
  end
  return total
end

--- 隐藏并清除某条登记（未使用的行/选项必须清，否则旧文字会被当成"还在画"）。
function M.hide(owner, id)
  ui.hide(id)
  local owned = records[owner]
  if owned then
    owned[id] = nil
  end
  return true
end

--- 一次性隐藏多个 id。
function M.hide_all(owner, ids)
  for _, id in ipairs(ids) do
    M.hide(owner, id)
  end
  return true
end

--- 清空（可选只清某个 owner）：开新档、切场景、关闭面板时调用。
function M.reset(owner)
  if owner then
    records[owner] = nil
  else
    records = {}
  end
  return true
end

--- 单行文字：按真实字宽裁剪到 `budget` 内再绘制（返回实际画出的文字）。
--- 同时登记**原始**文案（`raw`）：`audit()` 靠 `raw != text` 认出"画面上是省略号"。
--- 只登记裁剪后的那一份时，超宽文案永远量得过预算——门就是这么漏牙的（M33）。
function M.draw(owner, id, x, y, size, budget, text)
  local raw = tostring(text or "")
  local fitted = text_fit.fit(raw, size, budget)
  ui.text_at(id, x, y, fitted, 1.0, size)
  records[owner][id] = { text = fitted, raw = raw, size = size, budget = budget, x = x, y = y }
  return fitted
end

--- 多行文字（按宽度换行，最多 `max_lines` 行，超出用省略号）：对话正文用。
--- 返回画出的行表。
function M.draw_wrapped(owner, id, x, y, size, budget, max_lines, text)
  local lines = text_fit.wrap(text, size, budget, max_lines)
  local joined = table.concat(lines, "\n")
  ui.text_at(id, x, y, joined, 1.0, size)
  records[owner][id] = { text = joined, size = size, budget = budget, x = x, y = y }
  return lines
end

--- 不裁剪、只登记（固定短文案：仍会被量宽检查，超了说明布局真的错了）。
function M.draw_exact(owner, id, x, y, size, budget, text)
  local value = tostring(text or "")
  ui.text_at(id, x, y, value, 1.0, size)
  records[owner][id] = { text = value, size = size, budget = budget, x = x, y = y }
  return value
end

--- 逐条量宽：返回 `{ checked, bad = {...}, clipped = {...}, worst }`。验收与运行期自检共用同一实现。
--- 判定：① 字体缺字 ② 字号 < 11px（`docs/42` §1 的可读性下限，随验收一起检查）
--- ③ 实际宽度 > 登记的可用宽度（多行文字逐行量）。
--- ④ `clipped`：**登记时就被裁过**的条目（原始文案宽于预算，画面上是省略号）。
---    它不并进 `bad`，因为"内容长度不可控"的详情类文案（物品描述、对话正文）本来就
---    靠裁剪收在框里；门要守的是**定长文案不许裁**（HUD 行、面板行），那由验收点名断言。
function M.audit()
  local checked = 0
  local bad = {}
  local clipped = {}
  local worst = 0
  for owner, owned in pairs(records) do
    for id, rec in pairs(owned) do
      checked = checked + 1
      if rec.size < 11 then
        bad[#bad + 1] = owner .. "/" .. id .. ":font=" .. tostring(rec.size)
      end
      if rec.raw ~= nil and rec.raw ~= rec.text then
        clipped[#clipped + 1] = string.format("%s/%s:%s", owner, id, rec.raw)
      end
      for line in tostring(rec.text):gmatch("[^\n]+") do
        local measured = ui.measure_text(line, rec.size)
        if measured.missing ~= "" then
          bad[#bad + 1] = owner .. "/" .. id .. ":missing=" .. measured.missing
        end
        local slack = measured.width - rec.budget
        if slack > worst then
          worst = slack
        end
        -- 容差 0.05px：引擎整串度量与 Lua 逐字符求和的 f32/f64 差异（见 text_fit.EPSILON）。
        if slack > 0.05 then
          bad[#bad + 1] = string.format(
            "%s/%s:overflow=%.1f>%.1f(%s)",
            owner,
            id,
            measured.width,
            rec.budget,
            line
          )
        end
      end
    end
  end
  table.sort(bad)
  table.sort(clipped)
  return { checked = checked, bad = bad, clipped = clipped, worst = worst }
end

return M
