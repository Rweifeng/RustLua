#!/usr/bin/env python3
"""Apply project.toml icon_image to a staged Windows game.exe, before signing.

Packaging only: Pillow is needed when an override is configured. The Cargo
output and engine icon are never changed by package.ps1. Temporary ICO/PE files
are removed, and the staged exe is replaced only after readback.
"""

from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import importlib.util
import io
import os
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import warnings

# Share the byte-for-byte PE/ICO gate used for the compiled engine icon.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "engine" / "tools"))
from check_exe_icon import BadBinary, PE, RT_GROUP_ICON, RT_ICON, compare, ico_entries

# One key parser for every branding surface: package-branding.py mirrors the
# engine's line scanner, and Web/WeChat/PC must not disagree on which image the
# project asked for.
_branding_path = Path(__file__).with_name("package-branding.py")
_branding_spec = importlib.util.spec_from_file_location("package_branding", _branding_path)
if _branding_spec is None or _branding_spec.loader is None:
    raise SystemExit(f"FAIL project icon: cannot load branding key parser: {_branding_path}")
package_branding = importlib.util.module_from_spec(_branding_spec)
_branding_spec.loader.exec_module(package_branding)


ICON_SIZES = (16, 24, 32, 48, 256)


def configured_image(project_root: Path) -> Path | None:
    return package_branding.configured_image(project_root, "icon_image")


def encode_ico(image_path: Path) -> bytes:
    try:
        from PIL import Image
    except ImportError as error:
        raise ValueError("icon_image requires Pillow on the packaging host; install with python -m pip install Pillow") from error

    with warnings.catch_warnings():
        warnings.simplefilter("error", Image.DecompressionBombWarning)
        with Image.open(image_path) as source:
            master = source.convert("RGBA")  # Fully decode; corrupt images fail the package.
    images = []
    for size in ICON_SIZES:
        # Same square Lanczos resize as the native window icon.
        scaled = master.resize((size, size), Image.Resampling.LANCZOS)
        if size == 256:
            output = io.BytesIO()
            scaled.save(output, format="PNG")
            body = output.getvalue()
        else:
            # Match gen_engine_icon.py: bottom-up BGRA DIB + DWORD-aligned,
            # zero AND mask. Small PNG entries are not accepted by every RC.
            pixels = scaled.tobytes("raw", "BGRA", 0, -1)
            header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
            body = header + pixels + bytes(((size + 31) // 32) * 4 * size)
        images.append((size, body))
    directory = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for size, body in images:
        edge = size % 256
        directory += struct.pack("<BBBBHHII", edge, edge, 0, 0, 1, 32, len(body), offset)
        offset += len(body)
    return bytes(directory) + b"".join(body for _, body in images)


def icon_resources(exe_path: Path) -> list[tuple[int, int, int]]:
    """Enumerate numeric icon resource names/languages, including stale groups."""
    blob = exe_path.read_bytes()
    # The existing checker deliberately rejects PE files without .rsrc. Older
    # -SkipBuild artifacts may have no resources; adding their first icon is OK.
    if blob[:2] != b"MZ":
        raise ValueError(f"Not a PE executable: {exe_path}")
    pe_offset = struct.unpack_from("<I", blob, 0x3C)[0]
    if blob[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise ValueError(f"Missing PE signature: {exe_path}")
    opt = pe_offset + 24
    magic = struct.unpack_from("<H", blob, opt)[0]
    if magic not in (0x10B, 0x20B):
        raise ValueError(f"Unsupported PE optional header: {magic:#x}")
    directories = opt + (96 if magic == 0x10B else 112)
    if struct.unpack_from("<I", blob, directories + 16)[0] == 0:
        return []
    pe = PE(blob, str(exe_path))
    resources = []
    for kind, packed in pe._entries(pe.rsrc_rva):
        if kind not in (RT_ICON, RT_GROUP_ICON):
            continue
        for name, name_packed in pe._entries(pe.child_rva(packed)):
            for language, data_packed in pe._entries(pe.child_rva(name_packed)):
                if (name is None or language is None or name > 65535 or language > 65535
                        or data_packed & 0x80000000):
                    raise ValueError("Unsupported named/nested icon resources; refusing a partial override")
                resources.append((kind, name, language))
    return resources


def update_resources(exe_path: Path, ico_path: Path) -> None:
    if os.name != "nt":
        raise ValueError("PE icon replacement requires a Windows packaging host")
    resources = icon_resources(exe_path)
    languages = sorted({lang for _, _, lang in resources}) or [0]
    entries = ico_entries(str(ico_path))
    group = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    for resource_id, (size, planes, bpp, body) in enumerate(entries, 1):
        edge = size % 256
        group += struct.pack("<BBBBHHIH", edge, edge, 0, 0, planes, bpp, len(body), resource_id)

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    begin = kernel.BeginUpdateResourceW
    begin.argtypes = [wintypes.LPCWSTR, wintypes.BOOL]
    begin.restype = wintypes.HANDLE
    update = kernel.UpdateResourceW
    update.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                       wintypes.WORD, ctypes.c_void_p, wintypes.DWORD]
    update.restype = wintypes.BOOL
    end = kernel.EndUpdateResourceW
    end.argtypes = [wintypes.HANDLE, wintypes.BOOL]
    end.restype = wintypes.BOOL
    handle = begin(str(exe_path), False)  # Preserve every non-icon resource.
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        for kind, name, language in resources:
            if not update(handle, kind, name, language, None, 0):
                raise ctypes.WinError(ctypes.get_last_error())
        # Replace every language, not just the first one the checker sees. Keep
        # one default group (ID 1) so Explorer cannot select an old project icon.
        for language in languages:
            for resource_id, (_, _, _, body) in enumerate(entries, 1):
                buffer = ctypes.create_string_buffer(body)
                if not update(handle, RT_ICON, resource_id, language, buffer, len(body)):
                    raise ctypes.WinError(ctypes.get_last_error())
            buffer = ctypes.create_string_buffer(bytes(group))
            if not update(handle, RT_GROUP_ICON, 1, language, buffer, len(group)):
                raise ctypes.WinError(ctypes.get_last_error())
        committed = end(handle, False)
        handle = None
        if not committed:
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        if handle:
            end(handle, True)  # Discard pending updates on failure.


def apply_project_icon(project_root: Path, exe_path: Path) -> bool:
    image = configured_image(project_root)
    if image is None:
        print("project_icon_default: leaving compiled engine icon unchanged")
        return False
    ico = encode_ico(image)
    exe_path = exe_path.resolve(strict=True)
    # Work on a real copy, not the Cargo artifact or a hard link. Even a failed
    # Win32 update/readback cannot corrupt the staged runner or previous release.
    with tempfile.TemporaryDirectory(prefix=".project-icon-", dir=exe_path.parent) as work:
        work = Path(work)
        ico_path = work / "project.ico"
        candidate = work / "game.exe"
        ico_path.write_bytes(ico)
        shutil.copy2(exe_path, candidate)
        update_resources(candidate, ico_path)
        problems, _ = compare(str(candidate), str(ico_path))
        if problems:
            raise ValueError("PE icon readback failed: " + "; ".join(problems))
        # UpdateResource can leave an existing executable open briefly on some
        # Windows hosts, so copy the verified bytes over the staged destination
        # rather than relying on os.replace's delete/rename semantics.
        shutil.copy2(candidate, exe_path)
        problems, _ = compare(str(exe_path), str(ico_path))
        if problems:
            raise ValueError("PE icon final readback failed: " + "; ".join(problems))
    print(f"project_icon_ok: {image} -> {exe_path} (16/24/32/48/256px, verified)")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-root", type=Path, required=True)
    parser.add_argument("--exe", type=Path, required=True, help="staged game.exe, never the Cargo output")
    args = parser.parse_args()
    try:
        apply_project_icon(args.project_root, args.exe)
    except (OSError, ValueError, BadBinary, struct.error, Warning) as error:
        print(f"FAIL project icon: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
