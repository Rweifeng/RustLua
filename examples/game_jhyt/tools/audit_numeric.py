"""Run actual Lua numeric rules without compiling Rust; requires Python + lupa.
Engine integration coverage: cargo run -p game --example jhyt_m53_meridian.
The Lua recovery assertions are shared; engine/UI adapters below are test doubles.
"""
from pathlib import Path
try:
    from lupa.lua54 import LuaRuntime
except ImportError:
    raise SystemExit('Requires lupa with Lua 5.4 (python -m pip install lupa).')

ROOT = Path(__file__).resolve().parents[1]
lua = LuaRuntime(unpack_returned_tuples=True)
lua.globals().scripts_path = (ROOT / 'scripts').as_posix()
lua.execute("package.path = scripts_path .. '/?.lua;' .. package.path")
lua.execute(r'''
local noop = function() end
item = {define=noop}
local slots, equipment = {}, {}
inv = {
  create=function() slots={} end, exists=function() return true end,
  capacity=function() return 20 end,
  count=function(_, id) local n=0; for _, s in pairs(slots) do if s.item==id then n=n+s.count end end; return n end,
  add=function(_,id,n) for i=1,20 do if not slots[i] or slots[i].item==id then slots[i]={item=id,count=(slots[i] and slots[i].count or 0)+n}; return 0 end end; return n end,
  remove=function(_,id,n) for i,s in pairs(slots) do if s.item==id then s.count=s.count-n; if s.count<=0 then slots[i]=nil end; return true end end; return false end,
  get=function() return {capacity=20,slots=slots} end,
  set=function(_,i,id,n) slots[i]=id and {item=id,count=n} or nil end,
  equip=function(_,slot,id) equipment[slot]=id end,
  equip_list=function() return equipment end
}
input={action_pressed=function() return false end,action=function() return false end,action_axis=function() return 0 end}
-- Deliberately stub only irrelevant rendering/input consumers; retain real state,
-- inventory, data tables, stats, mastery, meridian and ui.input control flow.
for _,name in ipairs({'player.player','systems.interaction','world.dialogue','systems.shop',
 'systems.shipping','systems.building','player.relationship','core.audio','ui.hud',
 'ui.tutorial','systems.goals','ui.inventory_ui','ui.quest_ui','ui.pause_menu','ui.virtual_pad','ui.hints'}) do
  package.loaded[name]=setmetatable({}, {__index=function() return noop end})
end
package.loaded['systems.combat']={is_player_dead=function() return false end,player_can_act=function() return true end,player_guard=noop}
require('player.inventory').setup()
''')
print(lua.eval("require('tools.numeric_audit').check_recovery()"))
print('crop,first_profit_per_day,regrow_gross_per_day,27_settlements_net,first_energy,regrow_energy')
rows = lua.execute(r'''
local crops,items=require('data.crops'),require('data.items')
local rows={}
for id,c in pairs(crops) do if not c.tree then
 local gross=c.yield*items[c.harvest_item].sell
 local cost=items[c.seed_item].buy
 local first=(gross-cost)/c.growth_days
 assert(first>=3 and first<=15,id..' first profit outside existing gate')
 local r=c.regrow_days
 local n=r and (1+math.floor((27-c.growth_days)/r)) or math.floor(27/c.growth_days)
 local net=r and (n*gross-cost) or n*(gross-cost)
 rows[#rows+1]=string.format('%s,%.4f,%s,%d,%d,%s',id,first,r and string.format('%.4f',gross/r) or '-',net,8+3+4*c.growth_days+2,r and tostring(4*r+2) or '-')
end end
table.sort(rows)
return table.concat(rows,'\n')
''')
print(rows)
print('numeric_audit_ok crops=24; unit adapters != full engine simulation')
