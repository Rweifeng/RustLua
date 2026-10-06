-- world/depth.lua：把角色（actor 排序层）的**脚位 Y** 折成 Sprite 组件的 `order`，
-- 让"谁靠下谁盖住谁"这条画家顺序在 PC 与 web/微信上同时成立。
--
-- 为什么是 order，不是 z：两个宿主的**主排序键**都是 `层秩 × 1000 + order`
-- （原生 `SpriteSorting::effective_layer`，web `entitySortKey`），而带 z 的那条世界深度
-- 通道只有原生渲染器有、web 完全不认，且原生的 overlay 通道显式忽略 z。所以 order 是唯一
-- 一份"两边都读"的深度杠杆。
--
-- 为什么量化成桶而不是逐帧写精确值：order 是整数且被夹在 ±999（超出会溢进相邻层桶），
-- 而只在桶位变化时才重写组件串——每帧给每个角色重排一次字符串就是每帧一次分配 + 一次解析。
local M = {}

local STEP = 2      -- Y 量化步长（世界单位）。角色贴图 64 单位高，2 单位的错位肉眼看不出。
local ORDER_MAX = 999

local tracked = {} -- 实体 id → { spec = 作者侧组件串（不含 order）, order = 已写下去的桶位 }

-- Y 的朝向：这台引擎的世界 +y 是**朝上**的（`world_to_screen` 里 `screen.y = h/2 - view.y*zoom`，
-- web 侧 `toCanvas` 的 `360 - (wy - camera.y)*zoom` 同一条）。于是"越靠画面下方 = 越近"
-- 对应的是 **y 越小越近**；画家顺序是后画的盖住先画的 ⇒ order 必须随 y **单调递减**。
-- 反方向写出来的症状不是报错，而是"主角站在 NPC 下面反而被 NPC 盖住"。
local function order_for(y)
  local order = -math.floor((tonumber(y) or 0) / STEP)
  if order < -ORDER_MAX then
    return -ORDER_MAX
  end
  if order > ORDER_MAX then
    return ORDER_MAX
  end
  return order
end

-- 按实体当前 Y 重写 Sprite 串。只在桶位变化时才调 sprite.set。
-- 追加的 `;order=` 一定赢过作者串里同名的字段：组件串解析是"后写的覆盖先写的"。
local function apply(id, record)
  local ok, pos = pcall(transform.get, id)
  local y = ok and type(pos) == "table" and pos.y or 0
  local order = order_for(y)
  if record.order == order then
    return false
  end
  sprite.set(id, record.spec .. ";order=" .. order)
  record.order = order
  return true
end

-- 登记一个角色实体并立刻按当前 Y 落一次 order。
-- 调用点用它**代替** sprite.set：这样"挂形象"和"算遮挡"是同一个入口，不会有人只调了一半。
-- 失败语义与 sprite.set 一致（直接向调用方抛出，外层的 pcall 照旧接得住）。
function M.attach(id, spec)
  tracked[id] = { spec = spec }
  apply(id, tracked[id])
  return true
end

-- 每帧一次：位置在宿主侧积分（web 的位移发生在 update 返回之后的状态导出里），
-- 所以这里读到的是"本帧将要被画出的那个位置"，两个宿主同一套一帧滞后。
function M.tick()
  for id, record in pairs(tracked) do
    if entity.is_alive(id) then
      apply(id, record)
    else
      tracked[id] = nil
    end
  end
end

return M
