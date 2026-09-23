-- camera_view.lua：2D 相机跟随玩家 + 取景范围锁在地图内（M32 竖版收口）。
--
-- 为什么需要这个模块
-- ------------------
-- M32 把工程改成竖版 720×1280 之后，相机一直是**默认相机**：中心 `(640,360)`、
-- `zoom=1`、**没有 bounds**（`docs/52` §4 当时把它记为"有意留下的偏差"）。表现是：
--   * 角色走到地图左右两端也看不出跟随（相机中心固定，整幅世界摊在屏幕上）；
--   * 竖屏比地图高（视口 1280 世界单位 vs 地图 704），屏幕上下露出地图外的空白。
-- 本模块把这两件事一起收口：
--   1. **跟随**：`camera2d.follow(player)` 让引擎的相机控制器每帧把相机中心推到
--      玩家世界坐标（`Camera2dController::advance`，步长由宿主按 delta 夹取，
--      与帧率无关）。
--   2. **不出界**：相机边界由**场景**声明（每张玩法地图一个 `GameCamera` 实体，
--      `component <id> Camera projection=orthographic;...;bounds=0,0,W*32,H*32`）。
--      引擎每帧在渲染前用 `Camera2D::constrained_center` 把相机中心夹到
--      `[bounds.min + 半视口, bounds.max - 半视口]`，因此**可见矩形始终在地图矩形内**。
--
-- 为什么边界写在场景而不是这里
-- ----------------------------
-- 引擎**没有**给 Lua 暴露 `Camera.bounds` 的写接口（`core/script.rs` 的 `camera2d` 表只有
-- `set`/`set_zoom`/`follow`/`unfollow`/`shake`/`get`/`get_zoom`/`screen_to_world`/
-- `world_to_screen`/`mouse_world`），所以边界只能声明在场景组件串里。本模块的
-- `validate_all`（登记在 `scripts/validate_data.lua`，启动与 `game --check` 都会跑）
-- 反过来核对"场景声明的 bounds == 该场景 GroundMap 的真实地图矩形"。
--
-- 竖屏的边界事实（写清楚，别当成 bug）
-- ----------------------------------
-- 竖屏视口 720×1280，地图 40×22 格 = 1280×704 世界单位（室内/洞穴 20×12 = 640×384）：
--   * **x 轴**：视口(720) < 地图(1280) ⇒ 夹取范围 [360, 920] 有效，相机真的左右跟随；
--   * **y 轴**：视口(1280) > 地图(704) ⇒ 该轴上界(352-640)低于下界(352+640)，引擎按
--     "两条界的中点"取值（`camera.rs` 的 `clamp_center`），于是 y 被**钉在地图纵向
--     中心**而不是乱夹。屏幕上下因此仍有留白——地图比屏幕矮时这在几何上不可避免
--     （要彻底消掉只能放大视野 `zoom > 1.82`，`docs/52` §4 已裁定不缩放世界）。
--   * 一句话：**x 轴上"跟随 + 不出界"同时成立，y 轴上退化为"钉在地图中心"。**
local player = require("player.player")
local maps = require("data.maps")

local M = {}

-- 是否是玩法地图：只有这些场景有玩家实体与 `GameCamera`（门面 `main` 两者都没有）。
-- 与 `main.lua` 的 `is_playable` 同一口径（`data.maps` 注册表），不另立一份清单。
local function is_playable(scene_id)
  return type(scene_id) == "string" and maps.get(scene_id) ~= nil
end

-- 最近一次绑定的 (场景, 实体 id, 是否已跟随)，供诊断与验收（`M.status()`）。
local bound = { scene = nil, entity = nil, following = false }

-- 宿主是否**真的**实现了 `camera2d`。
--
-- 为什么不能只用 `pcall(camera2d.follow, ...)` 判定：WebGL / 微信小游戏运行时**没有**
-- `camera2d`，而它把"引擎里有、web 侧没实现"的宿主全局换成了会**自报家门**的代理
-- （可调用、可索引，见 `plugins/webgl/web-runtime.js` 的 `__p2_missing_proxy`）——
-- 也就是说 `pcall` **不会**失败，`camera2d.follow(...)` 会静默地"成功"，直到后来读
-- `camera2d.get().x` 才炸在离现场很远的地方（实测：微信真机形态点「开始新游戏」后
-- 启动失败在 `camera_view.lua` 取 `center.x` 那一行）。
--
-- 判据按优先级两条，缺一不可：
--   ① **先**查宿主自报的缺失全局清单。web / 微信运行时在启动时把"这一侧没实现的宿主
--      全局"整体算一遍，落在 `__p2_missing_globals`（`plugins/webgl/web-runtime.js` 的
--      `engine-host-compat` 片段）。在清单里 ⇒ 整个 `camera2d` 都是代理，直接判不可用。
--      这一步是**权威**判据，且**零噪声**（只查表，不碰代理）。
--      只用下面的形状探测会误判：宿主把缺失全局换成了可调用/可索引的代理，
--      `pcall(camera2d.get)` 会"成功"并返回一张真表，于是判成"可用"，`camera2d.follow`
--      继续静默生效（代理吞掉），直到某处读字段才炸——这正是本次线上问题的形态
--      （微信形态点「开始新游戏」后启动失败在取 `center.x` 那一行）。
--   ② 形状探测兜底（覆盖"表在、方法没实现"的宿主，以及不带这套兼容层的宿主）：
--      真实现返回 `{x=, y=}`，代理返回**空壳代理表**，所以用 `next(value)` 判"表里真的
--      有字段"。这一步只能走 `next`/`type` 这类**不查索引**的手段：读 `value.x`（或等价
--      的 `value["x"]`）会经过 `__index`，在宿主日志里留下一条 `camera2d.get.x` 的假缺口，
--      harness 会因为"未在白名单里的缺失 API"判红（实测踩到，两处都不能写字段访问）。
-- 探测结果缓存一次（能力是宿主属性，不会一帧一变）。
local camera_ready = nil
local function camera_available()
  if camera_ready ~= nil then
    return camera_ready
  end
  camera_ready = false
  local missing_globals = rawget(_G, "__p2_missing_globals")
  if type(missing_globals) == "table" and missing_globals["camera2d"] then
    return false
  end
  if type(camera2d) == "table" or type(camera2d) == "userdata" then
    local ok, value = pcall(camera2d.get)
    -- `next(value)` 只认"表里真的有字段"；空壳代理表自身没有字段（字段都在 `__index` 里）。
    camera_ready = ok and type(value) == "table" and next(value) ~= nil
  end
  return camera_ready
end

-- 让相机跟随玩家，玩家因此始终停在视野正中。
-- **每次进入玩法地图都要调用**：换图会重建场景实体（Lua VM 与相机控制器活着，但玩家
-- 是新 id），沿用旧 id 会让跟随目标指向已销毁的实体。
-- 返回 ok, reason；宿主没有 `camera2d` 时返回 false + "host_has_no_camera2d"（调用方
-- 不需要区别对待——相机是表现层，缺了不该拖垮玩法）。
function M.follow_player()
  local current = scene.reload()
  if not is_playable(current) then
    -- 门面：没有玩家可跟。顺手取消跟随，免得相机停在上一张图的坐标上。
    if camera_available() then
      pcall(camera2d.unfollow)
    end
    bound = { scene = current, entity = nil, following = false }
    return false, "not_a_playable_map"
  end
  if not camera_available() then
    -- WebGL / 微信小游戏：宿主没有相机 API（相机由适配层自己决定取景）。
    -- 这里**不报错也不记 warn**：它是已知的平台能力差异，每张图记一条只会刷日志。
    bound = { scene = current, entity = nil, following = false }
    return false, "host_has_no_camera2d"
  end
  local pid = player.ensure()
  if not pid then
    bound = { scene = current, entity = nil, following = false }
    return false, "JianghuPlayer is not instantiated"
  end
  -- 无 offset / 无死区 / 无平滑 = 玩家**始终在视野正中**：
  --   * 死区会让角色在框内游走时相机不动（"始终居中"就不成立）；
  --   * 平滑（smoothing > 0）会让相机持续落后玩家一段距离，同样不居中。
  -- 这三个值本来就是引擎默认（`Camera2dController::default`），显式传一遍是为了让
  -- "本工程的相机手感只有一个说法"——将来引擎改默认值也不会悄悄改掉本作的手感。
  local ok, err = pcall(camera2d.follow, pid, {
    offset_x = 0,
    offset_y = 0,
    dead_zone_x = 0,
    dead_zone_y = 0,
    smoothing = 0
  })
  bound = { scene = current, entity = pid, following = ok == true }
  if not ok then
    log.warn("[camera] follow 失败: " .. tostring(err))
    return false, tostring(err)
  end
  log.info("[camera] " .. tostring(current) .. " 跟随玩家 entity=" .. tostring(pid))
  return true
end

-- 清掉跟随状态（新档/回门面时用，避免拿上一局的玩家实体当目标）。
function M.reset()
  if camera_available() then
    pcall(camera2d.unfollow)
  end
  bound = { scene = nil, entity = nil, following = false }
end

-- 诊断/验收只读快照。`center` 是**宿主每帧回填的真实值**（`camera2d.get`），
-- 不是脚本自述——验收断言"相机真的跟着玩家"必须看它。
--
-- 门面（没有玩家实体）上 `player` 为 nil：验收要用 `following` 先判定，不能直接取字段。
-- 宿主没有 `camera2d`（WebGL / 微信小游戏）时 `center`/`zoom` 为 nil、`available=false`。
function M.status()
  -- 玩法地图之外（门面/未进图）不建玩家实体：`player.ensure()` 会返回 nil，
  -- 取 .x 会抛 Lua 错误，因此这里必须先判空。
  local has_player = scene.reload() ~= nil and maps.get(scene.reload()) ~= nil
  local available = camera_available()
  local center = { x = nil, y = nil }
  local zoom = nil
  if available then
    local ok, value = pcall(camera2d.get)
    -- 同上：只有确认是**真表**（`next` 非空）才逐字段读，避免在代理上留下假缺口。
    if ok and type(value) == "table" and next(value) ~= nil then
      center.x, center.y = tonumber(value["x"]), tonumber(value["y"])
    end
    local zoom_ok, zoom_value = pcall(camera2d.get_zoom)
    if zoom_ok then
      zoom = tonumber(zoom_value)
    end
  end
  local pos = has_player and player.position() or nil
  return {
    scene = bound.scene,
    entity = bound.entity,
    following = bound.following,
    -- 宿主能力：false 时上面三个字段没有意义（相机由平台自己决定取景）。
    available = available,
    center = center,
    zoom = zoom,
    player = pos and { x = tonumber(pos.x), y = tonumber(pos.y) } or nil
  }
end

-- ===========================================================================
-- 数据门：场景里的 `GameCamera` 契约
-- ===========================================================================
--
-- 为什么需要一条门：`bounds` 与地图尺寸的关系横跨三层——Python 生成器
-- （`tools/pixel_kit.camera_line`）→ `.scene` 数据 → 引擎相机夹取。任何一层漂移的
-- 表现都是"相机看起来怪，但没有任何报错"：例如把地图改成 40×30 却忘了重跑生成器，
-- 相机就会在地图下沿之前被提前夹住。
--
-- 读文件与 `ui/layout_check.lua` 同一口径：**宿主没有 `io` 时静默跳过**
-- （WebGL/微信小程序的 Lua 运行时不提供 `io`，把"读不到文件"当数据错误会让
-- web 产物启动即失败）。守卫点是发布前的 `game --check`（原生、有 `io`）。
-- 读工程内文件的**绝对路径**。
--
-- 为什么不能直接 `io.open("scenes/manor.scene")`：相对路径由**进程工作目录**解析，而
-- `game --check` 并不 chdir 到工程根（实测：在 `engine/` 下跑 `--check --project
-- ../examples/game_jhyt`，`io.open("scenes/manor.scene")` 返回 nil）⇒ 这门会**静默跳过**
-- 全部场景核对，仍然打绿灯——比"没有门"更糟，因为它看起来在工作。
--
-- 工程根从 `app.project_root()` 取（引擎暴露的绝对路径，正斜杠）。取不到（Web 等没有
-- 文件系统的宿主）就退回相对路径；两条都失败时返回 nil，调用方按"宿主读不到文件"跳过。
local function read_file(relative)
  if type(io) ~= "table" or type(io.open) ~= "function" then
    return nil
  end
  local root = ""
  if type(app) == "table" and type(app.project_root) == "function" then
    local ok, value = pcall(app.project_root)
    if ok and type(value) == "string" then
      root = value
    end
  end
  local candidates = {}
  if root ~= "" then
    candidates[#candidates + 1] = root .. "/" .. relative
  end
  candidates[#candidates + 1] = relative
  for _, path in ipairs(candidates) do
    local handle = io.open(path, "r")
    if handle then
      local text = handle:read("*a")
      handle:close()
      return text
    end
  end
  return nil
end

-- 场景里某个组件的载荷（`component\t<id>\t<Name>\t<payload>`）。
-- 返回载荷列表（一个场景理论上可以有多份，实际按调用方要求判定数量）。
local function component_payloads(text, name)
  local out = {}
  for line in text:gmatch("[^\r\n]+") do
    local fields = {}
    for field in (line .. "\t"):gmatch("([^\t]*)\t") do
      fields[#fields + 1] = field
    end
    if fields[1] == "component" and fields[3] == name and fields[4] then
      out[#out + 1] = fields[4]
    end
  end
  return out
end

-- `key=value` 的单个数值（缺项/非数返回 nil）。
--
-- 逐**字段**取而不是在全串上找 `width=`：组件串里 `tile_size=32x32;width=40` 中
-- `width=` 的前一个字符是 `;`，而 `tile_size=32x32` 里也含 `x32`；按字段切分后
-- "键相等"是确定的，不依赖前后字符的巧合。
local function payload_field(payload, key)
  for field in (payload .. ";"):gmatch("([^;]*);") do
    local name, value = field:match("^%s*([%w_]+)%s*=%s*(.*)$")
    if name == key then
      return (value:gsub("%s+$", ""))
    end
  end
  return nil
end

local function payload_number(payload, key)
  local raw = payload_field(payload, key)
  return raw and tonumber(raw) or nil
end

-- `bounds=min_x,min_y,max_x,max_y` → 四个数（缺项/非数返回 nil）。
local function payload_vec4(payload, key)
  local raw = payload_field(payload, key)
  if not raw then
    return nil
  end
  local out = {}
  for part in raw:gmatch("[^,]+") do
    local value = tonumber((part:gsub("%s", "")))
    if not value then
      return nil
    end
    out[#out + 1] = value
  end
  return #out == 4 and out or nil
end

-- 返回该场景真实的地图矩形 `(宽, 高)`（取第一份 Tilemap）。室外图的 `GroundMap` 与
-- 农田 overlay 尺寸相同、室内图只有一份 Tilemap，因此"第一份"是无歧义的口径。
-- **注意是两个返回值**，不是表（`local rect = map_rect(...)` 只会拿到宽）。
local function map_rect(text)
  local payload = component_payloads(text, "Tilemap")[1]
  if not payload then
    return nil
  end
  -- `tile_size=32x32`：两个方向同值（引擎的单元尺寸），取第一个数即可。
  local tile = tonumber((payload_field(payload, "tile_size") or ""):match("^(%d+)"))
  local width = payload_number(payload, "width")
  local height = payload_number(payload, "height")
  if not (tile and width and height) then
    return nil
  end
  return tile * width, tile * height
end

function M.validate_all()
  local issues = {}
  local prefix = "world.camera_view:"
  -- 只有玩法地图需要相机；门面 `main` 没有玩家实体。
  local ids = {}
  for map_id in pairs(maps.maps) do
    if type(map_id) == "string" then
      ids[#ids + 1] = map_id
    end
  end
  table.sort(ids)

  for _, map_id in ipairs(ids) do
    local path = "scenes/" .. map_id .. ".scene"
    local text = read_file(path)
    if text then
      local cameras = component_payloads(text, "Camera")
      if #cameras == 0 then
        issues[#issues + 1] = string.format(
          "%s: 缺少相机（应为 1 个 `Camera projection=orthographic;...;bounds=0,0,W*32,H*32`，"
            .. "否则相机取景会超出地图边缘）", path
        )
      elseif #cameras > 1 then
        issues[#issues + 1] = string.format(
          "%s: 声明了 %d 个 `Camera`；本工程每张玩法地图只允许 1 台（多台会让"
            .. "`docs/54` §8-① 的“场景初始相机”取决于实体顺序）", path, #cameras
        )
      else
        local payload = cameras[1]
        if not payload:match("projection=orthographic") then
          issues[#issues + 1] = string.format(
            "%s:Camera 必须是 projection=orthographic（本工程是 2D 玩法，透视相机会让画面全黑）",
            path
          )
        end
        local zoom = payload_number(payload, "zoom")
        if zoom ~= 1 then
          issues[#issues + 1] = string.format(
            "%s:Camera zoom=%s，应为 1（docs/52 §4 裁定竖版只收窄可见列数、不缩放世界）",
            path, tostring(zoom)
          )
        end
        local rect_w, rect_h = map_rect(text)
        local bounds = payload_vec4(payload, "bounds")
        if not rect_w then
          issues[#issues + 1] = string.format("%s: 场景里没有可解析的 Tilemap，无法核对相机 bounds", path)
        elseif not bounds then
          issues[#issues + 1] = string.format(
            "%s:Camera 的 bounds 缺失或不是 4 个数（没有 bounds 时相机中心不受地图约束，"
              .. "取景会跑出地图边缘）", path
          )
        else
          -- 逐值比对：min 必须是原点、max 必须等于瓦片图覆盖的世界矩形。
          if bounds[1] ~= 0 or bounds[2] ~= 0
            or math.abs(bounds[3] - rect_w) > 0.001 or math.abs(bounds[4] - rect_h) > 0.001 then
            issues[#issues + 1] = string.format(
              "%s:Camera bounds = %s,%s,%s,%s，应为 0,0,%s,%s（= 地图 %.0f×%.0f 世界单位）",
              path, tostring(bounds[1]), tostring(bounds[2]), tostring(bounds[3]), tostring(bounds[4]),
              tostring(rect_w), tostring(rect_h), rect_w, rect_h
            )
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
