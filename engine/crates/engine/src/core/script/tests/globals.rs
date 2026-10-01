use super::*;

#[test]
fn event_bus_subscribes_emits_and_unsubscribes() {
    let path = script(
        "function on_create()\n\
         local received = {}\n\
         local cb = function(d) table.insert(received, d) end\n\
         local cb2 = function(d) table.insert(received, d * 10) end\n\
         event.on('ItemAdded', cb)\n\
         event.subscribe('ItemAdded', cb2)\n\
         event.emit('ItemAdded', 5)\n\
         assert(#received == 2)\n\
         assert(received[1] == 5 and received[2] == 50)\n\
         event.off('ItemAdded', cb)\n\
         received = {}\n\
         event.emit('ItemAdded', 7)\n\
         assert(#received == 1 and received[1] == 70)\n\
         event.unsubscribe('ItemAdded')\n\
         received = {}\n\
         event.emit('ItemAdded', 9)\n\
         assert(#received == 0)\n\
         event.emit('AttackHit')\n\
         local nested = 0\n\
         event.on('Outer', function() nested = nested + 1; event.emit('Inner') end)\n\
         event.on('Inner', function() nested = nested + 1 end)\n\
         event.emit('Outer')\n\
         assert(nested == 2)\n\
         return 'ok' end\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
#[test]
fn calendar_and_day_lifecycle_lua_api() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "local now = calendar.now()\n\
         assert(now.absolute_day == 1 and now.day == 1 and now.season == 1 and now.year == 1)\n\
         calendar.set(1, 2, 1, 60)\n\
         local spring2 = calendar.now()\n\
         assert(spring2.season == 2 and spring2.day == 1 and spring2.absolute_day == 29)\n\
         ended = 0\n\
         started = 0\n\
         function on_day_end() ended = ended + 1 end\n\
         function on_day_start() started = started + 1 end\n\
         local d = day.sleep()\n\
         assert(d == 30 and ended == 1 and started == 1)\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
