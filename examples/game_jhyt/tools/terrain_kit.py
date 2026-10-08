# -*- coding: utf-8 -*-
"""地表图集的**契约层**：色阶表 + autotile 条目排布 + 出图后处理。

M72-02 把引擎侧"运行时怎么解"钉死了（`resolve_atlas_index`），但图集**怎么排**与
**用哪些色**是同一件事的另一半，当时没有输入所以没建这个文件（见 M72-02 §未完成项）。
M72-04 起，出图侧（`tools/gen_terrain_tiles.py`）与验收侧（Rust 门
`jhyt_m72_manor_terrain`）都从 `tools/terrain_ramp.json` 这一张表取色、按本文件的
条目算术寻址——**生成器和运行时不同构是这一路最贵的错**（交界整体错一个方向，
而按数据画图的 `preview_scene.py` 看不出来）。

三件事分别由三个函数钉：

  * `entry_base` / `entry_transition`：与 Rust `Tilemap::resolve_atlas_index` 逐符号对应；
  * `mask_edges`：把 4-bit 掩码翻成"哪几条边暴露"，位序**按 index 命名**（bit0 = `y-1`），
    地图 row 0 在最底、图集 row 0 在最顶，所以 bit0 画的是瓦片的**下**沿；
  * `quantize_to_ramp`：出图后处理，alpha 二值化（>127→255，与 docs/57 的 RGBA8 硬指标
    同源）+ 色阶表外颜色一律就近吸附并报数（门要求报 0）。

用法：`import terrain_kit as tk` 于同目录工具中；纯标准库。
"""

import json
import os

from pixel_kit import Canvas, rgb

RAMP_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "terrain_ramp.json")

# 与 `rustlua-ecs/src/tilemap.rs` 的同名常量一一对应。
AUTOTILE_SLOTS = 16

# 掩码位序（bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1）→ 该位对应瓦片图像上的哪条边。
# 地图 row 0 在**最底**而图集 row 0 在**最顶**：所以 y-1 是下沿、y+1 是上沿。
# 与 `plugins/webgl/web-runtime.js`、`tools/preview_scene.py`、Rust 四处必须同序。
EDGE_BY_BIT = {
    0: "bottom",  # y-1
    1: "right",   # x+1
    2: "top",     # y+1
    3: "left",    # x-1
}


def load_ramp(path=RAMP_PATH):
    """读色阶表（唯一取色来源）。返回原表 dict，另附解析好的 `*_rgb` 字段。"""
    with open(path, encoding="utf-8") as handle:
        table = json.load(handle)
    if table.get("version") != 1:
        raise ValueError(f"terrain_ramp.json version 不认：{table.get('version')!r}")
    table["tile"] = int(table["tile_px"])
    table["cols"] = int(table["atlas"]["cols"])
    table["rows"] = int(table["atlas"]["rows"])
    table["kind_count"] = len(table["kinds"])
    return table


def family_levels(table, family):
    """某材质族的色阶（深→浅，RGB 元组列表）。表里没有就报错，不许悄悄兜底。"""
    levels = table["families"].get(family)
    if not levels:
        raise KeyError(f"色阶表里没有材质族 {family!r}")
    return [rgb(hex_value) for hex_value in levels]


def family_hexes(table, family):
    levels = table["families"].get(family)
    if not levels:
        raise KeyError(f"色阶表里没有材质族 {family!r}")
    return list(levels)


def luminance(color):
    """sRGB 相对亮度（门用它复核色阶表"深→浅"是不是真的单调）。"""
    return 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]


def ramp_is_monotonic(table, family):
    levels = family_levels(table, family)
    return all(luminance(levels[i]) < luminance(levels[i + 1]) for i in range(len(levels) - 1)), len(levels)


def allowed_colors(table):
    """允许出现在地表图集中的全部不透明色：六族色阶 + 通用墨/纸/金/木。

    返回 **RGB 三元组**集合：PNG 里读出来的像素是 RGBA，比对一律先切 `[:3]`，
    这里若带 alpha 就会变成"每个不透明像素都越表"的假阳性。
    """
    colors = set()
    for levels in table["families"].values():
        for hex_value in levels:
            colors.add(rgb(hex_value)[:3])
    for hex_value in table["utility"].values():
        colors.add(rgb(hex_value)[:3])
    return colors


def kind_of(table, kind_id):
    for entry in table["kinds"]:
        if int(entry["id"]) == kind_id:
            return entry
    raise KeyError(f"色阶表里没有 kind {kind_id}")


def entry_base(table, kind_id):
    """kind 的**满块瓦**条目号（1 基）= 恒等档 `tiles==id` 直接取用的那一格。

    对应 Rust：`1 + (kind-1) * variant_span`，本批 `variant_count=1`。autotile 开着时
    运行时不会走到这里，但它是"关掉 autotile 立刻回滚"的那一档，也是 combat_arena
    复用同一张图集时按恒等寻址取用的那 6 格，所以必须有真美术。
    """
    return 1 + (kind_id - 1)


def entry_transition(table, kind_id, mask):
    """kind 在某个 4-bit 邻居掩码下的过渡块条目号（1 基）。

    对应 Rust：`entry = 1 + kinds*span + (kind-1)*AUTOTILE_SLOTS + mask%16`。
    """
    span = 1
    return 1 + table["kind_count"] * span + (kind_id - 1) * AUTOTILE_SLOTS + (mask % AUTOTILE_SLOTS)


def entry_rect(table, entry):
    """条目号 → 图集内的 (col, row) 子矩形行列号（0 基，row 0 在贴图**顶部**）。"""
    position = entry - 1
    return position % table["cols"], position // table["cols"]


def entry_region(canvas, table, entry):
    """从整张图集 `canvas` 里裁出某个条目（32×32 的独立 Canvas）。

    Canvas 的 y 就是 PNG 的行号（row 0 在上），与 `entry_rect` 同一套朝向，所以直接
    `row * tile + y` 取像素。**预览、对拍、季节变体都走这里**，别各自再写一遍算式——
    四处 UV 复刻里任何一处写反，预览图就和实机不一致。
    """
    tile = table["tile"]
    col, row = entry_rect(table, entry)
    patch = Canvas(tile, tile)
    for y in range(tile):
        for x in range(tile):
            patch.set(x, y, canvas.get(col * tile + x, row * tile + y))
    return patch


def mask_edges(mask):
    """掩码 → 暴露边集合。位为 0 = 那个方向没有同类邻居（或是地图边）= 该边要画过渡。"""
    edges = set()
    for bit, edge in EDGE_BY_BIT.items():
        if not (mask >> bit) & 1:
            edges.add(edge)
    return edges


def transition_layout(table, kind_id):
    """该 kind 的 16 格过渡块分别落在哪些条目、对应哪个掩码（[(entry, mask, edges)]）。"""
    rows = []
    for mask in range(AUTOTILE_SLOTS):
        rows.append((entry_transition(table, kind_id, mask), mask, sorted(mask_edges(mask))))
    return rows


def capacity_check(table):
    """图集容量对账，与 Rust `validate_layout` 同一口径：needed = kinds*(1+16)。"""
    needed = table["kind_count"] * (1 + AUTOTILE_SLOTS)
    capacity = table["cols"] * table["rows"]
    return needed, capacity


def ground_tints(table):
    """per-tile tint 调色表 `[(tint_id, [r,g,b,a]), ...]`。

    引擎的 tint id 是 1 基连续编号（`tints=格号:档号`，档号进 `tints_rgb` 查表），所以
    表里写乱序或跳号必须在读的时候就炸，而不是等到运行期画成白格。
    """
    out = []
    for index, entry in enumerate(table.get("ground_tints", [])):
        want = index + 1
        got = int(entry["id"])
        if got != want:
            raise ValueError(f"ground_tints[{index}].id 必须是 {want}（1 基连续），实到 {got}")
        rgba = [float(value) for value in entry["rgba"]]
        if len(rgba) != 4:
            raise ValueError(f"ground_tints[{index}].rgba 必须是 4 个分量")
        out.append((got, rgba))
    return out


def rust_float(value):
    """按 Rust `{}` 的最短往返写法格式化 f32（`1.0` → `1`，`0.93` → `0.93`）。

    引擎 `to_component_str` 用 `format!("{},{},{},{}", ...)` 写 `tints_rgb`，场景里多写一
    个 `.00` 就会在"生成器产物 vs 引擎回写产物"对拍里出现假差异。
    """
    return f"{value:g}"


def tints_rgb_string(table):
    return "|".join(",".join(rust_float(value) for value in rgba) for _, rgba in ground_tints(table))


def tilemap_component(tile_px, width, height, texture, cells, solid, atlas_cols, atlas_rows,
                      tile_kinds=0, tints=None, tints_rgb=""):
    """一行 Tilemap 组件串，字段顺序与引擎 `Tilemap::to_component_str` 逐字符一致。

    三个生成器（`gen_manor_scene.py` / `gen_arena_scene.py` / `set_terrain_autotile.py`）
    以前各写一条 f-string，于是"生成器产物"和"引擎回写产物"的字段顺序不一样——
    M72-01 迁编码时就撞过一次。写一次在这里，谁要出 Tilemap 都走它。

    `tile_kinds > 0` 才追加 `tile_kinds/variant_count/autotile=1`（引擎也是这个成对规则：
    不开 autotile 也没有变体时这些字段根本不出现在串里）。
    """
    parts = [
        f"tile_size={tile_px}x{tile_px}",
        f"width={width}",
        f"height={height}",
        f"texture={texture}",
        "tiles_format=1",
        f"atlas_cols={atlas_cols}",
        f"atlas_rows={atlas_rows}",
        "tiles=" + ",".join(str(value) for value in cells),
        f"solid={solid}",
    ]
    if tile_kinds > 0:
        parts.append(f"tile_kinds={tile_kinds}")
        parts.append("variant_count=1")
        parts.append("autotile=1")
    if tints:
        parts.append("tints=" + ",".join(f"{index}:{tints[index]}" for index in sorted(tints)))
    if tints_rgb:
        parts.append(f"tints_rgb={tints_rgb}")
    return ";".join(parts)


# ---------------------------------------------------------------------------
# 画布容器：按条目号写瓦片，一次成图
# ---------------------------------------------------------------------------


class AtlasSheet:
    """`cols×rows` 格、每格 `tile_px` 的地表图集画布。"""

    def __init__(self, table):
        self.table = table
        self.tile = table["tile"]
        self.canvas = Canvas(self.tile * table["cols"], self.tile * table["rows"])

    def paint(self, entry, tile_canvas):
        col, row = entry_rect(self.table, entry)
        self.canvas.blit(tile_canvas, col * self.tile, row * self.tile)

    def quantize(self):
        """出图后处理：alpha 二值化 + 色阶表吸附。返回越表像素数（门要求 0）。"""
        out, offenders = quantize_to_ramp(self.canvas, allowed_colors(self.table))
        self.canvas = out
        return offenders

    def to_png(self):
        return self.canvas.to_png()


def quantize_to_ramp(canvas, allowed):
    """把画布压回色阶表：alpha>127→255 否则归 0；不透明像素若不在表内，就近吸附并报数。

    为什么不写 docs/57 §5 说的"6bit/通道量化"：那条是给**AI 软边大图**准备的收敛手段，
    而本表的 hex 本身不落在 6bit 格点上（`#4A6636` 的 0x4A=74 不是 4 的倍数），量化一次
    就和 57 §1.2 公布的颜色对不上。这里改成更强的口径：**逐像素必须等于表内某色**，
    越表就是错，吸附只为出图不废、报数让门能红。口径登记见 M72-04 §实现记录。
    """
    palette = sorted(allowed)
    out = Canvas(canvas.w, canvas.h)
    offenders = 0
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            alpha = 255 if color[3] > 127 else 0
            if alpha == 0:
                continue
            body = color[:3]
            if body not in allowed:
                offenders += 1
                body = min(palette, key=lambda c: _distance(c, body))
            out.set(x, y, (body[0], body[1], body[2], 255))
    return out, offenders


def _distance(a, b):
    return (a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2


def observed_levels(canvas, levels):
    """画布里实际出现的色阶档（按表内顺序逐个查有没有命中像素）。"""
    wanted = {color[:3] for color in levels}
    seen = set()
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            if color[3] > 0:
                seen.add(color[:3])
    return [color for color in levels if color[:3] in seen]
