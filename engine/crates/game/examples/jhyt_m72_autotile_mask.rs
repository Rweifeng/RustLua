//! Headless acceptance drive for M72-02：**4-bit autotile 掩码与它的图集条目布局**。
//!
//! 需求（`tasks/done/M72-02-per-tile-tint与确定性变体与autotile.md`）：地形交界是直线 = 每种地形
//! 只有一格瓦。autotile 让格子里存的始终是「逻辑瓦 id」（草 / 土），交界取哪一格由**邻居掩码**
//! 当场解出。这条门钉的是：位序（艺术契约）、掩码 → 条目 → UV 这一路、以及"内部格不许取压边瓦"。
//!
//! 为什么要手写一张掩码表："4 位各自代表哪个方向"是图集生成器与运行时之间那份**说不清就全体
//! 错位**的约定；跑两帧看像不像回答不了它。首版刻意只做 4 位 = 16 变体：星露谷那种 47 变体的
//! 不规则犬牙岸线要 8 位掩码或手工装饰层，设计内做不到（见 plan 的 §设计要点 2）。
//!
//! 章节：
//!   ① 8×8 草地嵌 4×4 土块：16 格土逐格对手写掩码表；取到 9 种掩码、四角四个不同值 → 位序钉死；
//!   ② 土块交界恰好 12 格；草侧贴边格各缺且只缺"指向土块那一侧"的一位；掩码 0b1111 的内部格必须
//!      取块内最后一格（满块瓦）——交界以外**不许**出现压边瓦；
//!   ③ 条目布局 `entry = 1 + kinds*span + (kind-1)*16 + mask`：草块 3..=18、土块 19..=34 互不越界；
//!   ④ 反面对照（真的跑出来，不是嘴上说）：(a) 抹掉 `y-1` 一位 → 变化的集合必须**含土块顶行全部 4 格**
//!      且不含底行（plan 点名的"红在顶部那一排交界格"）；(b) 抹掉 `y+1` 一位 → 顶行必须**一格都不在**
//!      变化集合里（这一位管的不是这一侧）；(c) 交换 `x+1`/`x-1` 两位 → 顶行 2 格错位（另两角对称）；
//!   ⑤ 地图边 = 暴露边界：越界当"没有邻居"，四角读 0b0110 / 0b1100 / 0b0011 / 0b1001；
//!   ⑥ 挖一格空洞：本格条目变 0，右邻与上邻各恰好少掉指向它的那一位；填回去逐格复原；
//!   ⑦ 一路到渲染：真把这张图画出来，每格 UV 必须等于「手写掩码 → 条目 → 12×8 子矩形」的算术结果；
//!   ⑧ **已退役（M73）**：原判据是跨宿主同值——把 `plugins/webgl/web-runtime.js` 内嵌的 Lua
//!      解算镜像喂进真 VM，对同一张图重解 64 格的掩码与条目，并用"全草图 36 个内部格必须是
//!      0b1111 + 满块瓦"当反面对照。M73 删掉旧 JS 世界后仓库里只剩引擎这一份实现
//!      （`plugins/` 下再无瓦片解算代码），对账没有第二个边可比，读文件只会得到 `os error 2`。
//!      整段删除而不是放宽成"文件不存在就跳过"——跳过等于让这道门在没有任何对账时报绿。
//!      重新立它的前提是**先有第二份实现**（新宿主若再带自己的解算器，把那段文本原样喂回来）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_autotile_mask [-- <game_jhyt-root>]

use engine::host::{RuntimeScene, SceneMesh};
use rustlua_input::input::InputSnapshot;
use std::{env, path::PathBuf};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
const PROBE_TILESET: &str = "assets/art/tilesets/m72/probe_autotile.png";
const WIDTH: usize = 8;
const HEIGHT: usize = 8;
const CELLS: usize = WIDTH * HEIGHT;
const ATLAS_COLS: usize = 12;
const ATLAS_ROWS: usize = 8;
const KINDS: usize = 2;
const SPAN: usize = 1;
const SLOTS: usize = 16;
const GRASS: usize = 1;
const DIRT: usize = 2;
/// 土块范围（含端点）：x 2..=5、y 2..=5 → 16 格、内芯 2×2、交界 12 格。
const INSET_X0: usize = 2;
const INSET_Y0: usize = 2;
const INSET_X1: usize = 5;
const INSET_Y1: usize = 5;
const PROBE: &str = "__m72_autotile_probe";
/// §④(c) 的暗号：`ignore` 里出现它表示"把两个列位互换"，而不是"抹掉某一位"。
const SWAP_COLUMNS: u8 = 9;

fn banner(title: &str) {
    println!("\n== {title} ==");
}

/// 门自己那份地图（与写进引擎的内容同源；§④ 的对照物要在纯数据侧跑，不能反过来问引擎要）。
fn cell_at(x: usize, y: usize) -> usize {
    if (INSET_X0..=INSET_X1).contains(&x) && (INSET_Y0..=INSET_Y1).contains(&y) {
        DIRT
    } else {
        GRASS
    }
}

/// 手写掩码参考实现：bit0=`y-1`、bit1=`x+1`、bit2=`y+1`、bit3=`x-1`（**按 index 命名**——地图
/// row 0 在最底、图集 row 0 在最顶，同一句"上邻居"在两个坐标系里指向相反方向）。
/// `ignore` 里列出的位当作"那个方向永远没有邻居"，这是 §④ 反面对照的开关。
fn reference_mask(x: usize, y: usize, own: usize, ignore: &[u8]) -> u8 {
    let swap = ignore.contains(&SWAP_COLUMNS);
    let mut mask = 0u8;
    for (bit, (dx, dy)) in [
        (0u8, (0isize, -1isize)),
        (1, (1, 0)),
        (2, (0, 1)),
        (3, (-1, 0)),
    ] {
        if ignore.contains(&bit) && !swap {
            continue;
        }
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if nx < 0 || ny < 0 || nx as usize >= WIDTH || ny as usize >= HEIGHT {
            continue;
        }
        if own == 0 {
            continue;
        }
        if cell_at(nx as usize, ny as usize) != own {
            continue;
        }
        // 交换列位：x+1 的结果写进 bit3、x-1 的写进 bit1。
        let target = if swap && (bit == 1 || bit == 3) {
            4 - bit
        } else {
            bit
        };
        mask |= 1 << target;
    }
    mask
}

/// 满块瓦（掩码 0b1111）该落在的条目：kind 自己那 16 格过渡块的最后一格。
const fn want_full_slot(kind: usize) -> u32 {
    1 + (KINDS * SPAN) as u32 + (kind as u32 - 1) * SLOTS as u32 + 15
}

const fn want_entry(kind: usize, mask: u8) -> u32 {
    1 + (KINDS * SPAN) as u32 + (kind as u32 - 1) * SLOTS as u32 + mask as u32
}

fn dirt_cells() -> Vec<(usize, usize)> {
    (INSET_Y0..=INSET_Y1)
        .flat_map(|y| (INSET_X0..=INSET_X1).map(move |x| (x, y)))
        .collect()
}

/// 哪几个方向的邻居是土：位序与 `reference_mask` 一致（bit0=`y-1`、bit1=`x+1`、bit2=`y+1`、bit3=`x-1`）。
fn dirt_neighbour_bits(x: usize, y: usize) -> u8 {
    let mut bits = 0u8;
    for (bit, (dx, dy)) in [
        (0u8, (0isize, -1isize)),
        (1, (1, 0)),
        (2, (0, 1)),
        (3, (-1, 0)),
    ] {
        let (nx, ny) = (x as isize + dx, y as isize + dy);
        if nx < 0 || ny < 0 || nx as usize >= WIDTH || ny as usize >= HEIGHT {
            continue;
        }
        if cell_at(nx as usize, ny as usize) == DIRT {
            bits |= 1 << bit;
        }
    }
    bits
}

/// 一格图集条目在 12×8 图集里该占的子矩形（与 `Tilemap::atlas_uv` 同一套算术，故意各写一遍）。
type UvQuad = ([f32; 2], [f32; 2]);

fn expected_uv(entry: u32) -> UvQuad {
    let position = entry as f32 - 1.0;
    let col = position % ATLAS_COLS as f32;
    let row = (position / ATLAS_COLS as f32).floor();
    (
        [col / ATLAS_COLS as f32, row / ATLAS_ROWS as f32],
        [
            (col + 1.0) / ATLAS_COLS as f32,
            (row + 1.0) / ATLAS_ROWS as f32,
        ],
    )
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
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    // 剔除关掉才是"整层全画"档：§⑦ 要比的是**每一格**的 UV，相机外的格子读不到就没有证据。
    runtime.set_tilemap_culling(false);

    let probe = spawn_probe(&mut runtime)?;
    let cells: Vec<usize> = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| cell_at(x, y)))
        .collect();
    let masks = masks_of(&mut runtime, probe)?;
    let entries = entries_of(&mut runtime, probe)?;
    let want: Vec<u8> = (0..CELLS)
        .map(|index| reference_mask(index % WIDTH, index / WIDTH, cells[index], &[]))
        .collect();

    banner("① 16 格土块逐格对手写掩码表（位序 = 艺术契约）");
    let wrong: Vec<(usize, usize, u8, u8)> = (0..CELLS)
        .filter(|index| masks[*index] != want[*index])
        .map(|index| (index % WIDTH, index / WIDTH, masks[index], want[index]))
        .take(6)
        .collect();
    if !wrong.is_empty() {
        return Err(format!(
            "{}/{} 格的邻居掩码与手写表不符，(x,y) 引擎/预期 = {wrong:?}——位序（bit0=y-1、bit1=x+1、\
             bit2=y+1、bit3=x-1）就是图集生成器与运行时之间那份约定，错一位交界就画到别的方向上"
        , (0..CELLS).filter(|i| masks[*i] != want[*i]).count(), CELLS)
            .into());
    }
    let dirt: Vec<u8> = dirt_cells()
        .iter()
        .map(|(x, y)| masks[y * WIDTH + x])
        .collect();
    let mut distinct = dirt.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let corners = [
        masks[INSET_Y0 * WIDTH + INSET_X0],
        masks[INSET_Y0 * WIDTH + INSET_X1],
        masks[INSET_Y1 * WIDTH + INSET_X0],
        masks[INSET_Y1 * WIDTH + INSET_X1],
    ];
    if corners
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != 4
    {
        return Err(format!(
            "土块四角掩码 = {corners:?}，四个角本该各取一个转角瓦——同了就说明两位被并成了一位"
        )
        .into());
    }
    println!(
        "  {} 格全部对上；土块 16 格取到 {} 种掩码 {distinct:?}；四角 {corners:?}",
        CELLS,
        distinct.len()
    );

    banner("② 交界 12 格取转角/T 形，内部格取满块瓦");
    let interior_dirt: Vec<(usize, usize)> = (INSET_Y0 + 1..INSET_Y1)
        .flat_map(|y| (INSET_X0 + 1..INSET_X1).map(move |x| (x, y)))
        .collect();
    for (x, y) in &interior_dirt {
        if masks[y * WIDTH + x] != 0b1111 {
            return Err(format!(
                "土块内芯 ({x},{y}) 掩码 = {:04b}，四邻都是土应为 0b1111",
                masks[y * WIDTH + x]
            )
            .into());
        }
    }
    let dirt_border: usize = dirt.iter().filter(|mask| **mask != 0b1111).count();
    if dirt_border != 12 {
        return Err(
            format!("4×4 土块的交界格应恰好 12 格（16 减 2×2 内芯），实测 {dirt_border}").into(),
        );
    }
    let pressed_interior: Vec<(usize, usize, u32)> = (0..CELLS)
        .filter(|index| masks[*index] == 0b1111 && entries[*index] != want_full_slot(cells[*index]))
        .map(|index| (index % WIDTH, index / WIDTH, entries[index]))
        .collect();
    if !pressed_interior.is_empty() {
        return Err(format!(
            "{} 格四邻同 kind 的内部格没取到块内最后一格（压边瓦出现在内部）：{:?}",
            pressed_interior.len(),
            &pressed_interior[..pressed_interior.len().min(4)]
        )
        .into());
    }
    // 草侧被土块压掉位的格子：缺的必须**正好是指向土块那一边**的一位，其余三位一格不许动。
    // （早先这里统计的是"所有 3 位草格"，那个数把地图边的草格也算进来了，读起来像证据其实是噪声。）
    let mut grass_against_dirt = 0usize;
    let mut grass_interior_full = 0usize;
    for index in 0..CELLS {
        let (x, y) = (index % WIDTH, index / WIDTH);
        if cells[index] != GRASS {
            continue;
        }
        let dirt_bits = dirt_neighbour_bits(x, y);
        let base = reference_mask(x, y, GRASS, &[]);
        if dirt_bits == 0 {
            if base == 0b1111 {
                if masks[index] != 0b1111 {
                    return Err(format!(
                        "内部草格 ({x},{y}) 掩码 = {:04b}，四邻全是草应为 0b1111（满块瓦）",
                        masks[index]
                    )
                    .into());
                }
                grass_interior_full += 1;
            }
            continue;
        }
        if dirt_bits.count_ones() != 1 {
            return Err(format!(
                "草格 ({x},{y}) 同时贴着 {} 格土：一位掩码在这里表达不了，探针地图得重画",
                dirt_bits.count_ones()
            )
            .into());
        }
        if masks[index] != base & !dirt_bits {
            return Err(format!(
                "草格 ({x},{y}) 贴着土块，掩码 = {:04b}，应为 {:04b}——只许缺指向土块那一位，\
                 多缺或错缺都说明交界取的是另一侧的压边瓦",
                masks[index],
                base & !dirt_bits
            )
            .into());
        }
        grass_against_dirt += 1;
    }
    if grass_against_dirt != 16 {
        return Err(format!(
            "草侧贴土块的格子应为 16（4×4 土块四边各 4 格），实测 {grass_against_dirt}"
        )
        .into());
    }
    println!(
        "  内芯 {} 格 = 0b1111；土块交界 {dirt_border} 格；草侧 {grass_against_dirt} 格各缺且只缺指向土块的\
         那 1 位，另有 {grass_interior_full} 格纯内部草格 = 0b1111；满块瓦条目 = 块内最后一格（草 {:?}、土 {:?}）",
        interior_dirt.len(),
        want_full_slot(GRASS),
        want_full_slot(DIRT)
    );

    banner("③ 条目布局：kind 各占 16 格过渡块，两块互不越界");
    for kind in [GRASS, DIRT] {
        let block: Vec<u32> = (0..CELLS)
            .filter(|index| cells[*index] == kind)
            .map(|index| entries[index])
            .collect();
        let lo = block.iter().min().copied().unwrap_or(0);
        let hi = block.iter().max().copied().unwrap_or(0);
        let want_lo = want_entry(kind, 0);
        let want_hi = want_entry(kind, 15);
        if lo < want_lo || hi > want_hi {
            return Err(format!(
                "kind {kind} 的条目实测 {lo}..={hi}，越出它自己的过渡块 {want_lo}..={want_hi}——\
                 越过一格就取到别的地形的压边瓦，图集生成器没法排版"
            )
            .into());
        }
        if let Some(index) = (0..CELLS).position(|index| {
            cells[index] == kind && entries[index] != want_entry(kind, masks[index])
        }) {
            let (x, y) = (index % WIDTH, index / WIDTH);
            return Err(format!(
                "({x},{y}) 条目 = {}，按 `1 + kinds*span + (kind-1)*16 + mask` 应为 {}",
                entries[index],
                want_entry(kind, masks[index])
            )
            .into());
        }
        println!("  kind {kind}: 实测 {lo}..={hi}，块界 {want_lo}..={want_hi}，逐格满足布局式");
    }
    if entries.contains(&0) {
        return Err("满图无空格，却有格子解出条目 0（什么都不画）".into());
    }

    banner("④ 反面对照：抹掉某一位 / 互换两位，红格必须落在指定那一排");
    let (drop_prev, top_prev, bottom_prev) = changed_cells(&masks, &cells, &[0]);
    let (drop_next, top_next, _) = changed_cells(&masks, &cells, &[2]);
    let (_, top_swap, _) = changed_cells(&masks, &cells, &[SWAP_COLUMNS]);
    if top_prev != 4 || bottom_prev != 0 {
        return Err(format!(
            "抹掉 bit0（y-1 那一位）后土块顶行变了 {top_prev}/4 格、底行变了 {bottom_prev} 格（应为 4 与 0）：\
             plan 点名的『红在顶部那一排交界格』没红，说明门对这一位其实是瞎的"
        )
        .into());
    }
    if top_next != 0 {
        return Err(format!(
            "抹掉 bit2（y+1）后土块顶行竟变了 {top_next} 格：bit2 管的不是索引上方这一侧，\
             位序命名与实现不符"
        )
        .into());
    }
    if top_swap != 2 {
        return Err(format!(
            "把两个列位互换后土块顶行变了 {top_swap} 格（应为 2：两个真角格错位，中间两格本就对称）——\
             列位没被单独看着，左右压边瓦会互换"
        )
        .into());
    }
    println!(
        "  (a) 抹掉 y-1：{drop_prev} 格变，土块顶行 4/4 全变、底行 0 格（门红在顶排交界格）\n    \
         (b) 抹掉 y+1：{drop_next} 格变，土块顶行 0 格（这一位管的是另一侧）\n    \
         (c) 交换 x+1/x-1：土块顶行 {top_swap} 格错位"
    );

    banner("⑤ 地图边 = 暴露边界（越界当没有邻居）");
    let edge = [
        ((0, 0), 0b0110u8),
        ((WIDTH - 1, 0), 0b1100),
        ((0, HEIGHT - 1), 0b0011),
        ((WIDTH - 1, HEIGHT - 1), 0b1001),
    ];
    for ((x, y), want_mask) in edge {
        if masks[y * WIDTH + x] != want_mask {
            return Err(format!(
                "地图角格 ({x},{y}) 掩码 = {:04b}，应为 {want_mask:04b}——地图边不是第三种情况，\
                 它就是没有邻居（不这样处理，整张图的边缘会围上一圈不存在的过渡瓦）",
                masks[y * WIDTH + x]
            )
            .into());
        }
    }
    println!(
        "  四角 {:04b}/{:04b}/{:04b}/{:04b}（左下角形与土块角同形：暴露的就是暴露的）",
        masks[0],
        masks[WIDTH - 1],
        masks[(HEIGHT - 1) * WIDTH],
        masks[CELLS - 1]
    );

    banner("⑥ 挖一格空洞：本格不画，两个邻居各少一位");
    let before_right = masks[1];
    let before_above = masks[WIDTH];
    lua_run(&mut runtime, &format!("tilemap.set_cell({PROBE}, 0, 0, 0)"))?;
    frame(&mut runtime)?;
    let holed_entries = entries_of(&mut runtime, probe)?;
    let holed_masks = masks_of(&mut runtime, probe)?;
    if holed_entries[0] != 0 {
        return Err(format!(
            "空格 (0,0) 解出条目 {}，应为 0（什么都不画）",
            holed_entries[0]
        )
        .into());
    }
    if holed_masks[1] != before_right & !(1 << 3) {
        return Err(format!(
            "把 (0,0) 挖空后右邻 (1,0) 的掩码是 {:04b}，应从 {before_right:04b} 恰好少掉 x-1 那一位 \
             变成 {:04b}",
            holed_masks[1],
            before_right & !(1 << 3)
        )
        .into());
    }
    if holed_masks[WIDTH] != before_above & !(1 << 0) {
        return Err(format!(
            "把 (0,0) 挖空后上邻 (0,1) 的掩码是 {:04b}，应从 {before_above:04b} 恰好少掉 y-1 那一位 \
             变成 {:04b}",
            holed_masks[WIDTH],
            before_above & !(1 << 0)
        )
        .into());
    }
    println!(
        "  (0,0) 条目 0；(1,0) 掩码 {before_right:04b}→{:04b}，(0,1) {before_above:04b}→{:04b}",
        holed_masks[1], holed_masks[WIDTH]
    );
    lua_run(
        &mut runtime,
        &format!("tilemap.set_cell({PROBE}, 0, 0, {GRASS})"),
    )?;
    frame(&mut runtime)?;
    if entries_of(&mut runtime, probe)? != entries {
        return Err("填回草格之后条目序列与挖空之前不同：块脏重建没把这一格恢复到原状".into());
    }
    println!("  填回后 64 格条目序列与挖空之前逐格全等（脏重建可逆）");

    banner("⑦ 一路到渲染：UV 就是「手写掩码 → 条目 → 12×8 子矩形」");
    let quads = quads_of(&mut runtime, PROBE_TILESET)?;
    if quads.len() != CELLS {
        return Err(format!(
            "{PROBE_TILESET} 画了 {} 格，应为 {CELLS} 格（地图无空格）",
            quads.len()
        )
        .into());
    }
    if let Some(index) = (0..CELLS)
        .position(|index| quads[index] != expected_uv(want_entry(cells[index], want[index])))
    {
        let (x, y) = (index % WIDTH, index / WIDTH);
        return Err(format!(
            "({x},{y}) 的 UV 不是条目 {:?} 该占的子矩形：掩码解出的条目与真画出来的不是同一格",
            want_entry(cells[index], want[index])
        )
        .into());
    }
    println!(
        "  {} 格 UV 逐格对上（草 {} 格 + 土 {} 格，各走自己那 16 格过渡块）",
        quads.len(),
        cells.iter().filter(|kind| **kind == GRASS).count(),
        cells.iter().filter(|kind| **kind == DIRT).count()
    );

    banner("⑧ web 的 Lua 垫片同解：随 M73 删除旧 JS 世界而退役");
    // 原 §⑧ 把 `plugins/webgl/web-runtime.js` 内嵌的 Lua 解算镜像喂进真 VM，让它对同一张图重解
    // 64 格的掩码与条目，再拿"全草图 36 个内部格必须是 0b1111 + 满块瓦"当反面对照。M73 删掉旧
    // JS 世界后仓库里只剩引擎这一份实现（`plugins/` 下再无瓦片解算代码），这条对账没有第二个边
    // 可比；继续读那个文件只会得到 `os error 2`。整段删除而不是改成"文件不存在就跳过"——
    // 跳过会让这道门在没有任何对账的情况下报绿。要重新立它，前提是先有第二份实现。

    println!(
        "\njhyt_m72_autotile_mask_ok project={} cells={CELLS} dirt_border=12 dirt_interior=4 mask_kinds={} \
         web_mirror=retired(M73)",
        project.display(),
        distinct.len()
    );
    Ok(())
}

/// 用"抹掉/互换某些位"的参考实现重算掩码，返回（变化格数、土块顶行变化格数、底行变化格数）。
fn changed_cells(masks: &[u8], cells: &[usize], ignore: &[u8]) -> (usize, usize, usize) {
    let mut changed = 0;
    let mut top = 0;
    let mut bottom = 0;
    for index in 0..CELLS {
        let (x, y) = (index % WIDTH, index / WIDTH);
        if reference_mask(x, y, cells[index], ignore) == masks[index] {
            continue;
        }
        changed += 1;
        if !(INSET_X0..=INSET_X1).contains(&x) {
            continue;
        }
        if y == INSET_Y1 {
            top += 1;
        }
        if y == INSET_Y0 {
            bottom += 1;
        }
    }
    (changed, top, bottom)
}

/// 8×8 草地嵌 4×4 土：格子里存**逻辑瓦 id**（1=草、2=土），图集 12×8（容量 96 ≥ 2×(1+16)）。
fn spawn_probe(runtime: &mut RuntimeScene) -> Result<i64, Box<dyn std::error::Error>> {
    let cells = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| format!("tilemap.set_cell({PROBE}, {x}, {y}, {})", cell_at(x, y)))
        .collect::<Vec<_>>()
        .join("\n           ");
    let script = format!(
        r#"{PROBE} = tilemap.create({WIDTH}, {HEIGHT}, 32, '{PROBE_TILESET}', 'M72AutotileProbe')
           tilemap.set_atlas({PROBE}, {ATLAS_COLS}, {ATLAS_ROWS})
           tilemap.set_kinds({PROBE}, {KINDS}, {SPAN}, true, {SEED})
           {cells}
           return {PROBE}"#
    );
    lua_run(runtime, &script)?;
    frame(runtime)?;
    lua_id(runtime, PROBE)
}

/// autotile 地图不需要变体种子；这里固定一个非零值，让"种子进了组件串"这件事在载荷里可见。
const SEED: u32 = 0x5EED_0002;

fn masks_of(runtime: &mut RuntimeScene, id: i64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let text = lua_batch(runtime, id, "autotile_mask")?;
    let values: Vec<u8> = text
        .split(',')
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .collect();
    if values.len() != CELLS {
        return Err(format!("掩码序列读回 {text:?}，长度 {}，应为 {CELLS}", values.len()).into());
    }
    Ok(values)
}

fn entries_of(runtime: &mut RuntimeScene, id: i64) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let text = lua_batch(runtime, id, "atlas_index")?;
    let values: Vec<u32> = text
        .split(',')
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .collect();
    if values.len() != CELLS {
        return Err(format!("条目序列读回 {text:?}，长度 {}，应为 {CELLS}", values.len()).into());
    }
    Ok(values)
}

/// 一次 Lua 往返取全图（逐格 round-trip 会把门自己拖成瓶颈）。
fn lua_batch(
    runtime: &mut RuntimeScene,
    id: i64,
    func: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    lua_str(
        runtime,
        &format!(
            r#"(function()
                 local out = {{}}
                 for y = 0, {HEIGHT} - 1 do
                   for x = 0, {WIDTH} - 1 do
                     out[#out + 1] = tilemap.{func}({id}, x, y)
                   end
                 end
                 return table.concat(out, ',')
               end)()"#
        ),
    )
}

fn quads_of(
    runtime: &mut RuntimeScene,
    path: &str,
) -> Result<Vec<UvQuad>, Box<dyn std::error::Error>> {
    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let overlay: Vec<(usize, usize)> = mesh
        .draws
        .iter()
        .filter(|draw| path_of(&mesh, draw.texture) == Some(path))
        .map(|draw| (draw.start as usize, draw.count as usize))
        .collect();
    let world: Vec<(usize, usize)> = mesh
        .world_sprite_draws
        .iter()
        // 世界通道的 `texture_id` 是 `paths` 的 0 基下标；overlay 的 `SceneDraw.texture` 才是
        // 1 基（0 = 内建白图，见 `frame_data.rs:816` 的 `+1`）。两套编号不能混用。
        .filter(|draw| mesh.paths.get(draw.texture_id as usize).map(String::as_str) == Some(path))
        .map(|draw| (draw.vertex_start, draw.vertex_count))
        .collect();
    if !overlay.is_empty() && !world.is_empty() {
        return Err(format!("{path} 同时出现在 overlay 与世界深度通道——瓦片层只该走一条").into());
    }
    let uvs: Vec<[f32; 2]> = if overlay.is_empty() {
        mesh.world_sprites.iter().map(|vertex| vertex.uv).collect()
    } else {
        mesh.vertices.iter().map(|vertex| vertex.uv).collect()
    };
    let mut out = Vec::new();
    for (start, count) in if overlay.is_empty() { world } else { overlay } {
        for quad in uvs[start..start + count].as_chunks::<6>().0.iter() {
            out.push((quad[0], quad[2]));
        }
    }
    Ok(out)
}

fn path_of(mesh: &SceneMesh, texture: u32) -> Option<&str> {
    if texture == 0 {
        return None;
    }
    mesh.paths.get((texture - 1) as usize).map(String::as_str)
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W, VIEW_H)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
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

/// 实体 id 按整数读回（带代际的大整数，走 f32 会掉精度）。
fn lua_id(runtime: &mut RuntimeScene, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, expr)?;
    text.parse::<i64>()
        .map_err(|_| format!("{expr} 不是实体 id（读回 {text:?}）").into())
}
