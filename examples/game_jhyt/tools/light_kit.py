# -*- coding: utf-8 -*-
"""全局光向的**单一读取入口**（M72-06）。

事实源是 `scripts/data/lighting.lua`，不是这个文件。这里只做一件事：把那份 Lua 表
按文本解析出来（不执行 Lua），让**生成器侧**（`gen_manor_art.py` 画投影贴片、
`gen_character_sprites.py` 画脚下阴影）与**门侧**读到的偏移向量必然相同。

为什么值得单独一份：docs/51 §2.2 / docs/57 §1 早就写了"光源左上、影子右下、一批资产
只能有一个光向"，可这条口径此前只是散文——谁都没法查，于是每个生成器各写一套 `(dx, dy)`。
症状不是报错，是"这棵树往右下倒、那棵树往左下倒"（docs/53 §3.9 记的就是这条）。

两种阴影**故意不同判**（这条区分是从像素量出来的，不是美学偏好）：
  * **投影贴片**（建筑/树/道具）= cast shadow，携带光向 ⇒ 角度门采它的样本；
  * **角色脚下椭圆** = contact shadow，任何光向下同形 ⇒ 只断言"不往左上跑"，
    要求它等于 sun 的角度会把正确画法判成错误（详见 lighting.lua 的 `actor_shadow` 段）。

解析约束：只认 `M.<key> = <数>` 与 `M.<key> = { ... }`（可嵌套、可多行、值里可以带
行尾注释）。写法漂移会直接抛 `ValueError` 而不是静默给默认值——**静默回退**是这类
"工具读数据"最坏的失败模式：读不到光向就等于各画各的，而门还是绿的。
依赖：Python 3 标准库。
"""

import argparse
import math
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIGHTING_PATH = os.path.join(PROJECT_ROOT, "scripts", "data", "lighting.lua")

# 十六进制**必须排在十进制前面**：`0x1f` 若先按十进制试，会只吃掉开头的 `0` 然后把
# `x1f` 留在原地——下一段找不到 `key =` 就跳过去，结果是 `color.r` 静默读成 0。
# 影子颜色读成 0 不会报错，只会让贴片变成纯黑，正是这类解析器最难查的一种漏。
_VALUE = r"[-+]?0[xX][0-9a-fA-F]+|[-+]?[0-9]*\.?[0-9]+(?:[eE][-+]?[0-9]+)?"


def _to_number(text):
    lowered = text.strip().lower()
    if "0x" in lowered:
        return int(lowered, 0)
    return float(lowered) if ("." in lowered or "e" in lowered) else int(lowered)


def _strip_comments(text):
    """去掉 `--` 行注释，但**不碰字符串字面量里的 `--`**（本文件没有字符串值，
    真出现时宁可留下也不会误吃代码）。"""
    out = []
    for line in text.splitlines():
        quote = None
        cut = None
        index = 0
        while index < len(line):
            ch = line[index]
            if quote:
                if ch == quote and line[index - 1] != "\\":
                    quote = None
            elif ch in "\"'":
                quote = ch
            elif line[index : index + 2] == "--":
                cut = index
                break
            index += 1
        out.append(line if cut is None else line[:cut])
    return "\n".join(out)


def _scan_table(text, start):
    """从 `text[start]`（必须是 `{`）起做花括号配对，返回 (body, 结束位置)。"""
    depth = 0
    for index in range(start, len(text)):
        ch = text[index]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return text[start + 1 : index], index + 1
    raise ValueError("lighting.lua: 花括号没闭合")


def _parse_table(body):
    """把一层表体解析成 dict：数字值、`key = { ... }` 子表都吃下；其它值原样留字符串。"""
    out = {}
    index = 0
    pattern = re.compile(
        r"(?:(?P<key>\w+)\s*=\s*(?P<kind>{|(?P<value>" + _VALUE + r"|\"[^\"]*\"|true|false))"
        r"|(?P<index>\[[0-9]+\])\s*=\s*(?P<index_value>" + _VALUE + r")\s*,?)"
    )
    while index < len(body):
        match = pattern.search(body, index)
        if not match:
            break
        if match.group("index"):
            index = match.end()
            continue
        key = match.group("key")
        if match.group("kind") == "{":
            child_start = body.index("{", match.start("kind"))
            child, index = _scan_table(body, child_start)
            out[key] = _parse_table(child)
        else:
            raw = match.group("value")
            index = match.end()
            #  Lua 允许最后一项不写逗号；`raw` 已是数字或布尔，直接收。
            if raw in ("true", "false"):
                out[key] = raw == "true"
            else:
                out[key] = _to_number(raw)
        if index < len(body) and body[index : index + 1] == ",":
            index += 1
    return out


def load(path=LIGHTING_PATH):
    """解析 lighting.lua，返回 `{section: {...} | number}`。"""
    if not os.path.exists(path):
        raise FileNotFoundError(
            f"{path} 读不到：光向的唯一事实源就是 lighting.lua，宽容地当成"
            "「按默认方向继续画」等于让每个生成器各回到自己那套偏移——正是本文件要消灭的状态"
        )
    with open(path, encoding="utf-8") as handle:
        text = _strip_comments(handle.read())
    sections = {}
    for match in re.finditer(r"M\.(\w+)\s*=\s*", text):
        key = match.group(1)
        rest = text[match.end() :]
        if rest.startswith("{"):
            body, _ = _scan_table(rest, 0)
            sections[key] = _parse_table(body)
            continue
        scalar = re.match(r"\s*(" + _VALUE + ")", rest)
        if scalar:
            sections[key] = _to_number(scalar.group(1))
    for required in ("sun", "shadow_reach", "shadow_tint", "cast_shadow", "actor_shadow",
                     "occlusion", "foreground_order"):
        if required not in sections:
            raise ValueError(
                f"lighting.lua 缺 `{required}` 段：解析器宁可抛错也不给默认值——"
                "少了光向就是各画各的，而门会拿着回退值绿下去"
            )
    return sections


# ---- 与 Lua 侧同一条公式（`M.shadow_direction` / `M.shadow_offset`）----


def sun_vector(sections=None):
    src = sections or load()
    sun = src["sun"]
    x, y = float(sun["dir_x"]), float(sun["dir_y"])
    length = math.hypot(x, y)
    if length == 0:
        raise ValueError("lighting: sun 方向是零向量")
    return x / length, y / length


def shadow_offset(height, sections=None):
    """高度 h（世界单位/绘制单位）应投出的偏移 `(dx, dy)`；dy 恒为负（世界 +y 朝上）。"""
    src = sections or load()
    ux, uy = sun_vector(src)
    reach = float(src["shadow_reach"]) * float(height)
    return ux * reach, uy * reach


def angle_deg(dx, dy):
    return math.degrees(math.atan2(dy, dx))


def sun_angle(sections=None):
    ux, uy = sun_vector(sections)
    return angle_deg(ux, uy)


# ---- 贴图侧：把同一条光向落成一次明度渐变（M72-07）----


def lit_sides(sections=None):
    """哪几面被照亮：`{'top','bottom','left','right'}`。光向量是**行进方向**，取反才是来源。

    `sun = (1, -1)`（世界 +y 朝上）= 光往右下走 = 从**左上**照过来 ⇒ 顶面、左面亮。
    """
    ux, uy = sun_vector(sections)
    return {"left": ux > 0, "right": ux < 0, "top": uy < 0, "bottom": uy > 0}


def face_gain(nx, ny, strength=0.09, sections=None):
    """图上归一化坐标 (nx, ny)（ny 是**图像行**，朝下为正）处的明度乘子。

    点越靠近光源越亮：把 (nx-0.5, 0.5-ny)（换成正朝上的偏移）与"光源方向"点积，
    所以换一行 `lighting.lua` 的 sun，全图渐变方向跟着翻，贴图里不用改任何数。
    """
    ux, uy = sun_vector(sections)
    return 1.0 + 2.0 * strength * ((nx - 0.5) * -ux + (0.5 - ny) * -uy)


def grade_toward_light(canvas, strength=0.09, box=None, sections=None):
    """对 `box=(x0,y0,x1,y1)`（默认整张图）内的不透明像素做一次光向渐变。

    放在各部件画完之后统一跑，而不是每个部件自己挑亮色：一张贴图只有一条光，
    屋顶、柱子和台基必须沿同一条渐变走。描边在渐变**之后**再画，墨线才不会忽明忽暗。
    """
    x0, y0, x1, y1 = box if box else (0, 0, canvas.w, canvas.h)
    span_x = max(1, x1 - x0)
    span_y = max(1, y1 - y0)
    for y in range(y0, y1):
        for x in range(x0, x1):
            r, g, b, a = canvas.get(x, y)
            if a == 0:
                continue
            gain = face_gain((x + 0.5 - x0) / span_x, (y + 0.5 - y0) / span_y,
                             strength=strength, sections=sections)
            canvas.set(x, y, (min(255, int(round(r * gain))),
                              min(255, int(round(g * gain))),
                              min(255, int(round(b * gain))), a))


# ---- 取值区间：与 Lua 的 validate_all **同一套边界**，改了那边必须改这里 ----

_RANGE_RULES = (
    ("sun.dir_x", ("sun", "dir_x"), lambda v: v > 0, "必须为正（影子往右落）"),
    ("sun.dir_y", ("sun", "dir_y"), lambda v: v < 0, "必须为负（世界 +y 朝上，影子往画面下方落）"),
    ("shadow_reach", ("shadow_reach",), lambda v: v > 0, "必须为正"),
    ("shadow_tint.a", ("shadow_tint", "a"), lambda v: 0 < v < 0.6, "必须落在 (0, 0.6)"),
    ("cast_shadow.order", ("cast_shadow", "order"), lambda v: abs(v) <= 999, "超出 ±999 会溢进相邻层桶"),
    ("cast_shadow.min_reach_units", ("cast_shadow", "min_reach_units"), lambda v: v > 0, "必须为正"),
    ("cast_shadow.contact_flatten", ("cast_shadow", "contact_flatten"), lambda v: 0 < v <= 1, "必须落在 (0,1]"),
    ("actor_shadow.shift_x", ("actor_shadow", "shift_x"), lambda v: v >= 0, "不能为负（那是往左上投影）"),
    ("actor_shadow.shift_rows", ("actor_shadow", "shift_rows"), lambda v: v >= 0, "不能为负（行号朝下为正）"),
    ("actor_shadow.alpha", ("actor_shadow", "alpha"), lambda v: 0 < v < 255, "必须是 (0,255) 的半透明档"),
    ("occlusion.max_cover_ratio", ("occlusion", "max_cover_ratio"), lambda v: 0 < v <= 1, "必须在 (0,1]"),
    ("foreground_order.occluder", ("foreground_order", "occluder"), lambda v: abs(v) <= 999, "超出 ±999 会溢进相邻层桶"),
    ("foreground_order.effect", ("foreground_order", "effect"), lambda v: abs(v) <= 999, "超出 ±999 会溢进相邻层桶"),
)


def range_violations(src):
    problems = []
    for name, trail, ok, why in _RANGE_RULES:
        node = src
        for part in trail:
            node = node[part] if isinstance(node, dict) else None
        if not isinstance(node, (int, float)):
            problems.append(f"{name} 缺失或不是数")
            continue
        if not ok(float(node)):
            problems.append(f"{name}={node} {why}")
    max_cover = src["occlusion"].get("max_cover_ratio")
    min_cover = src["occlusion"].get("min_cover_ratio")
    if isinstance(min_cover, (int, float)) and isinstance(max_cover, (int, float)):
        if not (0 <= float(min_cover) < float(max_cover)):
            problems.append(f"occlusion.min_cover_ratio={min_cover} 必须落在 [0, max_cover_ratio)")
    occluder = src["foreground_order"].get("occluder")
    effect = src["foreground_order"].get("effect")
    if isinstance(occluder, (int, float)) and isinstance(effect, (int, float)):
        if not float(effect) > float(occluder):
            problems.append("foreground_order 里特效的 order 必须大于遮断物（刀光不能被树冠埋掉）")
    return problems


def main(argv):
    parser = argparse.ArgumentParser(description="读取并校验全局光向事实源 lighting.lua")
    parser.add_argument("--check", action="store_true", help="只校验并打印读数，不写文件")
    parser.add_argument("--height", type=float, default=None, help="给一个物体高度，打印其投影偏移")
    args = parser.parse_args(argv[1:])

    src = load()
    problems = range_violations(src)
    if args.height is not None:
        dx, dy = shadow_offset(args.height, src)
        print(f"height={args.height:g} shadow_offset=({dx:.3f}, {dy:.3f})")
    if problems:
        for line in problems:
            print(f"FAIL {line}", file=sys.stderr)
        return 1
    if args.check or args.height is None:
        ux, uy = sun_vector(src)
        print(
            f"lighting ok sun=({src['sun']['dir_x']}, {src['sun']['dir_y']}) "
            f"unit=({ux:.4f}, {uy:.4f}) angle={sun_angle(src):.2f}deg "
            f"reach={src['shadow_reach']} tint_a={src['shadow_tint']['a']} "
            f"cast=(order {src['cast_shadow']['order']}, "
            f"min_reach {src['cast_shadow']['min_reach_units']}, "
            f"flatten {src['cast_shadow']['contact_flatten']}) "
            f"actor_shift=({src['actor_shadow']['shift_x']}, {src['actor_shadow']['shift_rows']}) "
            f"cover=[{src['occlusion']['min_cover_ratio']}, {src['occlusion']['max_cover_ratio']}] "
            f"order=({src['foreground_order']['occluder']}, {src['foreground_order']['effect']})"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
