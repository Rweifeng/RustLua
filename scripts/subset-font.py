# -*- coding: utf-8 -*-
"""Subset NotoSansSC to the game's used charset (Chinese + ASCII + punctuation).

Usage: python subset-font.py <input.ttf> <output.ttf>
Run from repo root; scans game/scripts/*.lua for the Chinese charset.
"""
import glob
import sys

from fontTools import subset


def collect_game_chars():
    chars = set()
    for path in glob.glob("game/scripts/*.lua"):
        with open(path, encoding="utf-8") as fh:
            chars.update(fh.read())
    # 常见中文标点
    chars.update("，。！？：；、（）【】《》“”‘’—…·～￥")
    # 可打印 ASCII
    for code in range(32, 127):
        chars.add(chr(code))
    return chars


def main():
    if len(sys.argv) < 3:
        print("usage: python subset-font.py <input.ttf> <output.ttf>")
        return 1
    inp, out = sys.argv[1], sys.argv[2]
    text = "".join(collect_game_chars())
    print(f"subset charset size: {len(text)} unicode chars")

    options = subset.Options()
    options.hinting = False
    options.desubroutinize = True
    options.layout_features = "*"
    options.notdef_outline = True
    options.drop_tables += ["DSIG"]
    # 保留名称表便于识别
    options.name_IDs = [1, 2, 3, 4, 6]

    font = subset.load_font(inp, options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(text=text)
    subsetter.subset(font)
    subset.save_font(font, out, options)
    print(f"saved: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
