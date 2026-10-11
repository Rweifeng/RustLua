//! Headless acceptance drive for M72-05：**装饰散布的数量、载体与遮挡带**。
//!
//! 需求（`tasks/plan/M72-05-装饰散布生成器与碎屑库.md`）：manor 从前只有 15 个非幽灵 sprite，
//! "画面几乎没有东西"是观感差距的头号来源。本门钉的是**每屏真的有多少东西**，以及那批东西
//! 有没有乖乖待在该待的层带里。
//!
//! 为什么判据从 mesh 与从文件两边各数一遍：只看文件，等于承认"写进去就画得出来"；只看 mesh，
//! 一次相机恰好停在热闹处就能蒙绿。两边必须对上，且文件侧要扫**所有**可停靠窗口取中位数。
//!
//! 章节：
//!   ① 结构契约：每件 `role=decor` 实体恰好 Sprite+Transform、**没有 Collider**、层名 `world`、
//!      order 只取两档之一；声明尺寸 == 门内目录副本 == PNG 实测（三方一致，贴图必须真的在盘上）。
//!      反面对照：把目录副本的一个尺寸改 1px，本节必须红。
//!   ② 每屏可见对象：按相机矩形（`zoom=2` → 640×360 世界单位）以 8px 步进滑过全图，
//!      报 min/median/max，判据 **median ≥40**（plan §验收方式；前值是 2）。
//!      为什么是 median 而不是 min：药田 200 格按玩法契约必须空着，最坏那一屏 89% 的面积
//!      是留白——"任何一屏 ≥40"在这张图上做不到，硬写只会逼生成器往田里撒东西。
//!   ③ 渲染器真在画：关掉视口外精灵剔除后 mesh 的装饰四边形数 == 文件里的装饰实体数；
//!      开剔除后 == 门按同一判据（视口 + 外接圆半径，复刻 `sprite_may_be_visible`）数出的数量。
//!      两个数一起报：第一条抓"实体没画出来"，第二条抓"剔除把装饰裁掉了"。
//!   ④ 遮挡带：装饰一律留在 `world` 层秩内（**不许进 actor**）；碎屑档 order 必须低于场景里
//!      任何非装饰 sprite，中景档必须低于树/竹。反面对照：把一件装饰提到树的 order，本节必须红。
//!   ⑤ 装饰不参与物理：每件装饰实体在**运行期的刚体位置**各做一遍圆查询，取到装饰贴图的次数必须为
//!      0，并另打一发带 collider 的实体作正向对照（`physics.overlap` 走 Rapier 的碰撞体宽相，装饰
//!      一旦长出 collider，玩家就会在草地上被一块"看不见的石头"挡住）。命中按**实体 id 分类**：
//!      归 GroundMap 的是地表实心格按格生成的静态碰撞体（`scene_spawn.rs` 的
//!      `solid_cell_colliders`），认不出的必须为 0。
//!
//! 目录（`CATALOG`）在门里是**手抄副本**，故意的：门的职责是"生成器画出的 PNG 与它声明的
//! 尺寸、场景里写的尺寸必须是同一个"，如果门去解析 `decor_kit.py` 就变成那份表自己跟自己一致。
//! 副本与表飘红时本门红在"声明尺寸 ≠ 目录"，报出来的就是飘掉的那一条。
//! 散布的**确定性**（同种子两次逐字节相同、换种子必须变、盘上段 == 重算段）在 Python 侧，
//! 因为那里才是散布的实现处：`python tools/set_scatter_decor.py scenes/manor.scene --check`。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_scatter_decor [-- <game_jhyt-root>]

use engine::host::{RuntimeScene, SceneMesh};
use rustlua_input::input::InputSnapshot;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;

const SCENE: &str = "scenes/manor.scene";
const DECOR_PREFIX: &str = "assets/art/decor/";
/// 碎屑档：画在所有建筑/道具之前，**永不**遮挡任何东西。
const BAND_DEBRIS: i32 = -30;
/// 中景档：夹在小道具（≤10）与树/竹（20）之间。
const BAND_MID: i32 = 12;
const BANDS: [i32; 2] = [BAND_DEBRIS, BAND_MID];
/// 树与竹的 order：中景档必须留在它们下面，否则灌木会浮在树冠上。
const TREE_ORDER: i32 = 20;

/// `zoom=2` 的一屏世界尺寸（docs/51 §2.2：1280×720 视口 → 20×11.25 格）。
const SCREEN_W: f32 = 640.0;
const SCREEN_H: f32 = 360.0;
/// 滑窗步进：8px = 1/4 格。再密不会改变结论，只会让门跑得更久。
const WINDOW_STEP: f32 = 8.0;
/// 地图尺寸（格）× tile：窗口可停靠的范围由此而定。
const MAP_W: f32 = 40.0 * 32.0;
const MAP_H: f32 = 23.0 * 32.0;

/// 量化判据（plan §验收方式）：每屏可见对象 ~2 → ≥40。
const MIN_MEDIAN_PER_SCREEN: usize = 40;
/// 装饰实体数下限：低于这个数，§② 的中位数根本不可能到 40，报错时先说这个。
const MIN_DECOR: usize = 100;

/// 装饰目录（`tools/decor_kit.py::CATALOG` 的手抄副本）：`(name, stem, w, h, band)`。
/// 镜像档是**另一张 PNG**（`_r` 后缀），不是场景里的 flip 字段——引擎的规范序列化不写 flip。
const CATALOG: &[(&str, &str, u32, u32, i32)] = &[
    ("GrassShort", "decor_grass_short", 16, 10, BAND_DEBRIS),
    ("GrassTuft", "decor_grass_tuft", 18, 14, BAND_DEBRIS),
    ("Twigs", "decor_twigs", 20, 9, BAND_DEBRIS),
    ("Pebbles", "decor_pebbles", 16, 7, BAND_DEBRIS),
    ("Petals", "decor_petals", 14, 6, BAND_DEBRIS),
    ("LeafPile", "decor_leaf_pile", 18, 10, BAND_DEBRIS),
    ("MossPatch", "decor_moss_patch", 20, 8, BAND_DEBRIS),
    ("Bush", "decor_bush", 28, 22, BAND_MID),
    ("Rock", "decor_rock", 24, 18, BAND_MID),
    ("Flowers", "decor_flowers", 22, 18, BAND_MID),
    ("Mushroom", "decor_mushroom", 16, 14, BAND_MID),
    ("Reed", "decor_reed", 20, 28, BAND_MID),
    ("Lily", "decor_lily", 22, 14, BAND_DEBRIS),
];

/// `terrain_ramp.json` 的 `ground_tints` + 白档：装饰的顶点色只许照抄地表那三档。
const TINT_LEVELS: [[f32; 3]; 3] = [[1.0, 1.0, 1.0], [0.96, 0.98, 1.0], [1.03, 1.02, 0.99]];

/// `scripts/world/depth.lua` 的层秩表：`world` = -1，`actor` = +1。
/// 主排序键是 `层秩×1000 + order`，所以**跨层秩的 order 比较没有意义**：装饰一旦进 `actor`，
/// 它对这个带里的任何东西（包括玩家）都恒占上风，长在房子北边的灌木就会浮在屋顶上。
const LAYER_RANK: &[(&str, i32)] = &[
    ("background", -2),
    ("world", -1),
    ("default", 0),
    ("actor", 1),
    ("foreground", 2),
    ("ui", 3),
];

/// 场景里一个 sprite 实体的门侧视图（从 `.scene` 文本读，与 mesh 独立）。
#[derive(Clone, Debug)]
struct Placed {
    name: String,
    texture: String,
    width: u32,
    height: u32,
    order: i32,
    layer: String,
    x: f32,
    y: f32,
    tint: [f32; 3],
    collider: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let sprites = read_scene(&project.join(SCENE))?;
    let decor: Vec<Placed> = sprites
        .iter()
        .filter(|item| item.name.starts_with("Decor_"))
        .cloned()
        .collect();
    let others: Vec<Placed> = sprites
        .iter()
        .filter(|item| !item.name.starts_with("Decor_"))
        .cloned()
        .collect();

    banner("① 结构契约：组件面、层带、目录与 PNG 三方尺寸一致");
    if decor.len() < MIN_DECOR {
        return Err(format!(
            "装饰实体只有 {} 件，应 ≥{MIN_DECOR}：散布没跑还是被清掉了？",
            decor.len()
        )
        .into());
    }
    for item in &decor {
        if item.collider {
            return Err(format!(
                "{} 带 Collider：碎屑一律非实体。给装饰加碰撞体会让玩家在草地上被看不见的东西挡住，\
                 并污染 `solid` 的语义（plan §设计要点 5）",
                item.name
            )
            .into());
        }
        if item.layer != "world" {
            return Err(format!(
                "{} 的 sorting_layer = {:?}，应为 world：装饰不许自己开一层，\
                 层秩×1000+order 是 `scripts/world/depth.lua` 定的唯一口径",
                item.name, item.layer
            )
            .into());
        }
        if !BANDS.contains(&item.order) {
            return Err(format!(
                "{} 的 order = {}，不在两档 {BANDS:?} 之内",
                item.name, item.order
            )
            .into());
        }
        if !item.texture.starts_with(DECOR_PREFIX) {
            return Err(format!(
                "{} 的贴图 = {:?}，不在 {DECOR_PREFIX} 下",
                item.name, item.texture
            )
            .into());
        }
        let want = catalog_of(&item.name).ok_or(format!(
            "{} 这个名字不在装饰目录里：散布与美术各画各的，门内副本先拦一个",
            item.name
        ))?;
        if (item.width, item.height) != (want.2, want.3) {
            return Err(format!(
                "{} 声明 {}x{}，目录（`decor_kit.CATALOG`）写的是 {}x{}",
                item.name, item.width, item.height, want.2, want.3
            )
            .into());
        }
        if item.order != want.4 {
            return Err(format!(
                "{} 的 order = {}，目录说它属于带 {}",
                item.name, item.order, want.4
            )
            .into());
        }
        let (w, h) = png_size(&project, &item.texture)?;
        if (w, h) != (item.width, item.height) {
            return Err(format!(
                "{} 的 PNG 实测 {w}x{h} ≠ 场景声明 {}x{}：贴图与散布各改了一半",
                item.name, item.width, item.height
            )
            .into());
        }
        if tint_bucket(item.tint).is_none() {
            return Err(format!(
                "{} 的 tint {:?} 不在地表染色三档里：装饰的明度必须照抄它脚下那一格的地表，\
                 否则同一屏会出现两套光照",
                item.name, item.tint
            )
            .into());
        }
    }
    let shapes: BTreeSet<&str> = decor.iter().map(|item| stem_of(&item.name)).collect();
    let mirrored = decor
        .iter()
        .filter(|item| item.texture.ends_with("_r.png"))
        .count();
    println!(
        "  {} 件装饰、{} 种形状全在目录内；PNG 与声明尺寸逐件相同；镜像档 {mirrored} 件（另出的 PNG，不是 flip 字段）",
        decor.len(),
        shapes.len()
    );
    if shapes.len() != CATALOG.len() {
        let missing: Vec<&str> = CATALOG
            .iter()
            .filter(|row| !shapes.contains(row.0))
            .map(|row| row.0)
            .collect();
        return Err(format!(
            "目录里 {} 种形状有 {missing:?} 一次都没被抽到：那张图白画了（香蒲就撞过一次——\
             它要求临水，而池塘四周的格子在留白规则里被药田吃掉了）",
            CATALOG.len()
        )
        .into());
    }
    // 反面对照：目录副本改 1px，同一套比对必须红（否则 §① 的尺寸断言是空的）。
    let mut bent = CATALOG.to_vec();
    bent[0] = (bent[0].0, bent[0].1, bent[0].2 + 1, bent[0].3, bent[0].4);
    let caught = decor.iter().any(|item| {
        let want = &bent[catalog_index(&item.name)];
        (item.width, item.height) != (want.2, want.3)
    });
    if !caught {
        return Err(
            "反面对照失手：把目录第一条的尺寸改 1px 后 §① 仍判绿，尺寸比对其实没在看目录".into(),
        );
    }
    println!("  反面对照：目录副本改 1px 即红（首件尺寸立刻对不上）");

    banner("② 每屏可见对象：滑过所有可停靠窗口，判据是中位数 ≥40");
    let (min, median, max, worst) = sweep_windows(&decor);
    if median < MIN_MEDIAN_PER_SCREEN {
        return Err(format!(
            "每屏可见装饰中位数只有 {median}，应 ≥{MIN_MEDIAN_PER_SCREEN}（重做前是 ~2）。\
             最坏窗口左下角 {worst:?}——那一屏压在药田上，按玩法契约必须留白，所以判据取中位数"
        )
        .into());
    }
    let debris = decor
        .iter()
        .filter(|item| item.order == BAND_DEBRIS)
        .count();
    println!(
        "  窗口 {SCREEN_W}x{SCREEN_H} 世界单位、每 {WINDOW_STEP}px 取一窗：min {min} / median {median} / max {max}\
         （最坏窗口左下角 {worst:?}）；碎屑 {debris} 件 / 中景 {} 件",
        decor.len() - debris
    );

    banner("③ 渲染器真在画：mesh 的装饰四边形数与文件对账");
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    for _ in 0..4 {
        frame(&mut runtime)?;
    }
    assert_scene(&mut runtime, "manor")?;
    runtime.set_sprite_culling(false);
    let whole = decor_quads(&mut runtime)?;
    if whole != decor.len() {
        return Err(format!(
            "关掉视口剔除后 mesh 只画出 {whole} 件装饰，文件里有 {} 件：\
             有实体没进帧（贴图解析失败、组件缺字段或被别的系统删掉），这种漏不会报错",
            decor.len()
        )
        .into());
    }
    runtime.set_sprite_culling(true);
    let (center, zoom) = camera(&mut runtime)?;
    let live = decor_quads(&mut runtime)?;
    let want = visible_count(&decor, center, zoom);
    if live != want {
        return Err(format!(
            "开剔除后 mesh 画出 {live} 件，门按视口判据数出 {want} 件：渲染器与门对\"屏内\"的理解分叉了"
        )
        .into());
    }
    if live == whole {
        return Err(format!(
            "开剔除与关剔除画出的件数相同（都 = {whole}）：视口外精灵剔除对装饰根本没生效，\
             §③ 第二条断言是空的"
        )
        .into());
    }
    println!(
        "  整层 {whole} 件全进帧；默认相机（中心 {center:?}、zoom {zoom}）这一屏 mesh {live} 件 == 门算 {want} 件"
    );

    banner("④ 遮挡带：装饰留在 world 层秩，碎屑低于一切物件、中景低于树");
    let lowest_prop = others
        .iter()
        .filter(|item| item.layer == "world")
        .map(|item| item.order)
        .min()
        .ok_or("场景里没有别的 world sprite，§④ 的\"低于一切物件\"无从比较")?;
    let violations = band_violations(&decor, &others);
    if !violations.is_empty() {
        return Err(format!("层带违例 {} 处，首条：{}", violations.len(), violations[0]).into());
    }
    let actor_rank = layer_rank("actor");
    let world_rank = layer_rank("world");
    if world_rank >= actor_rank {
        return Err("层秩表里 world 不再低于 actor：本节的排序前提已随 depth.lua 一起变了".into());
    }
    // 反面对照：把一件装饰提到树的 order 并放进 actor 带（模拟"让它去遮玩家/屋顶"），
    // 走的必须是**同一个** `band_violations`。
    let mut leaked = decor.clone();
    leaked[0].order = TREE_ORDER;
    leaked[0].layer = "actor".to_owned();
    let leaked_report = band_violations(&leaked, &others);
    if leaked_report.is_empty() {
        return Err(
            "反面对照失手：把一件装饰改成 actor 层 + 树的 order 后 §④ 仍判绿，层带检查是空的"
                .into(),
        );
    }
    println!(
        "  碎屑 {BAND_DEBRIS} < 最靠后的 world sprite {lowest_prop}；中景 {BAND_MID} < 树 {TREE_ORDER}；\
         层秩 world({world_rank}) < actor({actor_rank})"
    );
    println!("  反面对照：装饰进 actor 带即红（{}）", leaked_report[0]);

    banner("⑤ 装饰不参与物理：每件装饰的刚体位置各查一次圆，取不到装饰");
    // 查询点从**运行期**取（实体名 → `entity.find` → `transform.get`），不拿文件里的贴图坐标去推：
    // Rapier 刚体建在 `Transform.position` 上（`scene_spawn.rs:544`），贴图中心还得再过一遍 pivot
    // 与 y 翻转。用后者问碰撞体，测到的是门自己的坐标想象——装饰真长出 collider 也照样绿。
    let names: Vec<&str> = decor.iter().map(|item| item.name.as_str()).collect();
    if names
        .iter()
        .any(|name| !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err("装饰实体名里有非标识符字符，无法安全嵌进 Lua".into());
    }
    let report = lua_str(
        &mut runtime,
        &format!(
            r#"(function()
                 local names = {{{}}}
                 local map = entity.find('GroundMap')
                 if not map or map == 0 then return '-1 0 0 0' end
                 local probed, bad, ground, other = 0, 0, 0, 0
                 for _, name in ipairs(names) do
                   local id = entity.find(name)
                   if id and id ~= 0 then
                     probed = probed + 1
                     local t = transform.get(id)
                     for _, o in ipairs(physics.overlap(t.x, t.y, 4)) do
                       if o == map then
                         ground = ground + 1
                       else
                         local ok, s = pcall(sprite.get, o)
                         if ok and s and string.find(s.texture or '', '{DECOR_PREFIX}', 1, true) then
                           bad = bad + 1
                         else
                           other = other + 1
                         end
                       end
                     end
                   end
                 end
                 return string.format('%d %d %d %d', probed, bad, ground, other)
               end)()"#,
            names
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    )?;
    let [probed, hits, ground, other] = numbers(&report, ["probed", "bad", "ground", "other"])?;
    if probed < 0 {
        return Err(
            "运行期找不到 GroundMap：分不出\"地表的按格碰撞体\"与\"别的东西\"，\
             下面那句 0 就没有对照"
                .into(),
        );
    }
    if probed as usize != decor.len() {
        return Err(format!(
            "只有 {probed}/{} 件装饰在运行期查得到实体，上面的循环其余都在原地跳过：\
             名字对不上（场景里的实体名与文件里的不一致）就等于这条断言没跑",
            decor.len()
        )
        .into());
    }
    if hits != 0 {
        return Err(format!(
            "{hits} 次圆查询在装饰的刚体位置取到了带装饰贴图的实体：装饰长出了碰撞体。玩家会在\
             草地上被一块看不见的石头挡住，而 `solid` 串里根本没有这一格"
        )
        .into());
    }
    // 命中分类要落到"打到的是谁"，不是只数个数：`physics.overlap` 在 manor 里必然打到东西
    // （地表 tilemap 的每个实心格都挂着一个按格生成的静态 cuboid，见 `scene_spawn.rs` 的
    // `solid_cell_colliders`，`jhyt_m72_foreground_occlusion` §④ 逐格探针读数 359/359）。
    // 上一版把这批命中在文案里写成"地表 tilemap 的实心格"，靠的是推断；现在按实体 id 比出来：
    // 归给 GroundMap 的 = ground，剩下的必须为 0，否则就是有第三个碰撞体来源混进装饰堆里。
    if other != 0 {
        return Err(format!(
            "{other} 次命中既不是装饰贴图、也不是 GroundMap：装饰带里冒出了第三个碰撞体来源，\
             本节的分类不再覆盖全部命中，得先认出它是谁"
        )
        .into());
    }
    // 正向对照：同一套查询打到一个真带 collider 的实体上，必须取到**它自己**。
    // 少了这一条，"0 命中"就可能只是查询根本没在跑（pcall 吞错也算一种不跑）。
    let solid = others
        .iter()
        .find(|item| item.collider)
        .ok_or("场景里没有任何带 collider 的 sprite，⑤ 的正向对照无从下手")?;
    if !solid
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!("正向对照实体名 {:?} 不能安全嵌进 Lua", solid.name).into());
    }
    let report = lua_str(
        &mut runtime,
        &format!(
            r#"(function()
                 local id = entity.find('{name}')
                 if not id or id == 0 then return '0 false missing' end
                 local t = transform.get(id)
                 local hits = physics.overlap(t.x, t.y, 4)
                 local mine = false
                 for _, other in ipairs(hits) do
                   if other == id then mine = true end
                 end
                 return string.format('%d %s %s', #hits, tostring(mine), tostring(t.space))
               end)()"#,
            name = solid.name
        ),
    )?;
    let fields: Vec<&str> = report.split_whitespace().collect();
    let hits_on_solid: i64 = fields
        .first()
        .unwrap_or(&"0")
        .parse()
        .map_err(|error| format!("{report:?} 读不回整数：{error}"))?;
    let hit_itself = fields.get(1) == Some(&"true");
    if hits_on_solid == 0 || !hit_itself {
        return Err(format!(
            "正向对照失手：{name} 的刚体位置（Transform，空间 {:?}）取到 {hits_on_solid} 个命中、\
             其中含自身 = {hit_itself}。这条圆查询其实不干活，上面那个 0 是空转出来的",
            fields.get(2).unwrap_or(&"?"),
            name = solid.name
        )
        .into());
    }
    println!(
        "  {} 件装饰各查一次 physics.overlap（都在运行期的刚体位置），命中装饰贴图的次数 = 0；\
         命中别物的 {ground} 次按实体 id 认出全部是 GroundMap（= 实心格按格生成的地表碰撞体，\
         与装饰无关），认不出的 0 次；同一套查询打到 {name} 的刚体位置取到 {hits_on_solid} 个命中\
         且含其自身（正向对照，非空转）",
        decor.len(),
        name = solid.name
    );

    println!(
        "\njhyt_m72_scatter_decor_ok decor={} shapes={} median={median} min={min} max={max} mesh_all={whole} mesh_screen={live} bands={BAND_DEBRIS}/{BAND_MID} physics_hits={hits}",
        decor.len(),
        shapes.len(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// `.scene` 文本读取：门自己解析一遍，与 mesh 互为对照
// ---------------------------------------------------------------------------

fn read_scene(path: &Path) -> Result<Vec<Placed>, Box<dyn std::error::Error>> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut names: BTreeMap<i64, String> = BTreeMap::new();
    let mut sprite_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut transform_of: BTreeMap<i64, String> = BTreeMap::new();
    let mut collider: BTreeSet<i64> = BTreeSet::new();
    let mut roles: BTreeMap<i64, String> = BTreeMap::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        let field: Vec<&str> = line.split('\t').collect();
        if field.len() < 3 {
            continue;
        }
        let Ok(id) = field[1].parse::<i64>() else {
            continue;
        };
        match field[0] {
            "entity" => {
                names.insert(id, field[2].to_owned());
            }
            "component" if field[2] == "Sprite" => {
                sprite_of.insert(id, field[3].to_owned());
            }
            "component" if field[2] == "Transform" => {
                transform_of.insert(id, field[3].to_owned());
            }
            "component" if field[2] == "Collider" => {
                collider.insert(id);
            }
            "entity_meta" if field[2] == "role" => {
                roles.insert(id, field[3].to_owned());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (id, name) in &names {
        let Some(body) = sprite_of.get(id) else {
            continue;
        };
        let Some(transform) = transform_of.get(id) else {
            continue;
        };
        let fields = parse_fields(body);
        let position = parse_fields(transform)
            .get("position")
            .cloned()
            .ok_or_else(|| format!("{name} 的 Transform 没有 position 字段"))?;
        let mut axis = position.split(',');
        let x: f32 = axis.next().unwrap_or("0").parse()?;
        let y: f32 = axis.next().unwrap_or("0").parse()?;
        let size = fields
            .get("size")
            .ok_or_else(|| format!("{name} 的 Sprite 没有 size 字段"))?;
        let mut wh = size.split('x');
        let width: u32 = wh.next().unwrap_or("0").parse()?;
        let height: u32 = wh.next().unwrap_or("0").parse()?;
        let tint = fields
            .get("tint")
            .map(|value| {
                let mut parts = value.split(',').filter_map(|item| item.parse::<f32>().ok());
                [
                    parts.next().unwrap_or(1.0),
                    parts.next().unwrap_or(1.0),
                    parts.next().unwrap_or(1.0),
                ]
            })
            .unwrap_or([1.0, 1.0, 1.0]);
        let is_decor = roles.get(id).map(String::as_str) == Some("decor");
        if is_decor && !name.starts_with("Decor_") {
            return Err(
                format!("实体 {id} 标了 role=decor 但名字是 {name:?}：两套标记不一致").into(),
            );
        }
        out.push(Placed {
            name: name.clone(),
            texture: fields.get("texture").cloned().unwrap_or_default(),
            width,
            height,
            order: fields
                .get("order")
                .and_then(|value| value.parse::<f32>().ok())
                .map_or(0, |value| value as i32),
            layer: fields.get("sorting_layer").cloned().unwrap_or_default(),
            x,
            y,
            tint,
            collider: collider.contains(id),
        });
    }
    Ok(out)
}

fn parse_fields(value: &str) -> BTreeMap<String, String> {
    value
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(key, item)| (key.to_owned(), item.to_owned()))
        .collect()
}

/// 把 Lua 空白分隔的三个读数拆成整数；少一个就报错，**不静默补 0**——
/// 补 0 会把"那段 Lua 根本没跑完"读成"断言成立"。
fn numbers<const N: usize>(
    text: &str,
    labels: [&str; N],
) -> Result<[i64; N], Box<dyn std::error::Error>> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    if fields.len() != N {
        return Err(format!(
            "读回 {fields:?}，应有 {N} 个数（{}）：Lua 那段没跑完",
            labels.join(", ")
        )
        .into());
    }
    let mut values = [0i64; N];
    for (index, (field, label)) in fields.iter().zip(labels).enumerate() {
        values[index] = field
            .parse::<i64>()
            .map_err(|error| format!("{label} 读回 {field:?} 不是整数：{error}"))?;
    }
    Ok(values)
}

fn png_size(project: &Path, path: &str) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(project.join(path)).map_err(|error| {
        format!("{path} 读不到（{error}）：散布指向一张不存在的图，运行时只会静默不画")
    })?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("{path}: {error}"))?
        .to_rgba8();
    Ok((image.width(), image.height()))
}

/// `Decor_MossPatch_31` → `MossPatch`。
fn stem_of(name: &str) -> &str {
    let tail = name.strip_prefix("Decor_").unwrap_or(name);
    match tail.rfind('_') {
        Some(index) if tail[index + 1..].chars().all(|c| c.is_ascii_digit()) => &tail[..index],
        _ => tail,
    }
}

fn catalog_index(name: &str) -> usize {
    CATALOG
        .iter()
        .position(|row| row.0 == stem_of(name))
        .unwrap_or_else(|| panic!("目录里没有 {}", stem_of(name)))
}

fn catalog_of(name: &str) -> Option<&'static (&'static str, &'static str, u32, u32, i32)> {
    CATALOG.iter().find(|row| row.0 == stem_of(name))
}

fn layer_rank(layer: &str) -> i32 {
    LAYER_RANK
        .iter()
        .find(|(name, _)| *name == layer)
        .map(|(_, rank)| *rank)
        .unwrap_or(0)
}

fn tint_bucket(color: [f32; 3]) -> Option<usize> {
    TINT_LEVELS.iter().position(|level| {
        level
            .iter()
            .zip(color.iter())
            .all(|(want, got)| (want - got).abs() < 1e-3)
    })
}

// ---------------------------------------------------------------------------
// 窗口统计
// ---------------------------------------------------------------------------

/// 滑过所有可停靠的相机矩形，返回 `(min, median, max, 最坏窗口左下角)`。
fn sweep_windows(decor: &[Placed]) -> (usize, usize, usize, (i32, i32)) {
    let mut counts = Vec::new();
    let mut worst = (usize::MAX, 0i32, 0i32);
    let mut left = 0.0f32;
    while left + SCREEN_W <= MAP_W + 0.001 {
        let mut bottom = 0.0f32;
        while bottom + SCREEN_H <= MAP_H + 0.001 {
            let hits = count_in_rect(decor, left, bottom);
            if hits < worst.0 {
                worst = (hits, left as i32, bottom as i32);
            }
            counts.push(hits);
            bottom += WINDOW_STEP;
        }
        left += WINDOW_STEP;
    }
    counts.sort_unstable();
    if counts.is_empty() {
        return (0, 0, 0, (0, 0));
    }
    (
        counts[0],
        counts[counts.len() / 2],
        *counts.last().unwrap_or(&0),
        (worst.1, worst.2),
    )
}

/// 与窗口左下角 `(left, bottom)` 的矩形相交的装饰件数（**精确矩形相交**，不是外接圆）。
///
/// 这里不用渲染器那套外接圆判据：§② 要回答的是"玩家看得见几样东西"，一个探进画面
/// 一个角的灌木就该算一件；圆判据会把四角切掉，那是裁剪的性能近似，不是可见性。
fn count_in_rect(decor: &[Placed], left: f32, bottom: f32) -> usize {
    let (right, top) = (left + SCREEN_W, bottom + SCREEN_H);
    decor
        .iter()
        .filter(|item| {
            item.x + (item.width as f32) > left
                && item.x < right
                && item.y + (item.height as f32) > bottom
                && item.y < top
        })
        .count()
}

/// 复刻 `sprite_may_be_visible` 的判据（视口 + 外接圆半径 ×zoom + 1px），用于和 mesh 对账。
///
/// 照抄而不是"更准一点"：这条断言要抓的是"渲染器与门对屏内的理解分叉"，所以连它把
/// pivot 当中心、用外接圆而非矩形这两个近似一起复刻。y 方向的原生翻转不影响结果——
/// 视口区间 `[0, VIEW_H]` 关于中线对称，镜像后判据同真同假。
fn visible_count(decor: &[Placed], center: (f32, f32), zoom: f32) -> usize {
    let half_w = VIEW_W as f32 * 0.5;
    let half_h = VIEW_H as f32 * 0.5;
    decor
        .iter()
        .filter(|item| {
            let x = (item.x - center.0) * zoom + half_w;
            let y = (item.y - center.1) * zoom + half_h;
            let half_x = item.width as f32 * 0.5;
            let half_y = item.height as f32 * 0.5;
            let radius = (half_x * half_x + half_y * half_y).sqrt() * zoom + 1.0;
            x + radius >= 0.0
                && x - radius <= VIEW_W as f32
                && y + radius >= 0.0
                && y - radius <= VIEW_H as f32
        })
        .count()
}

/// 层带违例清单（§④ 的真实断言与反面对照走的是同一个函数）。
fn band_violations(decor: &[Placed], others: &[Placed]) -> Vec<String> {
    let mut out = Vec::new();
    for item in decor {
        if item.layer != "world" {
            out.push(format!(
                "{} 在 {:?} 带（层秩 {}）：装饰一跨层秩就恒压过 world 带里的静态物件，\
                 长在房子北边的灌木会浮在屋顶上",
                item.name,
                item.layer,
                layer_rank(&item.layer)
            ));
            continue;
        }
        if item.order == BAND_DEBRIS {
            let above = others
                .iter()
                .filter(|other| other.layer == "world" && other.order <= item.order)
                .count();
            if above > 0 {
                out.push(format!(
                    "{} 是碎屑档（{}），却有 {above} 件 world sprite 排在它后面：碎屑会盖住物件",
                    item.name, item.order
                ));
            }
        } else if item.order >= TREE_ORDER {
            out.push(format!(
                "{} 的 order = {} 不低于树/竹的 {TREE_ORDER}：灌木会浮在树冠上",
                item.name, item.order
            ));
        } else if item.order != BAND_MID {
            out.push(format!(
                "{} 的 order = {}，不在两档 {BANDS:?} 之内",
                item.name, item.order
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// mesh 与宿主脚手架
// ---------------------------------------------------------------------------

/// 本帧 mesh 里属于装饰的四边形数（按贴图路径前缀认领）。
fn decor_quads(runtime: &mut RuntimeScene) -> Result<usize, Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let mut quads = 0;
    for draw in &mesh.draws {
        if !is_decor_path(&mesh, draw.texture) {
            continue;
        }
        quads += (draw.count as usize) / 6;
    }
    for draw in &mesh.world_sprite_draws {
        if !is_decor_path(&mesh, draw.texture_id) {
            continue;
        }
        quads += draw.vertex_count / 6;
    }
    Ok(quads)
}

fn is_decor_path(mesh: &SceneMesh, texture: u32) -> bool {
    if texture == 0 {
        return false;
    }
    mesh.paths
        .get((texture - 1) as usize)
        .is_some_and(|path| path.starts_with(DECOR_PREFIX))
}

fn camera(runtime: &mut RuntimeScene) -> Result<((f32, f32), f32), Box<dyn std::error::Error>> {
    let text = lua_str(
        runtime,
        r#"(function()
             local c = camera2d.get()
             return string.format('%0.3f,%0.3f,%0.4f', c.x, c.y, camera2d.get_zoom())
           end)()"#,
    )?;
    let mut parts = text.split(',');
    let take = |part: Option<&str>| -> Result<f32, Box<dyn std::error::Error>> {
        Ok(part.unwrap_or("0").trim().parse::<f32>()?)
    };
    let x = take(parts.next())?;
    let y = take(parts.next())?;
    let zoom = take(parts.next())?;
    Ok(((x, y), zoom))
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W, VIEW_H)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

fn assert_scene(
    runtime: &mut RuntimeScene,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = lua_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

fn lua_run(runtime: &mut RuntimeScene, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime
        .with_script(|script| script.execute_console(code))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))
        .map_err(|error| error.to_string())?;
    Ok(text.trim().to_owned())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
