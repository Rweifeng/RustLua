# -*- coding: utf-8 -*-
"""中文文字桥：用 Windows GDI+ 把中文栅格化成美术像素。

背景：本项目的美术工具链只依赖 Python 标准库（没有 Pillow / freetype），
但美术里**确实需要中文字**（标题 Logo、店招匾额、木牌、预览图标签）。
Windows 自带 GDI+ 能按字体文件栅格化中文，于是走这条链路：

    Python 写请求 JSON → powershell.exe 调 GDI+ 渲染（并按字形包围盒裁剪）→ Python 读回 PNG

关键实现要点（都是踩过的坑）：

  * **批量渲染**：`Add-Type System.Drawing` 每次要 1–2 秒，一段文字一次调用会让生成器跑成分钟级；
    所以 `prewarm()` 把一次生成要用到的所有文字合并成**一次** PowerShell 调用。
  * **在 GDI+ 侧裁剪**：早期版本每段文字都建 2048×2048 画布，Python 那边要遍历 400 万像素去裁边，
    直接跑超时。现在用 `MeasureString(GenericTypographic)` 量出字形框，只保存那一小块。
  * **磁盘缓存**：`.temp/cjk/<sha1>.png`，同一 (文本, 字号, 模式, 粗细) 只渲染一次。

模式：`pixel` = `SingleBitPerPixelGridFit`（≤20px 匾额/标签，1 位点阵）；
      `smooth` = 在 `size×4` 上抗锯齿渲染再盒式缩小（大字 Logo，之后量化到色板）。

依赖：Windows + PowerShell 5.1（GDI+）+ **引擎侧**字体 `engine/crates/editor/fonts/NotoSansSC-VF.ttf`
（2026-10-17：工程资产树里那份 17.7 MB 副本已下架——它与引擎副本是同一个 blob，而躺在
`assets/fonts/` 下就会被打进每个发布包；工具链读引擎那份即可）。
渲染不可用时返回空画布并打印原因，不会让整条美术生成链失败。
"""

import hashlib
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, decode_png, rgb  # noqa: E402

CACHE_DIR = os.path.join(".temp", "cjk")
# 引擎侧副本，相对本文件（`examples/game_jhyt/tools/` → 仓库根 → engine/crates/editor/fonts/）。
FONT_PATH = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..",
                 "engine", "crates", "editor", "fonts", "NotoSansSC-VF.ttf")
)

PAPER = rgb("#F2EAD8")
INK = rgb("#1F2328")
GOLD = rgb("#C9A05A")

_PS_SCRIPT = r"""
param([string]$Request, [string]$OutDir)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$req = Get-Content -LiteralPath $Request -Raw -Encoding UTF8 | ConvertFrom-Json
$pfc = New-Object System.Drawing.Text.PrivateFontCollection
$pfc.AddFontFile($req.font)
$family = $pfc.Families[0]
$fmt = [System.Drawing.StringFormat]::GenericTypographic
$results = New-Object System.Collections.ArrayList
foreach ($item in $req.items) {
    $size = [int]$item.size
    $supersample = [int]$item.supersample
    $renderSize = $size * $supersample
    $style = [System.Drawing.FontStyle]::Regular
    if ($item.bold) { $style = [System.Drawing.FontStyle]::Bold }
    $font = New-Object System.Drawing.Font($family, $renderSize, $style, [System.Drawing.GraphicsUnit]::Pixel)
    # 先量字形框（GenericTypographic 更贴近实际墨迹）
    $probe = New-Object System.Drawing.Bitmap 4, 4
    $pg = [System.Drawing.Graphics]::FromImage($probe)
    $pg.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
    $sizeF = $pg.MeasureString($item.text, $font, 4096, $fmt)
    $pg.Dispose(); $probe.Dispose()
    $w = [int][math]::Ceiling($sizeF.Width) + 4
    $h = [int][math]::Ceiling($sizeF.Height) + 4
    if ($w -lt 2) { $w = 2 }
    if ($h -lt 2) { $h = 2 }
    $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Transparent)
    if ($item.mode -eq 'pixel') {
        $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::SingleBitPerPixelGridFit
    } else {
        $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
    }
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255,255,255,255))
    $g.DrawString($item.text, $font, $brush, 1, 1, $fmt)
    $g.Dispose()
    $out = Join-Path $OutDir ($item.key + '.png')
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    [void]$results.Add([pscustomobject]@{ key = $item.key; path = $out; w = $w; h = $h })
}
$results | ConvertTo-Json -Compress | Set-Content -LiteralPath (Join-Path $OutDir 'result.json') -Encoding UTF8
"""

_spec_cache = {}   # key -> (w, h)
_canvas_cache = {}  # key -> Canvas（已裁剪/二值化前的字形画布）


def _spec(text, size, mode, bold):
    if mode == "pixel" or mode == "smooth":
        return (text, int(size), mode, bool(bold))
    raise ValueError("mode must be pixel|smooth")


def _key(spec, supersample):
    text, size, mode, bold = spec
    raw = f"{text}|{size}|{mode}|{bold}|{supersample}|{FONT_PATH}"
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:16]


def _supersample(mode):
    return 1 if mode == "pixel" else 4


def prewarm(requests):
    """一次渲染多条文本（requests = [(text, size, mode, bold)]）。生成器开头调一次即可。"""
    os.makedirs(CACHE_DIR, exist_ok=True)
    pending = []
    for text, size, mode, bold in requests:
        spec = _spec(text, size, mode, bold)
        supersample = _supersample(spec[2])
        key = _key(spec, supersample)
        if not os.path.exists(os.path.join(CACHE_DIR, key + ".png")):
            pending.append({"key": key, "text": text, "size": spec[1], "mode": spec[2],
                            "bold": spec[3], "supersample": supersample})
    if not pending:
        return True
    script_path = os.path.join(CACHE_DIR, "render.ps1")
    request_path = os.path.join(CACHE_DIR, "request.json")
    with open(script_path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(_PS_SCRIPT)
    payload = {"font": os.path.abspath(FONT_PATH), "items": pending}
    with open(request_path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False)
    try:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
             "-File", os.path.abspath(script_path),
             "-Request", os.path.abspath(request_path),
             "-OutDir", os.path.abspath(CACHE_DIR)],
            capture_output=True, text=True, timeout=300,
        )
    except Exception as exc:
        print(f"[cjk_text] PowerShell 渲染失败：{exc}", file=sys.stderr)
        return False
    if result.returncode != 0:
        print(f"[cjk_text] 渲染返回 {result.returncode}：{(result.stderr or '').strip()[:300]}",
              file=sys.stderr)
        return False
    return True


def _raw_canvas(text, size, mode, bold):
    """读回字形 PNG（已按包围盒裁剪，尺寸很小）。"""
    spec = _spec(text, size, mode, bold)
    supersample = _supersample(mode)
    key = _key(spec, supersample)
    if key in _canvas_cache:
        return _canvas_cache[key]
    path = os.path.join(CACHE_DIR, key + ".png")
    if not os.path.exists(path):
        if not prewarm([(text, size, mode, bold)]):
            _canvas_cache[key] = None
            return None
    if not os.path.exists(path):
        _canvas_cache[key] = None
        return None
    width, height, channels, pixels = decode_png(path)
    canvas = Canvas(width, height)
    for y in range(height):
        row = y * width
        for x in range(width):
            i = (row + x) * channels
            alpha = pixels[i + 3] if channels == 4 else 255
            canvas.set(x, y, (255, 255, 255, alpha))
    if mode == "smooth" and supersample > 1:
        out = Canvas(max(1, width // supersample), max(1, height // supersample))
        for oy in range(out.h):
            for ox in range(out.w):
                total = 0
                count = 0
                for sy in range(supersample):
                    for sx in range(supersample):
                        total += canvas.get(ox * supersample + sx, oy * supersample + sy)[3]
                        count += 1
                out.set(ox, oy, (255, 255, 255, total // max(1, count)))
        canvas = out
    _canvas_cache[key] = canvas
    return canvas


def text_canvas(text, size=14, mode="pixel", color=PAPER, bold=True, threshold=96):
    """一段中文 → Canvas（1 位点阵，已去除四周空白）。失败时返回 1×1 空画布。"""
    canvas = _raw_canvas(text, size, mode, bold)
    if canvas is None or canvas.w <= 1:
        return Canvas(1, 1)
    min_x, min_y = canvas.w, canvas.h
    max_x = max_y = -1
    for y in range(canvas.h):
        for x in range(canvas.w):
            if canvas.get(x, y)[3] >= threshold:
                min_x = min(min_x, x)
                min_y = min(min_y, y)
                max_x = max(max_x, x)
                max_y = max(max_y, y)
    if max_x < 0:
        return Canvas(1, 1)
    out = Canvas(max_x - min_x + 1, max_y - min_y + 1)
    for y in range(min_y, max_y + 1):
        for x in range(min_x, max_x + 1):
            if canvas.get(x, y)[3] >= threshold:
                out.set(x - min_x, y - min_y, color)
    return out


def measure_cjk(text, size=14, mode="pixel", bold=True):
    canvas = text_canvas(text, size=size, mode=mode, bold=bold)
    return canvas.w, canvas.h


def draw_cjk(dst, x, y, text, size=14, mode="pixel", color=PAPER, outline=None, bold=True):
    """在 dst 上画中文；outline 会给字形加 1px 描边（小字号下更清楚）。"""
    canvas = text_canvas(text, size=size, mode=mode, color=color, bold=bold)
    if outline is not None and canvas.w > 1:
        base = Canvas(canvas.w + 2, canvas.h + 2)
        for gy in range(canvas.h):
            for gx in range(canvas.w):
                if canvas.get(gx, gy)[3] > 0:
                    for dx, dy in ((0, 1), (2, 1), (1, 0), (1, 2), (0, 0), (2, 0), (0, 2), (2, 2)):
                        base.set(gx + dx, gy + dy, outline)
        base.blit(canvas, 1, 1)
        canvas = base
        x -= 1
        y -= 1
    dst.blit(canvas, x, y)
    return canvas.w, canvas.h


def available():
    return text_canvas("测", size=12).w > 1


WOOD_MID = rgb("#6B4F3A")
WOOD_DARK = rgb("#4A3628")
GOLD_C = rgb("#C9A05A")


def draw_plaque(dst, x0, y0, x1, y1, text, size=12, board=WOOD_DARK, face=WOOD_MID, ink=PAPER):
    """匾额：木底 + 金边 + 居中中文（全项目招牌统一走这一个实现）。

    建筑、木牌、旗、店招都用它，保证"同一世界里的牌子长得一样"。
    """
    dst.rect(x0, y0, x1, y1, board)
    dst.rect(x0 + 2, y0 + 2, x1 - 2, y1 - 2, face)
    dst.hline(x0 + 2, x1 - 2, y0 + 2, GOLD_C)
    dst.hline(x0 + 2, x1 - 2, y1 - 2, GOLD_C)
    glyph = text_canvas(text, size=size, mode="pixel", color=ink)
    if glyph.w <= 1:
        return
    dst.blit(glyph, (x0 + x1) // 2 - glyph.w // 2, (y0 + y1) // 2 - glyph.h // 2)


if __name__ == "__main__":
    # 自检：把常用词在 11/13/16/20px 下的可读性落成一张检查图
    words = ["江湖有田", "云泽村", "青云山", "寒泉洞", "药庐", "杂货铺", "铁匠铺", "茶摊", "药田", "山庄"]
    sizes = [11, 13, 16, 20]
    prewarm([(word, size, "pixel", True) for size in sizes for word in words])
    row_h = 24
    width = 60 + sum(measure_cjk(word, size=sizes[-1])[0] + 10 for word in words)
    canvas = Canvas(max(640, width), 20 + len(sizes) * row_h + 6)
    canvas.rect(0, 0, canvas.w - 1, canvas.h - 1, rgb("#14171A"))
    from pixel_kit import draw_text as draw_ascii

    draw_ascii(canvas, 6, 6, "CJK LEGIBILITY", (0xE4, 0xC0, 0x78, 255), 1)
    for index, size in enumerate(sizes):
        y = 20 + index * row_h
        draw_ascii(canvas, 6, y + 4, f"{size}PX", (0x8A, 0x8F, 0x98, 255), 1)
        x = 56
        for word in words:
            w, _ = draw_cjk(canvas, x, y, word, size=size, color=PAPER)
            x += w + 10
    out = os.path.join(CACHE_DIR, "legibility.png")
    os.makedirs(CACHE_DIR, exist_ok=True)
    with open(out, "wb") as handle:
        handle.write(canvas.to_png())
    print("wrote", out, canvas.w, canvas.h, "available =", available())
