# -*- coding: utf-8 -*-
"""断链检查：工程里对 `*.scene` 的**真实引用**是否都指向存在的场景文件。

背景：删掉旧开始画面场景后，`scripts/main.lua` 里还留着 `scene.load("<旧场景>")`
以及注释里写着的字面路径，编辑器的依赖扫描据此报
「断链：依赖 scenes/xxx.scene 缺失」。**注释里的路径同样会被扫到**，
所以这里扫全文，而不是只扫字符串字面量。

只认两种真实引用形态（避免把 `snapshot.scene` 这种 Lua 字段访问误判成场景）：
  1. 带目录的字面路径：`scenes/<name>.scene`
  2. 场景加载调用：      `scene.load('<name>')` / `scene.load("<name>")`

用法：
    python tools/check_scene_refs.py            # 查源码/数据（scripts、scenes、tools、web、project.toml）
    python tools/check_scene_refs.py --all      # 连 builds/、.cache/ 等产物目录一起列（分区报告）

退出码：0 = 无断链；1 = 有断链。
"""

import argparse
import os
import re
import sys

SOURCE_DIRS = ["scripts", "scenes", "tools", "web"]
SOURCE_FILES = ["project.toml"]
ARTIFACT_DIRS = ["builds", ".cache", ".data"]

TEXT_SUFFIX = {".lua", ".scene", ".toml", ".py", ".json", ".js", ".ts", ".txt", ".html", ".css"}

PATH_REF = re.compile(r"scenes/([\w-]+)\.scene")
LOAD_REF = re.compile(r"scene\.load\(\s*['\"]([\w-]+)['\"]\s*\)")
# 场景文件头部的 name=<scene> 声明也要能被交叉校验
SCENE_NAME = re.compile(r"^name=([\w-]+)$", re.MULTILINE)


def existing_scenes():
    if not os.path.isdir("scenes"):
        return set(), {}
    names = {}
    for filename in os.listdir("scenes"):
        if filename.endswith(".scene"):
            names[filename[:-len(".scene")]] = os.path.join("scenes", filename)
    return set(names), names


def scan(paths, known, problems):
    self_path = os.path.abspath(__file__)
    for root in paths:
        if not os.path.exists(root):
            continue
        if os.path.isfile(root):
            walk = [(os.path.dirname(root) or ".", [], [os.path.basename(root)])]
        else:
            walk = os.walk(root)
        for dirpath, dirnames, filenames in walk:
            dirnames[:] = [d for d in dirnames if d != "__pycache__"]
            for filename in filenames:
                if os.path.splitext(filename)[1].lower() not in TEXT_SUFFIX:
                    continue
                full = os.path.join(dirpath, filename)
                if os.path.abspath(full) == self_path:
                    continue
                try:
                    with open(full, encoding="utf-8") as handle:
                        text = handle.read()
                except (OSError, UnicodeDecodeError):
                    continue
                for regex, label in ((PATH_REF, "字面路径"), (LOAD_REF, "scene.load")):
                    for match in regex.finditer(text):
                        name = match.group(1)
                        if name in known:
                            continue
                        line = text[:match.start()].count("\n") + 1
                        problems.append(
                            f"{full}:{line}: {label} 引用了不存在的场景 `{name}.scene`")
    return problems


def check_scene_names(known, names):
    """scenes/*.scene 头部的 name= 必须与文件名一致（场景名对不上也会被当断链）。"""
    problems = []
    for name, path in sorted(names.items()):
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        found = SCENE_NAME.search(text)
        if not found:
            problems.append(f"{path}: 缺少 name= 头")
        elif found.group(1) != name:
            problems.append(f"{path}: name={found.group(1)} 与文件名 {name} 不一致")
    return problems


def main():
    parser = argparse.ArgumentParser(description="工程断链检查（*.scene 引用）")
    parser.add_argument("--all", action="store_true", help="同时检查 builds/.cache/.data 产物目录")
    args = parser.parse_args()

    known, names = existing_scenes()
    print(f"scenes/ 现有 {len(known)} 个场景：" + "、".join(sorted(known)))

    problems = check_scene_names(known, names)
    scan(SOURCE_DIRS + SOURCE_FILES, known, problems)
    if problems:
        print(f"\n[FAIL] 源码/数据里有 {len(problems)} 处断链：")
        for problem in problems:
            print("  " + problem)
    else:
        print("\n[OK] 源码/数据无断链（字面路径、scene.load、name= 头全部对得上）")

    if args.all:
        artifact_problems = scan(ARTIFACT_DIRS, known, [])
        if artifact_problems:
            print(f"\n[注意] 产物/缓存里仍有 {len(artifact_problems)} 处旧引用（刷新构建即可消除）：")
            for problem in artifact_problems[:20]:
                print("  " + problem)
            if len(artifact_problems) > 20:
                print(f"  ...（共 {len(artifact_problems)} 处）")
        else:
            print("[OK] 产物/缓存无断链")

    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
