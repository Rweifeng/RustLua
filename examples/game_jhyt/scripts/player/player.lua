local state = require("core.state")
local characters = require("data.characters")
local appearances = require("data.appearances")

local M = {}

-- 场景原生玩家实体（JianghuPlayer）：由 manor.scene 用
-- Transform + Sprite + Collider(ball;radius=12) 声明，引擎实例化时自动生成
-- 动态刚体。跨场景 persistent=true，同名实体会在每次进入山庄时被场景本体
-- 覆盖重建（引擎 process_scene_transition 语义），因此这里始终用 entity.find
-- 重新定位，不缓存运行时 id。
function M.ensure()
  local pid = entity.find("JianghuPlayer")
  if not pid then
    return nil
  end
  state.player = pid
  return pid
end

-- 直接更新刚体速度（世界单位/秒）：引擎每帧 step_physics 推进位移，
-- 与 GroundMap solid 瓦片碰撞；0 表示停住。
-- 顺带记录"是否在走"与朝向，供主角形象系统切 walk/idle 与四向条带
-- （不额外读物理速度，Lua 侧本就持有指令来源，避免多一个引擎 API 依赖）。
function M.set_velocity(dx, dy)
  local pid = M.ensure()
  if not pid then
    return
  end
  physics.set_velocity(pid, dx, dy)
  local moving = (tonumber(dx) or 0) ~= 0 or (tonumber(dy) or 0) ~= 0
  M.moving = moving
  if moving then
    -- state.facing 是玩法口径（up/down/left/right，farm/combat 也在用）；
    -- 条带是四向 s/n/e/w，二者在这里一次性换算。
    M.facing = M.clip_dir()
  end
end

-- 停止移动（松开方向键时调用，避免惯性滑行）
function M.stop()
  local pid = M.ensure()
  if pid then
    physics.set_velocity(pid, 0, 0)
  end
  M.moving = false
end

-- 传送：把动态刚体与 ECS Transform 同步移到世界坐标 (x,y) 并清零速度。
-- 切图落点 / 读档恢复必须用它（transform.set 只改 ECS，下一帧 step_physics
-- 会把刚体旧位置写回覆盖）；physics.set_position 为 M17-01 新增的 2D 传送。
function M.teleport(x, y)
  local pid = M.ensure()
  if not pid then
    return false
  end
  if type(x) ~= "number" or type(y) ~= "number" then
    return false
  end
  physics.set_position(pid, x, y)
  physics.set_velocity(pid, 0, 0)
  return true
end

-- 查询位置（farm.act / 面向格计算用）
function M.position()
  local pid = M.ensure()
  if not pid then
    return nil
  end
  return transform.get(pid)
end

-- 位置快照使用场景中的稳定实体名 JianghuPlayer 重新绑定；引擎当前未暴露
-- PersistId 查询/设置 API，因此这是最小安全坐标快照：记录所在地图 id 与
-- 世界坐标，跨地图读档/继续时可定位目标场景。
function M.capture()
  local pos = M.position()
  if not pos then
    return { mode = "unavailable", entity = "JianghuPlayer" }
  end
  return {
    mode = "entity_name_position",
    entity = "JianghuPlayer",
    map = (map.current() or scene.reload()),
    x = tonumber(pos.x),
    y = tonumber(pos.y)
  }
end

function M.restore(data)
  if type(data) ~= "table" or data.mode ~= "entity_name_position" then
    return false, "player position snapshot unavailable"
  end
  local pid = M.ensure()
  if not pid then
    return false, "JianghuPlayer is not instantiated"
  end
  if type(data.x) ~= "number" or type(data.y) ~= "number" then
    return false, "player position is invalid"
  end
  -- 读档恢复发生在场景重建之后：用 2D 传送保证刚体与 ECS 同步，
  -- 避免 transform.set 被下一帧 physics 回写覆盖导致"回到旧位置"。
  M.teleport(data.x, data.y)
  return true
end

-- ===========================================================================
-- 主角形象系统
--
-- 数据：形象清单 data/appearances.lua（可选哪些、中文名、解锁条件）
--      形象本体 data/characters.lua（单帧 static_<dir> + Animation 组件串）
-- 状态：state.appearance（存档持久化，随 "state" provider 落盘）
-- 挂载：sprite.set(单帧) + animation.attach(序列帧)，两者必须同一实体
--
-- 为什么在运行期挂而不是只在 .scene 里声明：
--   1. 形象要能**切换**（暂停菜单/易容玩法），场景里写死就换不了；
--   2. 引擎 `animation.attach`（M21 新增）让运行期挂载成为可能，
--      NPC/敌人/主角三处共用同一条路径；
--   3. 场景里仍然声明了默认形象（编辑器打开就能看到真人），
--      运行期 apply() 负责"跟随 state.appearance 纠正"。
-- ===========================================================================

-- 玩法朝向（input/combat/farm 用的 up/down/left/right）→ 条带四向（s/n/e/w）。
-- 世界坐标 y 向上，所以 up → n（背对）、down → s（面向镜头）。
local DIRECTION_TO_CLIP = { up = "n", down = "s", left = "w", right = "e" }

function M.clip_dir(facing)
  local name = facing or state.facing or "down"
  return DIRECTION_TO_CLIP[name] or "s"
end

-- 当前形象 id：state 里的值必须真实存在，否则退回默认形象（坏档自愈）。
function M.appearance_id()
  local id = state.appearance
  if type(id) == "string" and appearances.get(id) and characters.get(id) then
    return id
  end
  local fallback = appearances.default_id()
  state.appearance = fallback
  return fallback
end

-- 当前形象条目（display/note/static_sprite/animation/clips）。
function M.appearance_entry()
  return characters.get(M.appearance_id())
end

-- 可选形象列表（UI 用）：id/中文名/说明/是否默认。
function M.appearance_options()
  return appearances.list()
end

-- 切换形象：校验 → 写 state → 立刻重挂美术。返回 ok, err。
-- 已删除/未知的 id 一律拒绝，避免把玩家变成白块。
function M.set_appearance(id)
  if type(id) ~= "string" or id == "" then
    return false, "appearance id must be a non-empty string"
  end
  if not appearances.get(id) then
    return false, "unknown appearance: " .. id
  end
  if not characters.get(id) then
    return false, "appearance has no character art: " .. id
  end
  if state.appearance == id and M.attached_key then
    return true
  end
  state.appearance = id
  M.apply()
  return true
end

-- 把"当前形象 + 当前场景"挂到 JianghuPlayer 上（幂等）。
-- 单帧先设，保证即使动画组件挂载失败也看得见人；attach 成功后立即切 idle_<dir>。
function M.apply()
  local pid = M.ensure()
  if not pid then
    return false, "JianghuPlayer is not instantiated"
  end
  local id = M.appearance_id()
  local entry = characters.get(id)
  if not entry then
    return false, "no character art for appearance: " .. tostring(id)
  end
  M.facing = M.clip_dir()
  local sprite_ok, sprite_err = pcall(sprite.set, pid, entry.static_sprite[M.facing])
  if not sprite_ok then
    return false, "sprite.set failed: " .. tostring(sprite_err)
  end
  local attached = pcall(animation.attach, pid, entry.animation)
  M.attached = attached == true
  M.attached_key = tostring(scene.reload()) .. "|" .. tostring(pid) .. "|" .. id
  if M.attached then
    pcall(animation.set, pid, (M.oneshot or "idle") .. "_" .. M.facing)
  end
  return true
end

-- 播放一次性动作（attack / hurt / faint / hoe / water）：
-- 条带是非循环的，播完由 tick() 切回 walk/idle；同名动作正在播时不打断。
-- 形象没有该动作（例如木人只有 idle/hurt）时静默返回 false。
function M.play(action)
  if type(action) ~= "string" or action == "" then
    return false
  end
  if M.oneshot == action then
    return false
  end
  local pid = M.ensure()
  if not pid or not M.attached then
    return false
  end
  local clip = action .. "_" .. (M.facing or "s")
  local entry = M.appearance_entry()
  if entry and entry.clips then
    local has = false
    for _, item in ipairs(entry.clips) do
      if item.name == clip then
        has = true
        break
      end
    end
    if not has then
      return false
    end
  end
  if pcall(animation.set, pid, clip) then
    M.oneshot = action
    return true
  end
  return false
end

-- 每帧驱动：
--   * 场景/形象变了 → 重挂（切图后实体重建，组件会丢）；
--   * 一次性动作播完（animation.ended）→ 清标记，落回 walk/idle；
--   * 否则按"是否在走"切 walk_<dir> / idle_<dir>（同状态时引擎返回 false，开销可忽略）。
function M.tick()
  local pid = M.ensure()
  if not pid then
    M.attached_key = nil
    return
  end
  local key = tostring(scene.reload()) .. "|" .. tostring(pid) .. "|" .. tostring(state.appearance)
  if key ~= M.attached_key then
    M.apply()
  end
  if not M.attached then
    return
  end
  if M.oneshot then
    if pcall(animation.ended, pid) then
      M.oneshot = nil
    else
      return
    end
  end
  local clip = (M.moving and "walk_" or "idle_") .. (M.facing or M.clip_dir())
  pcall(animation.set, pid, clip)
end

-- 诊断/验收用：当前形象、朝向、动画状态与是否已挂序列帧。
function M.status()
  local pid = M.ensure()
  local clip = nil
  if pid and M.attached then
    local ok, value = pcall(animation.get, pid)
    clip = ok and value or nil
  end
  return {
    appearance = M.appearance_id(),
    display = (M.appearance_entry() or {}).display,
    facing = M.facing or M.clip_dir(),
    moving = M.moving == true,
    oneshot = M.oneshot,
    animated = M.attached == true,
    clip = clip
  }
end

return M
