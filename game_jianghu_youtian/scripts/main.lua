local state = require("state")
local player = require("player")
local inventory = require("inventory")
local input = require("input")

function on_create()
  inventory.setup()
  player.ensure()
  log.info("江湖有田工程启动：山庄尚待修复")
  return "jianghu_youtian_ready"
end

function update()
  return input.update()
end

function on_day_end()
  save.blob_write("jianghu_youtian.state", state)
end

function on_day_start()
  log.info("新的一天：" .. tostring(clock.now().day))
end

function on_migrate(from, to)
  log.info("存档迁移：" .. tostring(from) .. " -> " .. tostring(to))
end

return state
