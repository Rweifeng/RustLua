-- effects.lua：短生命周期特效实体（docs/48 effects 接线，8 条带）。
--
-- 设计（对齐 assets/art/effects/manifest.json）：
--   * 引擎能力：`entity.spawn` 只插 Transform；运行期创建的实体要用
--     `animation.attach(id, component_str)` 自己挂动画组件（core/script.rs
--     L7940 注释：NPC/敌人/特效只能显示单帧，attach 让 Lua 侧挂组件）。
--   * 每条带一张 1xN 横向条带贴图：cols 帧、fps 播放速度、looped 是否循环；
--     贴图与帧数全部来自 manifest.json 的 `animation` 字段，逐条对齐。
--   * 生命周期：一次性特效 ttl = cols/fps 秒，播完由 M.update 自毁
--     （entity.destroy）；循环特效（poison_motes 毒雾）默认 ttl 1 秒，
--     调用方可传 ttl_override 控制常驻时长。
--   * 渲染层：sorting_layer=actor + order=50，盖在角色/敌人之上。
--   * 可查询：M.active() 返回当前活动特效数（headless 断言用）。
local M = {}

-- kind → 条带定义（与 assets/art/effects/manifest.json 完全一致）
local FX = {
  hoe_dust = {
    texture = "assets/art/effects/hoe_dust.png",
    cols = 6, fps = 12, looped = false, w = 32, h = 32
  },
  water_splash = {
    texture = "assets/art/effects/water_splash.png",
    cols = 6, fps = 12, looped = false, w = 32, h = 32
  },
  blade_arc = {
    texture = "assets/art/effects/blade_arc.png",
    cols = 5, fps = 18, looped = false, w = 48, h = 48
  },
  hit_spark = {
    texture = "assets/art/effects/hit_spark.png",
    cols = 5, fps = 18, looped = false, w = 32, h = 32
  },
  hurt_flash = {
    texture = "assets/art/effects/hurt_flash.png",
    cols = 3, fps = 20, looped = false, w = 32, h = 32
  },
  heal_motes = {
    texture = "assets/art/effects/heal_motes.png",
    cols = 6, fps = 8, looped = false, w = 32, h = 32
  },
  poison_motes = {
    texture = "assets/art/effects/poison_motes.png",
    cols = 6, fps = 6, looped = true, w = 32, h = 32
  },
  dash_streak = {
    texture = "assets/art/effects/dash_streak.png",
    cols = 4, fps = 16, looped = false, w = 32, h = 32
  }
}

local active = {} -- id → { ttl }（键即特效实体 id，值存剩余秒数）

-- animation component_str（与 manifest 的 animation 字段同格式）
local function clip_str(kind, fx)
  local loop = fx.looped and "true" or "false"
  return "default=" .. kind .. ";clip:" .. kind .. ":" .. fx.texture
    .. ":" .. fx.cols .. ":" .. fx.fps .. ":" .. loop
end

-- 在 (x, y) 生成一个 kind 特效实体（x,y 为世界坐标，特效矩形以该点为中心）。
-- ttl_override：循环条带默认 1 秒；非循环条带忽略（按 cols/fps 播完自毁）。
function M.spawn(kind, x, y, ttl_override)
  local fx = FX[kind]
  if not fx or type(x) ~= "number" or type(y) ~= "number" then
    return nil
  end
  local id = entity.spawn(nil, nil)
  if not id then
    return nil
  end
  transform.set(id, x - fx.w / 2, y - fx.h / 2)
  sprite.set(
    id,
    "texture=" .. fx.texture
      .. ";size=" .. fx.w .. "x" .. fx.h
      .. ";pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000"
      .. ";sorting_layer=actor;order=50"
  )
  animation.attach(id, clip_str(kind, fx))
  local ttl
  if fx.looped then
    ttl = tonumber(ttl_override) or 1.0
  else
    ttl = fx.cols / fx.fps
  end
  active[id] = { ttl = ttl }
  return id
end

-- 销毁指定特效实体（循环条带提前结束/切图清理用）。
function M.despawn(id)
  if not id then
    return
  end
  if active[id] then
    if entity.is_alive(id) then
      entity.destroy(id)
    end
    active[id] = nil
  end
end

-- 每帧推进：一次性特效播完自毁。
function M.update(dt)
  for id, entry in pairs(active) do
    if entry.ttl > 0 then
      entry.ttl = entry.ttl - dt
      if entry.ttl <= 0 then
        if entity.is_alive(id) then
          entity.destroy(id)
        end
        active[id] = nil
      end
    end
  end
end

-- 当前活动特效数量（headless 断言：触发点生效 = 特效确实生成了）。
function M.active()
  local n = 0
  for _ in pairs(active) do
    n = n + 1
  end
  return n
end

-- 切图/重开清理：全部特效实体销毁。
function M.clear()
  for id in pairs(active) do
    if entity.is_alive(id) then
      entity.destroy(id)
    end
    active[id] = nil
  end
end

return M
