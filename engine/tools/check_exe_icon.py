#!/usr/bin/env python3
"""核验：二进制的 PE 资源里确实嵌上了引擎图标，而且嵌的就是入库那张 .ico。

用法（仓库根）：
    PYTHONIOENCODING=utf-8 py engine/tools/check_exe_icon.py \
        engine/target/debug/editor.exe engine/target/debug/game.exe engine/target/debug/assetc.exe

为什么需要这层：build.rs 只能证明"rc.exe 编出了 .res 并交给了链接器"，
`link.exe` 收下它不等于产物里真有 `RT_GROUP_ICON`——参数写错、换目标平台、
将来有人改打包脚本，症状都是"构建全绿、Explorer 里还是白纸图标"。
本脚本把图标从 PE 里**读回来**与 `engine/assets/engine_icon.ico` 逐字节对账，
对不上就 exit 1。纯标准库，不要求装 Windows SDK。
"""

from __future__ import annotations

import argparse
import os
import struct
import sys

ICO_PATH_DEFAULT = "engine/assets/engine_icon.ico"

RT_ICON = 3
RT_GROUP_ICON = 14


def u16(data, off):
    return struct.unpack_from("<H", data, off)[0]


def u32(data, off):
    return struct.unpack_from("<I", data, off)[0]


class BadBinary(Exception):
    """读不回图标的所有原因都归这一类：调用方把它转成一行 FAIL 判决。"""


class PE:
    """只解析到"按 RVA 取字节"和"按类型遍历资源树"两层，够用就停。"""

    def __init__(self, blob: bytes, origin: str):
        self.blob = blob
        self.origin = origin
        if blob[:2] != b"MZ":
            raise BadBinary(f"{origin}：不是 DOS/PE 文件")
        pe_off = u32(blob, 0x3C)
        if blob[pe_off:pe_off + 4] != b"PE\0\0":
            raise BadBinary(f"{origin}：找不到 PE 头")
        # COFF 头紧跟签名：节数在 +2、可选头大小在 +16（偏移都相对 "PE\0\0"）。
        nsec = u16(blob, pe_off + 6)
        opt_size = u16(blob, pe_off + 20)
        opt = pe_off + 24
        magic = u16(blob, opt)
        if magic not in (0x10B, 0x20B):
            raise BadBinary(f"{origin}：未知可选头魔数 {magic:#x}")
        # 数据目录项宽 8 字节：PE32 从可选头内偏移 96 起、PE32+ 从 112 起；
        # 资源目录是第 **3** 项（下标 2：exports、imports、resources）。
        dir_off = opt + (96 if magic == 0x10B else 112)
        self.rsrc_rva, self.rsrc_size = struct.unpack_from("<II", blob, dir_off + 2 * 8)
        self.sections = []
        for i in range(nsec):
            entry = opt + opt_size + 40 * i
            # IMAGE_SECTION_HEADER 前两个 u32 是 VirtualSize 在前、VirtualAddress 在后。
            vsize, virtual, raw_size, raw_ptr = struct.unpack_from("<IIII", blob, entry + 8)
            self.sections.append((virtual, vsize, raw_ptr, raw_size))
        if not self.rsrc_rva:
            raise BadBinary(f"{origin}：没有 .rsrc 段，图标根本没被链进去")

    def at_rva(self, rva: int, size: int) -> bytes:
        for virtual, vsize, raw, fsize in self.sections:
            if virtual <= rva < virtual + max(vsize, fsize):
                off = raw + (rva - virtual)
                if off + size > len(self.blob):
                    raise BadBinary(f"{self.origin}：RVA {rva:#x}+{size} 越过文件尾")
                return self.blob[off:off + size]
        raise BadBinary(f"{self.origin}：RVA {rva:#x}+{size} 落在任何节之外")

    def child_rva(self, packed: int) -> int:
        # 目录项里的 OffsetToData 是"相对 .rsrc 段首"的偏移（与数据项那种
        # 相对镜像基址的 RVA 不同），最高位为 1 表示指向子目录。
        return self.rsrc_rva + (packed & 0x7FFFFFFF)

    def _entries(self, dir_rva: int):
        """一个目录节点的条目：[(资源 ID 或 None, OffsetToData 原值), ...]。

        IMAGE_RESOURCE_DIRECTORY 是 16 字节头，条目表从 +16 起，先名字表后 ID 表；
        名字条目首字段是"名称 RVA|0x80000000"而不是资源 ID，所以一律给 None 占位，
        免得把名称 RVA 当成 ID 拿去和 RT_ICON 的编号比。
        """
        named, ided = struct.unpack_from("<HH", self.at_rva(dir_rva, 16), 12)
        out = []
        for i in range(named):
            packed = u32(self.at_rva(dir_rva + 16 + 8 * i, 8), 4)
            out.append((None, packed))
        for i in range(ided):
            ident, packed = struct.unpack_from(
                "<II", self.at_rva(dir_rva + 16 + 8 * named + 8 * i, 8)
            )
            out.append((None if ident & 0x80000000 else ident, packed))
        return out

    def data_of_type(self, type_id: int):
        """[(资源名 ID, [数据块, ...]), ...]。

        资源树只有三层目录：root（按类型）→ 类型目录（按名字）→ 名字目录（按语言）。
        前两层的条目最高位为 1、指向下一层目录；**名字目录的条目最高位为 0、直接指向
        IMAGE_RESOURCE_DATA_ENTRY**，而那 16 字节里的 OffsetToData 才是相对镜像基址的
        RVA（与目录层的"相对段首偏移"不同口径）。少剥一层会把条目头当数据头读，症状是
        "类型明明在、一块数据都取不出来"。
        """
        for ident, packed in self._entries(self.rsrc_rva):
            if ident != type_id:
                continue
            results = []
            for name_id, name_packed in self._entries(self.child_rva(packed)):
                blocks = []
                for _language, language_packed in self._entries(self.child_rva(name_packed)):
                    if language_packed & 0x80000000:
                        raise BadBinary(
                            f"{self.origin}：语言层之下还有子目录，不是本工具认的图标布局"
                        )
                    rva, size = struct.unpack_from(
                        "<II", self.at_rva(self.child_rva(language_packed), 8)
                    )
                    blocks.append(self.at_rva(rva, size))
                results.append((name_id, blocks))
            return results
        return []


def ico_entries(path: str):
    """入库 .ico → [(边长, 平面数, 位深, 图条目字节)]；条目字节就是 RT_ICON 该存的内容。"""
    data = open(path, "rb").read()
    reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    if reserved != 0 or kind != 1:
        raise BadBinary(f"{path}：不是 ICO（reserved={reserved}、type={kind}）")
    entries = []
    for i in range(count):
        edge_w, edge_h, _, _, planes, bpp, length, offset = struct.unpack_from(
            "<BBBBHHII", data, 6 + 16 * i
        )
        if edge_w != edge_h:
            raise BadBinary(f"{path}：第 {i} 项非正方形（{edge_w}x{edge_h}），本工具只核方形图标")
        entries.append((256 if edge_w == 0 else edge_w, planes, bpp,
                        data[offset:offset + length]))
    return entries


def exe_icon_entries(path: str):
    """exe → [(边长, 平面数, 位深, RT_ICON 字节)]，按 GRPICONDIR 的清单配上各 RT_ICON 块。"""
    blob = open(path, "rb").read()
    pe = PE(blob, path)

    groups = pe.data_of_type(RT_GROUP_ICON)
    if not groups:
        raise BadBinary(f"{path}：PE 资源里没有 RT_GROUP_ICON（图标没被链进去）")
    if len(groups) > 1:
        raise BadBinary(f"{path}：有 {len(groups)} 个图标组，本工具只核「默认图标」这一种形态")

    grp = groups[0][1][0]
    reserved, kind, count = struct.unpack_from("<HHH", grp, 0)
    if reserved != 0 or kind != 1:
        raise BadBinary(f"{path}：GRPICONDIR 头不对（type={kind}）")

    icons = {}
    for name_id, blocks in pe.data_of_type(RT_ICON):
        if name_id is not None:
            icons[name_id] = blocks[0]
    entries = []
    for i in range(count):
        record = 6 + 14 * i
        edge_w, edge_h, _, _, planes, bpp, length = struct.unpack_from("<BBBBHHI", grp, record)
        if edge_w != edge_h:
            raise BadBinary(f"{path}：组里第 {i} 项非正方形")
        resource_id = u16(grp, record + 12)
        blob_icon = icons.get(resource_id)
        if blob_icon is None:
            raise BadBinary(f"{path}：组里引用了不存在的 RT_ICON #{resource_id}")
        if len(blob_icon) != length:
            raise BadBinary(
                f"{path}：RT_ICON #{resource_id} 声明 {length} 字节、实际 {len(blob_icon)} 字节"
            )
        entries.append((256 if edge_w == 0 else edge_w, planes, bpp, blob_icon))
    return entries


def compare(exe_path: str, ico_path: str):
    expected = ico_entries(ico_path)
    actual = exe_icon_entries(exe_path)
    problems = []

    def table(rows):
        return sorted((edge, planes, bpp, len(blob)) for edge, planes, bpp, blob in rows)

    if table(expected) != table(actual):
        problems.append(
            "档位表不一致："
            f"期望 {[(e, n) for e, _, _, n in table(expected)]}，"
            f"实际 {[(e, n) for e, _, _, n in table(actual)]}"
        )
    by_size = {edge: (planes, bpp, blob) for edge, planes, bpp, blob in expected}
    for edge, planes, bpp, blob in actual:
        if edge not in by_size:
            problems.append(f"{edge}px 这一档不在入库 .ico 里")
            continue
        exp_planes, exp_bpp, exp_blob = by_size[edge]
        if (planes, bpp) != (exp_planes, exp_bpp):
            problems.append(
                f"{edge}px 格式不符：期望 {exp_planes} 平面/{exp_bpp}bpp，实际 {planes}/{bpp}"
            )
        if blob != exp_blob:
            problems.append(f"{edge}px 像素字节与入库 .ico 不一致（改完规格没重编 exe？）")
    return problems, actual


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="核验 exe 里嵌的图标就是 engine_icon.ico")
    parser.add_argument("exe", nargs="+", help="待核验的 .exe 路径")
    parser.add_argument("--ico", default=ICO_PATH_DEFAULT, help="对照的入库 .ico")
    arguments = parser.parse_args(argv[1:])

    if not os.path.exists(arguments.ico):
        print(f"FAIL 对照文件 {arguments.ico} 不存在，先跑 py engine/tools/gen_engine_icon.py 出图")
        return 1

    failed = False
    for exe_path in arguments.exe:
        if not os.path.exists(exe_path):
            print(f"FAIL {exe_path} 不存在（先 cargo build -p editor -p game -p assetc）")
            failed = True
            continue
        try:
            problems, entries = compare(exe_path, arguments.ico)
        except BadBinary as error:
            print(f"FAIL {error}")
            failed = True
            continue
        except (OSError, struct.error) as error:
            print(f"FAIL 解析 {exe_path} 失败：{error}")
            failed = True
            continue
        if problems:
            failed = True
            for problem in problems:
                print(f"FAIL {os.path.basename(exe_path)}：{problem}")
            continue
        sizes = "/".join(str(edge) for edge, _, _, _ in sorted(entries))
        print(f"OK   {os.path.basename(exe_path)}：RT_GROUP_ICON + {len(entries)} 张 RT_ICON"
              f"（{sizes}px），逐字节等于 {arguments.ico}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
