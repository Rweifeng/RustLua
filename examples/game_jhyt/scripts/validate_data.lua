-- validate_data.lua：领域数据统一校验入口（M20-01）。
--
-- 单一事实来源：启动（main.lua 的 validate_content）、发布前检查
-- （`game --check` / `editor --check`）与编辑器资源扫描共用本模块，
-- 保证三处诊断结果一致，且规则只写一次。
--
-- 契约：
--   local report = require("validate_data").check()
--   * report == ""               数据全部通过；
--   * report ~= ""               以换行分隔的问题列表，每条含来源模块与
--                                文件/对象/字段路径（由各模块 validate_all 给出）。
--   * check({ strict = true })   额外做「模块可加载性」检查（require 失败也报错）。
local M = {}

-- 校验器清单：{ 诊断前缀, 校验模块 }。新增领域数据校验只在此处登记。
-- M19-02 追加 "tutorial"（data/tutorial.lua 的引导步骤表）。
-- M21 追加 "appearances"（主角形象清单；会交叉校验 data/characters.lua 的 role=player）。
local function validators()
  return {
    { "maps", require("data.maps") },
    { "places", require("data.places") },
    { "npcs", require("data.npcs") },
    { "characters", require("data.characters") },
    { "appearances", require("data.appearances") },
    { "schedule", require("world.schedule") },
    { "quest", require("world.quest") },
    { "dialogue", require("world.dialogue") },
    { "shop", require("systems.shop") },
    { "skills", require("data.skills") },
    { "enemies", require("data.enemies") },
    { "relationships", require("data.relationships") },
    { "buildings", require("data.buildings") },
    { "recipes", require("data.recipes") },
    { "tutorial", require("data.tutorial") }
  }
end

-- 返回问题列表（已排序稳定：按前缀）；无问题时为空表。
function M.issues(options)
  options = options or {}
  local issues = {}
  for _, entry in ipairs(validators()) do
    local name = entry[1]
    local module = entry[2]
    if type(module) ~= "table" or type(module.validate_all) ~= "function" then
      if options.strict then
        issues[#issues + 1] = "[" .. name .. "] 缺少 validate_all 校验入口"
      end
    else
      local ok, valid, list = pcall(module.validate_all)
      if not ok then
        issues[#issues + 1] = "[" .. name .. "] 校验执行失败: " .. tostring(valid)
      elseif valid ~= true then
        for _, issue in ipairs(list or {}) do
          issues[#issues + 1] = "[" .. name .. "] " .. tostring(issue)
        end
      end
    end
  end
  table.sort(issues)
  return issues
end

-- 契约入口：返回 ""（通过）或换行分隔的问题列表。
function M.check(options)
  local issues = M.issues(options)
  if #issues == 0 then
    return ""
  end
  return table.concat(issues, "\n")
end

return M
