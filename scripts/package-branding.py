#!/usr/bin/env python3
"""Stage early-boot PNG branding outside the resource pack."""

import argparse
from pathlib import Path
import shutil
import sys
import warnings


def configured_image(root, key):
    """Read `<key>` exactly like the engine does, then validate it as a file.

    `rustlua_infra::project_config::parse_key` is a line scanner (first `key =`
    line wins, tables are not scopes, quotes are trimmed, an empty value means
    missing). A real TOML parser would disagree on backslashes, inline comments
    and scoping, so packaging must not use one: the packaged bytes have to be
    the image the runtime picks.
    """
    root = root.resolve(strict=True)
    for line in (root / "project.toml").read_text(encoding="utf-8-sig").splitlines():
        line = line.strip().lstrip("\ufeff")
        if not line.startswith(key + " ="):
            continue
        value = line[len(key) + 2:].strip().strip('"')
        if not value.strip():
            continue
        normalized = value.replace("\\", "/")
        parts = normalized.split("/")
        # Reject drive/UNC/device paths, traversal, ADS, wildcards and Win32
        # aliases (trailing dots/spaces, DOS device names) before resolving.
        reserved = {"CON", "PRN", "AUX", "NUL"}
        reserved.update(f"{prefix}{digit}" for prefix in ("COM", "LPT") for digit in "123456789¹²³")
        if (normalized.startswith("/") or not any(part not in ("", ".") for part in parts)
                or any(ord(char) < 32 or char in ':<>"|?*' for char in normalized)
                or any(part == ".." or (part not in ("", ".") and
                       (part.rstrip(" .") != part or part.split(".")[0].upper() in reserved))
                       for part in parts)):
            raise ValueError(f"{key} must be a project-relative file without '..': {value!r}")
        image = root.joinpath(*parts).resolve(strict=True)
        if not image.is_relative_to(root) or not image.is_file():
            raise ValueError(f"{key} must resolve to a file inside the project: {value!r}")
        return image
    return None


def stage_branding(root, destination):
    assets = Path(__file__).resolve().parents[1] / "engine" / "assets"
    destination.mkdir(parents=True, exist_ok=True)
    for kind in ("icon", "splash"):
        source = configured_image(root, kind + "_image")
        target = destination / (kind + ".png")
        if source is None:
            shutil.copyfile(assets / ("engine_" + kind + ".png"), target)
        else:
            try:
                from PIL import Image
            except ImportError as error:
                raise ValueError("Project branding conversion requires Pillow on the packaging host") from error
            with warnings.catch_warnings():
                warnings.simplefilter("error", Image.DecompressionBombWarning)
                with Image.open(source) as image:
                    decoded = image.convert("RGBA")
                    if image.format == "PNG":
                        shutil.copyfile(source, target)
                    else:
                        decoded.save(target, format="PNG")
        print(f"branding_ok {kind}={source or 'engine-default'} bytes={target.stat().st_size}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        stage_branding(args.project_root, args.out)
    except (OSError, ValueError, Warning) as error:
        print(f"branding_failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
