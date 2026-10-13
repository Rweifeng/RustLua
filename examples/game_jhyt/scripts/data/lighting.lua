-- data/lighting.lua：全局光源与投影契约（M72-06）。
--
-- **为什么要有这一份文件**：docs/51 §2.2 与 docs/57 §1 写死"光源来自左上、阴影落在右下，
-- 禁止同一批资产出现 2 种光源"，可这条口径此前只存在于散文里——谁都没法查。投影方向一旦
-- 各画各的，症状不是报错，是"这一棵树往右下倒、那一棵树往左下倒"，玩家一眼就觉得画面不是一
-- 个世界（正是 docs/53 §3.9 记的那条）。
--
-- **单一事实源**：这里写一次，三处读——
--   ① `tools/light_kit.py` 解析本文件（与 `gen_manor_scene.py` 解析 `data/places.lua` 同一条先例），
--      生成器按它画贴片像素与偏移；
--   ② `crates/game/examples/jhyt_m72_foreground_occlusion.rs` 与光向一致性门按它比对盘上读数；
--   ③ 前景遮挡的容差（`occlusion` 段）是运行期与门共用的判据。
-- 谁都不许在自己文件里再写一份 `(dx, dy)`。
--
-- **坐标系**：世界 +y 朝上（`scripts/world/depth.lua` 头注释：`world_to_screen` 里
-- `screen.y = h/2 - view.y*zoom`，web 侧同一条）。所以屏幕上的"右下"= **(+x, -y)**。
-- 这条符号是整个文件的前提：写成 y 为正，门会直接把光向判反（跨宿主绘制顺序门踩过同一条坑）。
local M = {}

-- 光的**传播方向**（未归一，只钉方向不钉长度）：左上射来 ⇒ 落右下。
-- 刻意用整数对，让"角度是否一致"成为可以逐字比对的读数，而不是浮点近似。
M.sun = { dir_x = 1, dir_y = -1 }

-- 建筑/树/道具的**地面投影贴片**：偏移长度 = 物体贴图高度 × reach（角度恒等于 sun 方向）。
-- reach 的取值口径：正房 224 单位高、偏移向量长 ≈27 单位（x/y 各 19，≈0.8 个瓦格）——
-- 比"低太阳角长影子"短，因为这游戏的相机接近俯视，影子过长会读成"房子在斜面上"。
-- 星露谷同档（目测其建筑影子 ≈0.5–1 格）；**这条是推断**，等实机参考再校（见 docs/57 的标注）。
M.shadow_reach = 0.12

-- 投影贴片的画法（像素与 tint 一起决定"看着有多暗"）：
--   * PNG 一律 RGBA8、alpha 只有 0/255（docs/57 §1.3 软边门），**透明度走 sprite tint 的 a**，
--     这样贴片不会变成"半透明例外"的第二个口子；
--   * 颜色取墨色一档（与角色脚下那片 baked shadow 同色族 `#1f2328`），否则影子读起来像洞。
M.shadow_tint = { r = 0.122, g = 0.137, b = 0.157, a = 0.28 }

-- 投影贴片（cast shadow）的**挂载参数**。几何不在这里：贴片的偏移恒等于
-- `M.shadow_offset(物体总高)`，这里只钉"挂哪一档、什么体量才配挂、椭圆多厚"。
M.cast_shadow = {
  -- world 层内的 order 分带。盘上读数（`tools/decor_kit.py` 的 BAND_* 与各本体实体）：
  -- 碎屑 -30、正房 0、道具 5/10、中景 12、树 20 ⇒ 取 **-20** 恰好压在碎屑之上、
  -- 任何本体之下——"影子盖住房子"这种翻车在桶位上就不成立，不靠人记得。
  order = -20,
  -- 体量门槛：偏移向量长 < 8 世界单位（¼ 瓦）的物体不挂贴片。山庄 11 类候选实测：
  -- 正房 26.9 / 柴棚 15.4 / 庄门 11.5 / 树 10.6 / 竹 9.1 ⇒ 挂；篱 7.7 / 架 5.5 /
  -- 井 5.0 / 招牌 5.0 / 草垛 4.1 / 罐 3.6 ⇒ 不挂——偏移不到半格时贴片只会读成一团脏像素。
  min_reach_units = 8,
  -- 接触椭圆的厚宽比。**不是新的审美数**：它就是角色脚下那片既有椭圆的厚宽比
  -- （`gen_character_sprites.draw_shadow` 最宽一行 15 单位、共 3 行 ⇒ 3/15），
  -- 贴片沿用同一档，免得"脚下的"和"落地的"是两种透视。
  contact_flatten = 0.2,
}

-- 角色脚下投影：星露谷式、**烘在序列帧里**（`tools/gen_character_sprites.py` 的 `draw_shadow`）。
-- 为什么角色不另开贴片实体：角色有 8 向 × 8 组动作帧，脚位在帧内逐帧移动，
-- 贴片要么每帧配一个偏移（多一份要漂移的数据），要么固定偏移（走起来脱脚）；
-- 烘在帧里的椭圆天然跟脚，而且三个宿主同幅生效、零运行期开销。
-- 这一档 alpha=70/255 是**已在盘上的既有事实**（角色帧底部第 60–63 行一直是这个值），
-- docs/57 §1.3 把它登记为软边门的例外。
--
-- `shift_x` / `shift_rows`：椭圆相对**脚位**往右下挪多少（绘制空间 32 的像素，行号朝下为正）。
-- 这里取 `2 / 0` 而不是按 sun 的 45° 等量下移，两条理由都是量出来的：
--   ① 装不下——角色帧导出后不透明像素一直铺到第 63 行（64×64 的最后一行），投影本来贴在
--      60–63 行，再往下挪就出框；
--   ② 不该装——脚下这片是**接触阴影**（脚和地面之间那一条），它在任何光向下同形，
--      本身不携带光源信息。真正表达"光从左上来"的是建筑/树/道具的**投影贴片**。
--      所以光向角度方差门只采投影贴片的样本，对接触阴影只断言"不往左上跑"（shift ≥ 0），
--      不要求它等于 sun 的角度——否则门会把一个正确的画法判成错误。
M.actor_shadow = {
  shift_x = 2,
  shift_rows = 0,
  alpha = 70,
  color = { r = 0x1f, g = 0x23, b = 0x28 },
}

-- 前景遮挡的容差（docs/57 §1.3 第 5 条"遮挡门"要求的可量判据）。
-- 一条散文规则如果不给数，就只能靠目验，而目验不会红。
M.occlusion = {
  -- 玩家站在前景覆盖最深的地方，允许被盖住的**自身不透明像素**占比上限。
  -- 0.65 **是量出来以后留余量的数**，不是审美目标（门 §④ 每次跑都重算，命令在下面）。
  --   * 口径：深度 = 玩家按**关卡可行走契约**（GroundMap `solid`，与 `data/places.lua` 的出生点、
  --     `tilemap.find_path` 的寻路、`systems/building.lua` 的占地判定同一张表）能站到的位置里，
  --     前景块不透明像素盖住玩家不透明像素的占比；角色取 `idle_s` 条带**每帧各扫一遍取最坏**，
  --     像素按 `alpha > 0` 计（所以帧底那片接触阴影椭圆算角色像素——它是 sprite 自己的一部分）。
  --   * 实测（`cargo run --quiet -p game --example jhyt_m72_foreground_occlusion` §④）：
  --     屋顶 0.307 / 西树 0.000 / 北树 0.070 / 东树 0.418 / 东南树 **0.604**，最深点在东南树的
  --     (1216,558)。0.65 = 0.604 + 0.046（≈7.6% 余量：够后续微调美术而不必每次回来改这条）。
  --     这一档是**脚下接触阴影改成"镜像之后盖章"（C-1）之后重量的**：阴影椭圆按上面的口径算角色
  --     像素，而盖章前镜像帧的椭圆在另一侧 ⇒ 玩家条带的不透明像素集合变了，最深点从 (1216,548)
  --     挪到 (1216,558)、0.602 变 0.604。数字跟着画法走，不是把上界挪了去迁就画面。
  --     西树那一列的 **0.000 不是侥幸**：同一行"不限可行走 0.604"说明几何上压得到，只是那些位置
  --     按 `solid` 全进不去——那棵树在图北缘（cell 3,19），它树冠占的那两行（行 20–21、列 0–4）
  --     整片是实心格。下限 `min_cover_ratio` 断言的是全图最深，所以这种"压不到人的树"不会被放过：
  --     五件遮断物一件都压不到人时最深就是 0，直接红。
  --   * 这一条**曾经先是几何问题，不是数字问题**：树冠切到顶 57 行时最深是 0.861（人被吞得只剩
  --     一条边），而"上界 0.40"在那一档是任何可达位置都做不到的。把切点改到顶 40 行
  --     （`tools/gen_manor_art.LAYER_SPLITS`，两块 PNG 与 `manor.scene` 里那 5 件实体跟着走，
  --     整张 `manor_tree.png` 一个字节没动 ⇒ village / combat_arena 不受影响）才落到 0.604。
  --     括号里那句"village / combat_arena 不受影响"**只对 M72-06 当时的盘上状态成立**：那两张
  --     图当时引用的确实是整图。M72-11 ④ 之后演武场也走拆层（`grep -c manor_tree_canopy
  --     scenes/combat_arena.scene` = 4），所以顶 40 行这个切点**现在是这三张图共用的**，
  --     改它要 manor / village / combat_arena 一起复量。
  --     所以这条上界钉的是**美术切点**，不是运行期的宽容度。三档渲染对比的读法：57 行 =
  --     人没了一半、40 行 = 露腿而头在树后、26 行 = 读成"人站在树前面"（错），取中间那档。
  --   * 同一次扫描里"不限可行走"的理论上界是 1.000（屋顶）——那要求玩家站进正房的实心格里。
  --     实心格在物理世界里是真的墙：`scene_spawn.rs` 的 `solid_cell_colliders` 给每个实心格
  --     在 GroundMap 的静态刚体上挂一个 cuboid，门 §④ 的逐格探针读数 360 个实心格各恰好
  --     命中 1 个、560 个空格 0 个 ⇒ "可达"就是玩家真能站到的地方，那 1.000 只是"扫描没漏看"
  --     的证据。（这一句此前写反了，写成"本项目没把 solid 做成碰撞体"，凭的是只 grep 场景文件
  --     里那一行 Collider；探针跑出来的读数把它纠正了，措辞跟着读数改。）
  --   * **已知取舍**：0.65 仍然允许"头肩全没、只剩腿"的瞬间。要把它也消掉，正确解法是被完全
  --     遮断时让前景淡出（需要逐帧覆盖率的运行期读数，本包不做），登记为后续项，不是靠放宽
  --     这条上界来装作没问题。
  max_cover_ratio = 0.65,
  -- 下限同样要断言：前景层加了却一点没压到人，那条层就是白加的（门拿这条防"绿的空门"）。
  min_cover_ratio = 0.05,
  -- **这里原来有一档 `ground_shade`（被前景压住的地面走 per-tile tint 压暗），已删**。
  -- 删的理由不是"没做"，是那条通道在层序上表达不了这件事：Tilemap 恒在一切 sprite 之前
  -- 画（`preview_scene.py` §排序与引擎同一条），所以"被遮断物盖住的那格地面"只有在遮断物
  -- 自己的**透明缝隙**里才看得见——压暗会从零散缝隙里漏出来，读成花斑而不是阴影。真正看得见
  -- 的"被遮地面"是遮断物**周围**那一圈，而那一圈正是 M72-06 C-2 的投影贴片负责的区域。
  -- 实测交集（`tools/set_cast_shadows.py` 同一条读数口径；M72-07 把遮断物从 5 件扩到 11 件
  -- 之后重新量过）：11 件遮断物覆盖的 116 格地面 与 16 张贴片覆盖的 124 格 交集 = **8 格**，
  -- 而那 8 格全部落在柴房/库房/练功房/茶棚**自己的贴图之下**（盘上是 `solid` 格，人走不进去）
  -- ⇒ 两种载体管的是两块走得到的地，不存在"二选一"，也不存在叠两层墨。
  -- 附带一条纠错：这一档当时写作"与色阶表 `shaded` 档同族"，其实 `terrain_ramp.json` 的
  -- `shaded` 是 0.96/0.98/1.00（±3% 的地块变化），0.78 是**第三档**，加进去要动色阶表本身。
}

-- 前景层内的 order 分带（层秩 foreground = +2，见 `rustlua-render/src/sprite.rs`）。
-- **为什么静态遮断物的 order 比特效低**：特效（`systems/effects.lua`）用的就是同一个
-- foreground 桶、order 0。树冠/檐口写负数，特效才永远压在树冠之上——刀光被自己的树冠
-- 盖掉是不可接受的（docs/57 §1.3 第 5 条：前景可压角色，但不可覆盖交互提示与 HUD）。
M.foreground_order = { occluder = -100, effect = 0 }

-- 归一后的投影方向（Lua 侧与工具侧同一个算法；dir_y 恒为负由 validate_all 钉住）。
function M.shadow_direction()
  local x = M.sun.dir_x
  local y = M.sun.dir_y
  local len = math.sqrt(x * x + y * y)
  if len == 0 then
    return nil, "lighting: sun 方向是零向量"
  end
  return { x = x / len, y = y / len }
end

-- 高度 h 的物体应投出的偏移（世界单位）：长度 = h × reach，角度 = sun。
function M.shadow_offset(height)
  local dir, err = M.shadow_direction()
  if not dir then
    return nil, err
  end
  local reach = M.shadow_reach * (tonumber(height) or 0)
  return { x = dir.x * reach, y = dir.y * reach }
end

function M.validate_all()
  local errors = {}
  local function err(message)
    errors[#errors + 1] = "lighting: " .. message
  end

  -- 光向必须在**右下象限**（世界 +y 朝上 ⇒ x>0、y<0）。写成 y 为正的门症状是
  -- "全庄园的影子朝天上去"，而美术不会自己说。
  if (tonumber(M.sun.dir_x) or 0) <= 0 then
    err("sun.dir_x 必须为正（影子往右落），现在 " .. tostring(M.sun.dir_x))
  end
  if (tonumber(M.sun.dir_y) or 0) >= 0 then
    err("sun.dir_y 必须为负（世界 +y 朝上，影子往画面下方落），现在 " .. tostring(M.sun.dir_y))
  end
  if not M.shadow_direction() then
    err("sun 方向归一失败")
  end

  if (tonumber(M.shadow_reach) or 0) <= 0 then
    err("shadow_reach 必须为正，现在 " .. tostring(M.shadow_reach))
  end

  local tint = M.shadow_tint or {}
  for _, key in ipairs({ "r", "g", "b", "a" }) do
    local v = tonumber(tint[key])
    if not v or v < 0 or v > 1 then
      err("shadow_tint." .. key .. " 必须在 0..1，现在 " .. tostring(tint[key]))
    end
  end
  -- a=1 的投影贴片等于在地上刷一块墨，那不是影子是洞；a 太小则看不见。
  if tonumber(tint.a) and (tint.a <= 0 or tint.a >= 0.6) then
    err("shadow_tint.a 必须落在 (0, 0.6)——投影要看得见，又不能读成地面本身的颜色，"
      .. "现在 " .. tostring(tint.a))
  end

  local cast = M.cast_shadow or {}
  local cast_order = tonumber(cast.order)
  if not cast_order or math.abs(cast_order) > 999 then
    err("cast_shadow.order 必须是 ±999 以内的整数（层桶 = 层秩×1000 + order，越界会溢进相邻层），"
      .. "现在 " .. tostring(cast.order))
  end
  if (tonumber(cast.min_reach_units) or 0) <= 0 then
    err("cast_shadow.min_reach_units 必须为正，现在 " .. tostring(cast.min_reach_units))
  end
  local flatten = tonumber(cast.contact_flatten)
  if not flatten or flatten <= 0 or flatten > 1 then
    err("cast_shadow.contact_flatten 必须落在 (0,1]，现在 " .. tostring(cast.contact_flatten))
  end

  local actor = M.actor_shadow or {}
  if (tonumber(actor.shift_x) or 0) < 0 then
    err("actor_shadow.shift_x 不能为负（那是往左上投影），现在 " .. tostring(actor.shift_x))
  end
  if (tonumber(actor.shift_rows) or 0) < 0 then
    err("actor_shadow.shift_rows 不能为负（行号朝下为正，负值就是朝天投影），"
      .. "现在 " .. tostring(actor.shift_rows))
  end
  local alpha = tonumber(actor.alpha)
  if not alpha or alpha <= 0 or alpha >= 255 then
    err("actor_shadow.alpha 必须是 (0,255) 的半透明档，现在 " .. tostring(actor.alpha))
  end

  local occ = M.occlusion or {}
  local max_cover = tonumber(occ.max_cover_ratio)
  local min_cover = tonumber(occ.min_cover_ratio)
  if not max_cover or max_cover <= 0 or max_cover > 1 then
    err("occlusion.max_cover_ratio 必须在 (0,1]，现在 " .. tostring(max_cover))
  end
  if not min_cover or min_cover < 0 or min_cover >= max_cover then
    err("occlusion.min_cover_ratio 必须落在 [0, max_cover_ratio)，现在 " .. tostring(min_cover))
  end

  local fg = M.foreground_order or {}
  local occluder = tonumber(fg.occluder)
  local effect = tonumber(fg.effect)
  if not occluder or not effect then
    err("foreground_order 需要 occluder 与 effect 两个整数档")
  else
    -- 特效必须在遮断物**之后**画（order 大 = 后画 = 盖在上面），否则刀光被树冠埋掉。
    if not (effect > occluder) then
      err("foreground_order 里特效的 order 必须大于遮断物（刀光不能被树冠埋掉），"
        .. "现在 occluder=" .. tostring(occluder) .. " effect=" .. tostring(effect))
    end
    -- 夹取口径：层桶是 层秩×1000 + clamp(order, ±999)，越界会溢进相邻层。
    if math.abs(occluder) > 999 or math.abs(effect) > 999 then
      err("foreground_order 超出 ±999 会溢进相邻层桶")
    end
  end

  return errors
end

return M
