# -*- coding: utf-8 -*-
"""《江湖有田》批次 4：对话头像（128×128，三种表情）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_portraits_art.py
    python tools/gen_portraits_art.py --check

产出：`assets/art/portraits/<id>_<expression>.png` + `_preview.png`（中文名 + 表情标注）

规格（docs/35 §8.3）：头像 128×128，左侧贴对话框；UI 侧另有
`ui/dialogue/ui_portrait_frame.png`（128×128 透明框）叠加在头像上，所以**头像本体满幅绘制**。

表情三档：`calm` 平静 / `surprised` 惊讶 / `angry` 生气（docs/31 §8 要求关键 NPC 至少一套情绪变化）。

配色直接复用 `gen_character_sprites.CHARACTERS` 的调色板与发型字段，
保证"头像与场景里那个小人长得是同一个人"。

主角两张（`player_male` / `player_female`）不进对话（说话人永远是 NPC），它们是 HUD
右上角 48×48 头像框里那张脸（`ui/hud.lua` 取 `state.appearance` + calm）——立绘与小人同源，
换形象时头像跟着换。

依赖：Python 3 标准库 + pixel_kit.py + gen_manor_art.py + gen_character_sprites.py + cjk_text.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, mix, rgb, shade  # noqa: E402
from gen_manor_art import fill_ellipse  # noqa: E402
from gen_character_sprites import by_id  # noqa: E402  复用角色调色板/发型
from cjk_text import draw_cjk, prewarm  # noqa: E402

SIZE = 128
OUT_DIR = os.path.join("assets", "art", "portraits")

INK = rgb("#1F2328")
INK900 = rgb("#1F2328")
INK800 = rgb("#262C33")
INK700 = rgb("#31383F")
PAPER = rgb("#F2EAD8")
PAPER_DIM = rgb("#D8CDB2")
CINNABAR = rgb("#C3452F")

EXPRESSIONS = ["calm", "surprised", "angry"]
EXPRESSION_CN = {"calm": "平静", "surprised": "惊讶", "angry": "生气"}

PORTRAIT_IDS = [
    ("shen_qingluo", "沈青萝"),
    ("gu_changfeng", "顾长风"),
    ("zhao_tiesuanpan", "赵铁算盘"),
    ("a_qi", "阿七"),
    ("situ_kui", "司徒魁"),
    # 主角（docs/45 欠账 ②：HUD 右上角只有 48×48 空框）。形象可由
    # `jianghu_set_appearance` 换、随存档持久化，所以两个形象都要有脸，
    # 否则换到青衣女侠时头像框又空回去。
    ("player_male", "青衫少侠"),
    ("player_female", "青衣女侠"),
]


def backdrop(cv):
    """背景：深墨底 + 中心略亮（把视线压到脸上）。"""
    cv.rect(0, 0, SIZE - 1, SIZE - 1, INK800)
    for step in range(6):
        tone = mix(INK800, INK700, 1 - step / 6.0)
        cv.rect(step * 3, step * 3, SIZE - 1 - step * 3, SIZE - 1 - step * 3, tone)
    # 角落暗角
    for corner_x, corner_y in ((0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)):
        for radius in range(18):
            alpha = 1 - radius / 18.0
            cv.rect(corner_x - radius if corner_x else 0, corner_y - radius if corner_y else 0,
                    corner_x + radius if corner_x else radius, corner_y + radius if corner_y else radius,
                    mix(INK800, INK900, alpha))


def shoulders(cv, pal):
    """肩与衣领：决定"这是谁的衣服"。"""
    cloth = pal["cloth"]
    trim = pal["trim"]
    # 肩部轮廓（梯形）
    for y in range(96, SIZE):
        t = (y - 96) / float(SIZE - 96)
        half = int(22 + 30 * t)
        cv.hline(64 - half, 64 + half, y, cloth)
    cv.hline(20, 108, 96, shade(cloth, 1.25))
    cv.hline(20, 108, SIZE - 1, shade(cloth, 0.75))
    # 交领
    for step in range(20):
        cv.set(58 - step // 2, 96 + step, trim)
        cv.set(59 - step // 2, 96 + step, trim)
        cv.set(70 + step // 2, 96 + step, trim)
        cv.set(69 + step // 2, 96 + step, trim)
    cv.rect(50, 100, 78, 127, shade(cloth, 0.9))


def hair(cv, ch, pal):
    """按角色的 hair 字段画头发（与场景小人的发型对应）。"""
    style = ch.get("hair", "topknot")
    base = pal["hair"]
    light = mix(base, (255, 255, 255, 255), 0.22)
    cv.hline(38, 90, 30, base)
    cv.hline(36, 92, 34, base)
    cv.rect(36, 30, 91, 40, base)
    for x in range(36, 92, 5):
        cv.vline(x, 30, 44, shade(base, 0.85))
    if style == "topknot":
        cv.rect(56, 10, 72, 32, base)
        cv.rect(52, 18, 76, 28, base)
        cv.hline(56, 72, 10, light)
    elif style == "bun":
        fill_ellipse(cv, 44, 20, 10, 9, base)
        fill_ellipse(cv, 84, 20, 10, 9, base)
        cv.set(42, 16, light)
        cv.set(82, 16, light)
    elif style == "ponytail":
        cv.rect(60, 12, 70, 30, base)
        cv.rect(84, 34, 92, 78, base)
        cv.vline(84, 34, 78, light)
    elif style == "kerchief":
        cv.rect(34, 24, 93, 42, pal["trim"])
        cv.hline(34, 93, 24, shade(pal["trim"], 1.2))
        cv.rect(88, 40, 96, 46, pal["trim"])
    elif style == "cap":
        cv.rect(36, 16, 91, 42, shade(base, 0.9))
        cv.hline(34, 93, 42, INK)
        cv.set(64, 16, mix(base, (255, 255, 255, 255), 0.35))
    elif style == "dao":
        cv.rect(56, 8, 72, 30, base)
        cv.hline(52, 76, 16, rgb("#8A6A46"))
        cv.set(64, 8, light)
    elif style == "band":
        cv.rect(34, 36, 93, 44, pal["trim"])
        cv.rect(60, 14, 70, 34, base)
    # 侧发
    cv.rect(34, 34, 42, 78, base)
    cv.rect(85, 34, 93, 78, base)


def beard(cv, ch, pal):
    kind = ch.get("beard", "none")
    base = pal["hair"]
    if kind == "stubble":
        cv.hline(48, 80, 86, mix(base, pal["skin"], 0.45))
    elif kind == "goatee":
        cv.rect(60, 88, 68, 100, base)
        cv.hline(48, 80, 86, mix(base, pal["skin"], 0.3))
    elif kind == "long":
        cv.rect(58, 88, 70, 118, base)
        cv.hline(48, 80, 86, base)
        cv.set(64, 96, mix(base, (255, 255, 255, 255), 0.45))


def face(cv, ch, pal, expression):
    """五官：三档表情（平静 / 惊讶 / 生气）。"""
    skin = pal["skin"]
    dark = shade(skin, 0.7)
    # 脸（椭圆）覆盖在头发之上
    fill_ellipse(cv, 64, 58, 25, 29, skin)
    fill_ellipse(cv, 58, 50, 18, 20, mix(skin, (255, 255, 255, 255), 0.16))  # 左上受光
    # 眉与眼
    if expression == "surprised":
        fill_ellipse(cv, 52, 54, 5, 6, PAPER)
        fill_ellipse(cv, 76, 54, 5, 6, PAPER)
        cv.rect(50, 53, 54, 57, INK)
        cv.rect(74, 53, 78, 57, INK)
        cv.hline(46, 58, 42, dark)
        cv.hline(70, 82, 42, dark)
        fill_ellipse(cv, 64, 76, 6, 5, shade(skin, 0.6))
        cv.hline(60, 68, 74, INK)
    elif expression == "angry":
        cv.hline(46, 58, 46, dark)
        cv.hline(70, 82, 46, dark)
        cv.set(47, 47, dark)
        cv.set(81, 47, dark)
        cv.rect(49, 55, 56, 57, INK)
        cv.rect(72, 55, 79, 57, INK)
        cv.hline(56, 72, 78, INK)
        cv.hline(58, 70, 79, INK)
        cv.set(56, 77, dark)
        cv.set(72, 77, dark)
    else:
        cv.hline(47, 57, 47, dark)
        cv.hline(71, 81, 47, dark)
        cv.rect(50, 54, 54, 57, INK)
        cv.rect(74, 54, 78, 57, INK)
        cv.hline(58, 70, 78, INK)
    # 鼻
    cv.set(64, 66, dark)
    cv.set(63, 67, dark)
    # 脸侧阴影
    cv.vline(86, 46, 76, shade(skin, 0.85))
    cv.hline(46, 82, 84, shade(skin, 0.88))


def accessories(cv, ch, pal):
    """身份提示（和场景小人的 accessory 对应，只画露在肩上的部分）。"""
    kind = ch.get("accessory")
    if kind == "med_box":
        cv.rect(96, 104, 122, 126, rgb("#8A6A46"))
        cv.hline(96, 122, 104, rgb("#4A3628"))
    elif kind == "abacus":
        cv.rect(8, 108, 30, 124, rgb("#4A3628"))
        cv.hline(10, 28, 114, rgb("#8A6A46"))
        cv.set(14, 118, rgb("#E4C078"))
        cv.set(24, 118, rgb("#E4C078"))
    elif kind in ("saber", "sword_hip"):
        cv.rect(100, 62, 106, 112, rgb("#C8CCD4"))
        cv.set(100, 60, rgb("#E4C078"))
    elif kind == "tea_basket":
        cv.rect(4, 96, 26, 126, rgb("#8A6A46"))
        cv.hline(4, 26, 104, rgb("#4A3628"))
    elif kind == "hammer":
        cv.rect(6, 108, 26, 118, rgb("#7E8391"))
        cv.rect(12, 118, 18, 126, rgb("#8A6A46"))
    elif kind == "whisk":
        cv.rect(104, 60, 110, 100, rgb("#8A6A46"))
        cv.rect(100, 48, 114, 62, rgb("#F4F4F8"))
    elif kind == "sword_back":
        cv.rect(58, 22, 70, 30, rgb("#4A3628"))


def portrait(char_id, expression):
    ch = by_id(char_id)
    pal = ch["palette"]
    cv = Canvas(SIZE, SIZE)
    backdrop(cv)
    shoulders(cv, pal)
    hair(cv, ch, pal)
    face(cv, ch, pal, expression)
    beard(cv, ch, pal)
    accessories(cv, ch, pal)
    cv.outline(INK)
    return cv


def build_preview(rendered, scale=1):
    cols = 3
    pad = 12
    label_h = 16
    cell = SIZE * scale + pad * 2
    rows = (len(rendered) + cols - 1) // cols
    cv = Canvas(cell * cols + 8, 24 + (cell + label_h + 6) * rows + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "PORTRAITS 128PX X1", rgb("#E4C078"), 1)
    for index, (char_id, name, expression, canvas) in enumerate(rendered):
        row, col = divmod(index, cols)
        x0 = 4 + col * cell
        y0 = 24 + row * (cell + label_h + 6)
        draw_cjk(cv, x0 + pad, y0, f"{name}·{EXPRESSION_CN[expression]}", size=13, color=PAPER)
        cv.blit(canvas, x0 + pad, y0 + label_h)
    return cv


def generate(quiet=False):
    prewarm([(f"{name}·{EXPRESSION_CN[expr]}", 13, "pixel", True)
             for _, name in PORTRAIT_IDS for expr in EXPRESSIONS])
    os.makedirs(OUT_DIR, exist_ok=True)
    rendered = []
    for char_id, name in PORTRAIT_IDS:
        for expression in EXPRESSIONS:
            canvas = portrait(char_id, expression)
            rendered.append((char_id, name, expression, canvas))
            with open(os.path.join(OUT_DIR, f"{char_id}_{expression}.png"), "wb") as handle:
                handle.write(canvas.to_png())
    with open(os.path.join(OUT_DIR, "_preview.png"), "wb") as handle:
        handle.write(build_preview(rendered).to_png())
    if not quiet:
        print(f"wrote {len(rendered)} 张头像（{len(PORTRAIT_IDS)} 角色 × {len(EXPRESSIONS)} 表情）→ {OUT_DIR}")


def check():
    from pixel_kit import decode_png

    issues = []
    for char_id, _ in PORTRAIT_IDS:
        for expression in EXPRESSIONS:
            path = os.path.join(OUT_DIR, f"{char_id}_{expression}.png")
            if not os.path.exists(path):
                issues.append("missing " + path)
                continue
            width, height, _, _ = decode_png(path)
            if (width, height) != (SIZE, SIZE):
                issues.append(f"{path}: {width}x{height} != {SIZE}x{SIZE}")
    if not os.path.exists(os.path.join(OUT_DIR, "_preview.png")):
        issues.append("missing " + os.path.join(OUT_DIR, "_preview.png"))
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》对话头像生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 头像校验通过（{len(PORTRAIT_IDS)} × {len(EXPRESSIONS)} = {len(PORTRAIT_IDS) * len(EXPRESSIONS)} 张）")
        return
    generate()


if __name__ == "__main__":
    main()
