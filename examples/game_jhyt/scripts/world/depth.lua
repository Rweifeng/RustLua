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

-- M72-06 C-3：**Y 排序容差的显式定义**。`docs/53` §5 记的是"遮挡规则从 1–2 张截图推不出来"，
-- 所以下面第 1 条是**推断规则**（等实机参考再校），但它必须给数：不给数就只能靠目验，而目验不会红。
--   1. **容差 = `STEP` 本身（2 世界单位）**。脚位 y 差 < STEP 的两个 actor 落进同一个 order 桶，
--      先后由渲染器的稳定次键（实体插入序）决定 ⇒ 这一档**不许**被读成"谁在前谁在后"，美术与
--      玩法都不能依赖它；差 ≥ STEP 才要求绘制顺序稳定翻转（`jhyt_m72_foreground_occlusion` 门 §⑧
--      两侧各测一次：Δy=1 → order 一字不变、Δy=2 → 恰好翻一桶）。
--   2. **静态物件不参与 Y 折**。主键是 `层秩 × 1000 + order`（world −1 < actor +1 < foreground +2，
--      见 `rustlua-render/src/sprite.rs` 的 `sorting_layer_rank`），所以人恒画在所有 world 物件
--      **之后**：站在房子北边不会被房子盖住。这是设计，不是 bug——要压住人只有一条路，把悬空的
--      那上半张登记进 foreground（M72-06 B 段起的 4 件树冠，M72-07 把屋顶拆法推广到
--      柴房与五座可建造建筑后共 11 件）。
--   3. **foreground 恒压人、不看 y**，所以整张图进 foreground 会连"站在它南边的人"一起盖掉。
--      这条就是第 2 点必须拆层的原因，也是 `data/lighting.lua` 的 `occlusion.max_cover_ratio`
--      要钉住覆盖率上界的原因（拆到顶 57 行时最深 0.861，人被吞得只剩一条边）。
--
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
