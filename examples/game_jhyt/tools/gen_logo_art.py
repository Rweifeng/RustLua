# -*- coding: utf-8 -*-
"""《江湖有田》批次 4：中文标题 Logo（512×256，唯一允许烤字的切图）。

用法：
    python tools/gen_logo_art.py
    python tools/gen_logo_art.py --check

产出：`assets/art/ui/logo/ui_title_logo.png`（512×256）+ `_preview.png`（叠在标题背景上）

按 docs/35 §1，全项目"文字一律运行时渲染"，**唯一例外是标题 Logo**；
本文件把它做出来：木匾 + 金边 + 中文「江湖有田」+ 朱砂印章「田」。

中文字形走 `cjk_text`（Windows GDI+ 栅格化 + 二值化），再做
"外描边 + 正文 + 右下压印高光"三步，得到牌匾式的立体感，风格与像素 UI 一致。

依赖：Python 3 标准库 + pixel_kit.py + cjk_text.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, mix, rgb, shade  # noqa: E402
from cjk_text import draw_plaque, prewarm, text_canvas  # noqa: E402

W, H = 512, 256
OUT_DIR = os.path.join("assets", "art", "ui", "logo")
TITLE = "江湖有田"
SEAL_TEXT = "田"

INK = rgb("#1F2328")
WOOD = rgb("#6B4F3A")
WOOD_LIGHT = rgb("#8A6A46")
WOOD_DARK = rgb("#4A3628")
WOOD_DEEP = rgb("#33251A")
GOLD = rgb("#C9A05A")
GOLD_LIGHT = rgb("#E4C078")
PAPER = rgb("#F2EAD8")
CINNABAR = rgb("#C3452F")
CINNABAR_DARK = rgb("#8F2E1F")


def wood_board(cv):
    """匾额底：木纹板 + 金边 + 四角铆钉 + 内侧暗线。"""
    cv.rect(0, 0, W - 1, H - 1, WOOD_DEEP)
    cv.rect(6, 6, W - 7, H - 7, WOOD)
    # 木纹（横向，板面用）
    for y in range(8, H - 8, 7):
        cv.hline(8, W - 9, y, shade(WOOD, 0.9))
        cv.hline(8, W - 9, y + 1, WOOD_LIGHT)
    # 顶部/底部加深，像一块厚板
    cv.rect(6, 6, W - 7, 26, mix(WOOD, WOOD_DARK, 0.35))
    cv.rect(6, H - 27, W - 7, H - 7, mix(WOOD, WOOD_DARK, 0.45))
    # 金边（双层）
    cv.rect(2, 2, W - 3, H - 3, GOLD)
    cv.rect(5, 5, W - 6, H - 6, INK)
    cv.rect(6, 6, W - 7, H - 7, WOOD)
    # 四角铆钉
    for cx, cy in ((16, 16), (W - 18, 16), (16, H - 18), (W - 18, H - 18)):
        cv.rect(cx, cy, cx + 4, cy + 4, GOLD_LIGHT)
        cv.rect(cx + 1, cy + 1, cx + 3, cy + 3, GOLD)
        cv.set(cx, cy, shade(GOLD, 0.7))
        cv.set(cx + 4, cy + 4, shade(GOLD, 0.7))
    cv.outline(INK)


def embossed_title(cv, text, size, color, edge, x_center, y_center):
    """牌匾字：外描边 + 正文 + 右下压印高光。"""
    mask = text_canvas(text, size=size, mode="smooth", color=(255, 255, 255, 255), threshold=110)
    if mask.w <= 1:
        return False
    ox = x_center - mask.w // 2
    oy = y_center - mask.h // 2
    # 外描边（对掩码做 3px 膨胀）
    for radius in (3, 2, 1):
        for y in range(mask.h):
            for x in range(mask.w):
                if mask.get(x, y)[3] == 0:
                    continue
                for dx, dy in ((radius, 0), (-radius, 0), (0, radius), (0, -radius)):
                    cv.set(ox + x + dx, oy + y + dy, INK)
    # 正文
    for y in range(mask.h):
        for x in range(mask.w):
            if mask.get(x, y)[3] > 0:
                cv.set(ox + x, oy + y, color)
    # 右下压印高光（笔画右下缘）
    for y in range(mask.h):
        for x in range(mask.w):
            if mask.get(x, y)[3] == 0:
                continue
            if mask.get(x + 2, y + 2)[3] == 0 and mask.get(x + 1, y + 1)[3] == 0:
                cv.set(ox + x, oy + y, edge)
    return True


def seal(cv, cx, cy, size):
    """朱砂印章：内嵌「田」。"""
    x0, y0 = cx - size // 2, cy - size // 2
    cv.rect(x0, y0, x0 + size, y0 + size, CINNABAR)
    cv.rect(x0 + 2, y0 + 2, x0 + size - 2, y0 + size - 2, CINNABAR_DARK)
    cv.rect(x0 + 4, y0 + 4, x0 + size - 4, y0 + size - 4, CINNABAR)
    glyph = text_canvas(SEAL_TEXT, size=int(size * 0.62), mode="pixel", color=PAPER)
    if glyph.w > 1:
        cv.blit(glyph, cx - glyph.w // 2, cy - glyph.h // 2)
    cv.outline(INK)


def build(with_seal=False):
    """默认**不画印章**：美术评审认为标题旁的红印章装饰抢视线，去掉后只剩木匾中文。

    印章函数保留并做成 `--with-seal` 开关，将来做"落款/朝代标记"时可以再开。
    """
    cv = Canvas(W, H)
    wood_board(cv)
    ok = embossed_title(cv, TITLE, 104, PAPER, GOLD_LIGHT, W // 2, H // 2 - 6)
    if with_seal:
        seal(cv, W - 54, H - 54, 56)
    if not ok:
        # 中文渲染不可用时退化为纯木匾（不写错字，也不崩）
        print("[warn] 中文栅格化不可用，Logo 只输出牌匾底", file=sys.stderr)
    return cv


def build_preview(logo):
    """叠在标题背景上预览（背景存在才叠）。"""
    bg_path = os.path.join("assets", "art", "title", "title_bg_pixel.png")
    cv = Canvas(1280, 720)
    if os.path.exists(bg_path):
        from pixel_kit import blit_scaled, load_canvas

        blit_scaled(cv, load_canvas(bg_path), 0, 0, 1280, 720, 0, 0, 1280, 720)
    else:
        cv.rect(0, 0, 1279, 719, rgb("#141A2A"))
    cv.blit(logo, (1280 - W) // 2, 150)
    draw_text(cv, 16, 686, "TITLE LOGO 512X256 / CJK BAKED (ONLY ALLOWED EXCEPTION)", rgb("#A89F8A"), 1)
    return cv


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def generate(quiet=False, with_seal=False):
    requests = [(TITLE, 104, "smooth", True)]
    if with_seal:
        requests.append((SEAL_TEXT, 34, "pixel", True))
    prewarm(requests)
    logo = build(with_seal=with_seal)
    write_png(os.path.join(OUT_DIR, "ui_title_logo.png"), logo)
    write_png(os.path.join(OUT_DIR, "_preview.png"), build_preview(logo))
    if not quiet:
        suffix = f" + 印章「{SEAL_TEXT}」" if with_seal else "（无印章装饰）"
        print(f"  ui_title_logo.png {logo.w}x{logo.h}  「{TITLE}」{suffix}")
        print("  _preview.png（叠在像素标题背景上）")


def check():
    from pixel_kit import decode_png

    issues = []
    path = os.path.join(OUT_DIR, "ui_title_logo.png")
    if not os.path.exists(path):
        issues.append("missing " + path)
    else:
        width, height, channels, _ = decode_png(path)
        if (width, height) != (W, H):
            issues.append(f"{path}: {width}x{height} != {W}x{H}")
        if channels != 4:
            issues.append(f"{path}: 期望 RGBA")
    if not os.path.exists(os.path.join(OUT_DIR, "_preview.png")):
        issues.append("missing " + os.path.join(OUT_DIR, "_preview.png"))
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》中文标题 Logo 生成")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--with-seal", action="store_true",
                        help="额外画右下角朱砂印章「田」（默认关闭：评审要求去掉红色印章装饰）")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 标题 Logo 校验通过（{W}x{H}，中文「{TITLE}」，无印章装饰）")
        return
    print("生成中文标题 Logo →", OUT_DIR)
    generate(with_seal=args.with_seal)


if __name__ == "__main__":
    main()
