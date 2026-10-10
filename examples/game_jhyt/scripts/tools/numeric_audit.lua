-- Numeric regression shared by the engine M53 example and tools/audit_numeric.py.
-- Exercises actual inventory/meridian/input modules; restores persistent state.
local M = {}
function M.check_recovery()
  local s = require('core.state')
  local bag = require('player.inventory')
  local m = require('systems.meridian')
  local saved_state, saved_bag = s.capture(), bag.capture()
  local saved_meridians = m.capture()
  local saved_stats = require('player.stats').capture()
  local pressed = input.action_pressed
  local function eq(actual, expected, label)
    assert(actual == expected, label .. ': ' .. tostring(actual) .. ' ~= ' .. tostring(expected))
  end
  local ok, err = pcall(function()
    local function stock(id)
      local n = inv.count(s.inventory, id)
      if n > 0 then inv.remove(s.inventory, id, n) end
      inv.add(s.inventory, id, 2)
    end
    local function use(id, energy, inner, want_energy, want_inner, consumed)
      stock(id)
      s.energy, s.inner_power = energy, inner
      local used = bag.use(id)
      eq(used, consumed, id .. ' used')
      eq(s.energy, want_energy, id .. ' energy')
      eq(s.inner_power, want_inner, id .. ' inner')
      eq(inv.count(s.inventory, id), consumed and 1 or 2, id .. ' count')
    end
    m.reset()
    use('heal_pill', 80, 50, 100, 50, true)
    use('heal_pill', 100, 50, 100, 50, false)
    m.restore({opened={'ren','du','xinshaoyin'}})
    eq(m.max_energy(), 110, 'energy cap')
    eq(m.max_inner_power(), 110, 'inner cap')
    use('heal_pill', 105, 50, 110, 50, true)
    use('heal_pill', 110, 50, 110, 50, false)
    use('qi_pill', 110, 105, 110, 110, true)
    use('qi_pill', 110, 110, 110, 110, false)
    use('qi_pill', 105, 110, 110, 110, true)
    s.inner_power = 5
    eq(m.on_day_start(), 30, 'daily recovery amount')
    eq(s.inner_power, 35, 'daily is not full restore')
    s.inner_power = 105
    eq(m.on_day_start(), 5, 'daily cap')
    s.set_mode('Playing')
    input.action_pressed = function(action) return action == 'practice' end
    for _, pair in ipairs({{103,108},{108,110},{110,110}}) do
      s.inner_power = pair[1]
      require('ui.input').update()
      eq(s.inner_power, pair[2], 'practice dynamic cap')
    end
    m.reset()
    s.inner_power = 98
    require('ui.input').update()
    eq(s.inner_power, 100, 'practice base cap')
  end)
  input.action_pressed = pressed
  m.restore(saved_meridians)
  require('player.stats').restore(saved_stats)
  bag.restore(saved_bag)
  s.restore(saved_state)
  assert(ok, err)
  return 'numeric_recovery_ok cases=13'
end
return M
