#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""trend_db.py —— 性能/覆盖率/编译/体积趋势数据库（P4-04）。

把"这次 vs 上次"升级为"这次 vs 同环境最近 N 次中位数 + 已批准基线"。
历史可 review、可离线重建、可归因，不依赖外部服务。

命令：
  rebuild --history <dir> [--db <path>]         从 history JSONL 重建 SQLite
  ingest  --input <file|dir> [--history <dir>]  把产物适配成 metrics-run-v1 入库
  report  [--domain perf|coverage|compile|binary|package|all] [--since 30d] [--format md|json]
  check   --policy <policy.json>                按域判红（样本不足退 3）
  query   --sql "select ..."                    只读查询（拒绝非 SELECT）
  prune   [--keep-days 730]                     只删派生库数据，不删 history
  selftest [--keep]                             正反自测（注入超阈值样本必须 exit 2）
  accept-baseline --input <file> --reason "..."  人工审阅后登记批准基线（要求工作区干净）

退出码：0 = 通过；1 = 错误（坏 schema / 解析失败 / 拒绝）；2 = 回归；3 = 样本不足（BLOCKED）。

适配器（输入原始产物 -> metrics-run-v1）：
  wp0_3d_stress / wp0_scene_frame / wp0_ecs_storage / ecs_stress / sprite_stress /
  tilemap_stress（通用 perf JSON）、jhyt_m19_soak、editor_perf（`--perf-baseline`
  表格文本，场景数随报告动态变化）、coverage-v1 / compile-v1 / binary-v1 / package-v1。
"""

import argparse
import hashlib
import json
import os
import re
import sqlite3
import sys
import zlib
from datetime import datetime, timezone, timedelta

SCHEMA = "metrics-run-v1"
DB_VERSION = 1
DB_TABLE = """CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT)"""
RUNS_TABLE = """CREATE TABLE IF NOT EXISTS runs (
  run_id TEXT PRIMARY KEY,
  schema TEXT NOT NULL,
  ts TEXT NOT NULL,
  domain TEXT NOT NULL,
  env_fingerprint TEXT NOT NULL,
  git_commit TEXT,
  git_dirty INTEGER DEFAULT 0,
  source_kind TEXT,
  source_path TEXT,
  source_sha256 TEXT,
  baseline INTEGER DEFAULT 0,
  raw TEXT
)"""
SAMPLES_TABLE = """CREATE TABLE IF NOT EXISTS samples (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL REFERENCES runs(run_id),
  domain TEXT NOT NULL,
  workload TEXT NOT NULL,
  metric TEXT NOT NULL,
  value REAL NOT NULL,
  unit TEXT,
  created_at TEXT NOT NULL
)"""
SAMPLES_INDEX = "CREATE INDEX IF NOT EXISTS idx_samples_dwm ON samples(domain, workload, metric, created_at)"
ARTIFACTS_TABLE = """CREATE TABLE IF NOT EXISTS artifacts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL REFERENCES runs(run_id),
  path TEXT,
  url TEXT,
  sha256 TEXT
)"""


def now_iso():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def git_state(project_root):
    """尽量读取 git 状态；非 git 仓库时返回 (None, False)。"""
    try:
        import subprocess
        c = subprocess.run(
            ["git", "-C", project_root, "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=10,
        )
        commit = c.stdout.strip() if c.returncode == 0 else None
        d = subprocess.run(
            ["git", "-C", project_root, "status", "--porcelain"],
            capture_output=True, text=True, timeout=10,
        )
        dirty = bool(d.stdout.strip())
        return commit, dirty
    except Exception:
        return None, False


def env_fingerprint_auto():
    try:
        import platform
        return "{}-{}".format(platform.system().lower(), platform.machine().lower())
    except Exception:
        return "unknown"


# ── 适配器 ─────────────────────────────────────────────────────────────────

def _json_file(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def adapt_wp0(path, project_root, env_fp):
    """通用 WP0 / stress 系列：wp0-v3/v4/v5、ecs_stress、sprite_stress、tilemap_stress。"""
    data = _json_file(path)
    rv = data.get("report_version", "?")
    bench = data.get("benchmark", os.path.basename(path))
    commit, dirty = git_state(project_root)
    samples = []
    stairs = data.get("stairs") or data.get("levels") or []
    for s in stairs:
        stair_key = str(s.get("mesh_count") or s.get("level") or s.get("n") or s.get("id") or "0")
        wl = "{}|{}".format(bench, stair_key)
        fs = s.get("samples") or s.get("frame_us") or {}
        for m in ("p50_us", "p95_us", "p99_us", "average_us", "min_us", "max_us"):
            if m in fs:
                samples.append({"workload": wl, "metric": m, "value": float(fs[m]), "unit": "us"})
        cpu = s.get("cpu_peak_bytes")
        if cpu is not None:
            samples.append({"workload": wl, "metric": "cpu_peak_bytes", "value": float(cpu), "unit": "bytes"})
        alloc = s.get("cpu_alloc_count")
        if alloc is not None:
            samples.append({"workload": wl, "metric": "cpu_alloc_count", "value": float(alloc), "unit": "count"})
        render = s.get("render") or {}
        for m, u in (("draw_calls", "count"), ("instances", "count"), ("triangles", "count")):
            if m in render:
                samples.append({"workload": wl, "metric": m, "value": float(render[m]), "unit": u})
    return build_run(
        path, "perf", "wp0_3d_stress" if rv == "wp0-v3" else "wp0_stress", samples,
        commit, dirty, env_fp, {"report_version": rv},
    )


def adapt_m19_soak(path, project_root, env_fp):
    data = _json_file(path)
    commit, dirty = git_state(project_root)
    samples = []
    wl = data.get("source", "jhyt_m19_soak")
    for m in ("per_step_ms", "soak_days", "harvested_total", "real_load_cycles",
              "disk_save_load_cycles", "snapshot_restores", "total_ms"):
        if m in data:
            unit = "ms" if m in ("per_step_ms", "total_ms") else "count"
            samples.append({"workload": wl, "metric": m, "value": float(data[m]), "unit": unit})
    return build_run(path, "perf", "jhyt_m19_soak", samples, commit, dirty, env_fp, {"task": data.get("task")})


EDITOR_SCENARIO_RE = re.compile(
    r"^(?P<name>\S+)\s+n=(?P<n>\d+)\s+last=\s*(?P<last>[\d.]+)\s+"
    r"p50=\s*(?P<p50>[\d.]+)\s+p95=\s*(?P<p95>[\d.]+)\s+"
    r"p99=\s*(?P<p99>[\d.]+)\s+max=\s*(?P<max>[\d.]+)\s+ms\s+(?P<status>ok|FAILED)$"
)
EDITOR_PROFILE_RE = re.compile(r"^profile=(\w+)")
EDITOR_SUMMARY_RE = re.compile(r"^scenarios=(\d+)\s+failed=(\d+)\s+result=(\w+)$")


def adapt_editor_perf(path, project_root, env_fp):
    """editor --perf-baseline 的表格文本。

    真实格式（`perf_baseline.rs::to_text`）：
        profile=debug os=windows
        <场景名>  n=5  last=... p50=... p95=... p99=... max=... ms  ok
        scenarios=35 failed=0 result=ok

    场景数随报告动态变化（1.2 版的适配器只认 `场景名 数值(us|ms)`，实际一行都解析不出来）。
    workload 里带 profile：debug 与 release 差 7~8 倍，混在同一条序列里会把同比做废。
    摘要里的 scenarios= 必须与实际解析出的场景数一致；不一致说明报告被截断或格式漂移，直接退 1。
    """
    commit, dirty = git_state(project_root)
    samples = []
    unparsed = 0
    profile = "unknown"
    declared = None
    names = set()
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            pm = EDITOR_PROFILE_RE.match(line)
            if pm:
                profile = pm.group(1)
                continue
            sm = EDITOR_SUMMARY_RE.match(line)
            if sm:
                declared = (int(sm.group(1)), int(sm.group(2)), sm.group(3))
                continue
            if line.startswith("===") or line.startswith("note:") or line.startswith("- "):
                continue
            m = EDITOR_SCENARIO_RE.match(line)
            if not m:
                unparsed += 1
                continue
            name = m.group("name")
            names.add(name)
            workload = "editor|{}|{}".format(profile, name)
            for metric in ("p50", "p95", "p99", "max"):
                samples.append({
                    "workload": workload,
                    "metric": "{}_us".format(metric),
                    "value": float(m.group(metric)) * 1000.0,
                    "unit": "us",
                })
            if m.group("status") != "ok":
                samples.append({"workload": workload, "metric": "failed", "value": 1.0, "unit": "count"})
    if not samples:
        raise RuntimeError("editor_perf 无有效场景行（报告格式漂移或文件被截断）: {}".format(path))
    if declared is None:
        raise RuntimeError("editor_perf 缺少 scenarios=/failed=/result= 摘要行: {}".format(path))
    if declared[0] != len(names):
        raise RuntimeError(
            "editor_perf 场景计数不符：摘要 scenarios={} 实际解析={}（报告被截断或格式漂移）".format(
                declared[0], len(names)
            )
        )
    return build_run(
        path, "perf", "editor_perf", samples, commit, dirty, env_fp,
        {
            "profile": profile,
            "scenario_count": len(names),
            "failed": declared[1],
            "result": declared[2],
            "source": {"kind": "unparsed" if unparsed else "parsed", "unparsed_lines": unparsed},
        },
    )


def _coverage_percent(block, legacy_key):
    """coverage-v1 的百分比取法。

    新版 `metrics/baselines/coverage.json` 的字段是嵌套的 `{count, covered, percent}`，
    而旧代码按扁平的 `lines_pct` 读 -> 历史库里 coverage 样本全是 0.0、覆盖率棘轮从未生效
    （2026-11-26 修复）。这里对两种形状都兼容，避免再出现"结构变了但读数静默变 0"。
    """
    if isinstance(block, dict):
        if "percent" in block:
            return float(block["percent"])
        if legacy_key in block:
            return float(block[legacy_key])
    elif isinstance(block, (int, float)):
        return float(block)
    return 0.0


def adapt_coverage(path, project_root, env_fp):
    data = _json_file(path)
    if data.get("schema") != "coverage-v1":
        raise RuntimeError("coverage 输入不是 coverage-v1: {}".format(path))
    commit, dirty = git_state(project_root)
    overall = data.get("overall", {})
    samples = [
        {"workload": "workspace", "metric": "lines_pct",
         "value": _coverage_percent(overall.get("lines"), "lines_pct"), "unit": "pct"},
        {"workload": "workspace", "metric": "functions_pct",
         "value": _coverage_percent(overall.get("functions"), "functions_pct"), "unit": "pct"},
        {"workload": "workspace", "metric": "regions_pct",
         "value": _coverage_percent(overall.get("regions"), "regions_pct"), "unit": "pct"},
    ]
    for crate in data.get("crates", []):
        wl = "crate|{}".format(crate.get("name"))
        # per-crate 用独立 metric 名：policy 的 `crate_lines_pct` 规则要能命中，
        # 否则 0.5pp 的 workspace 阈值会被套到每个 crate 上（粒度错）。
        samples.append({"workload": wl, "metric": "crate_lines_pct",
                        "value": _coverage_percent(crate.get("lines"), "lines_pct"), "unit": "pct"})
    return build_run(path, "coverage", "coverage", samples, commit, dirty, env_fp,
                     {"reported": data.get("reported", "unreported")})


def adapt_compile(path, project_root, env_fp):
    data = _json_file(path)
    if data.get("schema") != "compile-v1":
        raise RuntimeError("compile 输入不是 compile-v1: {}".format(path))
    commit, dirty = git_state(project_root)
    samples = []
    for m in ("check_cold", "check_warm", "test_build_cold", "release_build_cold",
              "incremental_after_clean_package", "change_radius_assetc", "change_radius_game"):
        if m in data:
            samples.append({"workload": m, "metric": "wall_ms", "value": float(data[m]), "unit": "ms"})
    units = data.get("units", {})
    for k, v in units.items():
        samples.append({"workload": "units|{}".format(k), "metric": "count", "value": float(v), "unit": "count"})
    return build_run(path, "compile", "compile", samples, commit, dirty, env_fp,
                     {"profile": data.get("profile"), "jobs": data.get("jobs")})


def adapt_binary(path, project_root, env_fp):
    data = _json_file(path)
    if data.get("schema") != "binary-v1":
        raise RuntimeError("binary 输入不是 binary-v1: {}".format(path))
    commit, dirty = git_state(project_root)
    samples = []
    for art in data.get("artifacts", []):
        wl = art.get("name", "?")
        for m in ("bytes_raw", "bytes_gzip9", "bytes_brotli11"):
            if m in art:
                samples.append({"workload": "artifact|{}".format(wl), "metric": m, "value": float(art[m]), "unit": "bytes"})
    return build_run(path, "binary", "binary", samples, commit, dirty, env_fp, {"partial": data.get("partial", False)})


def adapt_package(path, project_root, env_fp):
    data = _json_file(path)
    if data.get("schema") != "package-v1":
        raise RuntimeError("package 输入不是 package-v1: {}".format(path))
    commit, dirty = git_state(project_root)
    samples = []
    for pkg in data.get("packages", []):
        wl = "{}|{}|{}".format(pkg.get("platform", "?"), pkg.get("form", "?"), pkg.get("name", "?"))
        for m in ("bytes_total", "bytes_main", "bytes_sub"):
            if m in pkg:
                samples.append({"workload": wl, "metric": m, "value": float(pkg[m]), "unit": "bytes"})
    return build_run(path, "package", "package", samples, commit, dirty, env_fp, {"partial": data.get("partial", False)})


ADAPTERS = {
    "wp0_3d_stress": adapt_wp0,
    "wp0_scene_frame": adapt_wp0,
    "wp0_ecs_storage": adapt_wp0,
    "ecs_stress": adapt_wp0,
    "sprite_stress": adapt_wp0,
    "tilemap_stress": adapt_wp0,
    "jhyt_m19_soak": adapt_m19_soak,
    "editor_perf": adapt_editor_perf,
    "coverage": adapt_coverage,
    "compile": adapt_compile,
    "binary": adapt_binary,
    "package": adapt_package,
}


def build_run(path, domain, source_kind, samples, commit, dirty, env_fp, extra):
    ts = now_iso()
    payload = {
        "schema": SCHEMA,
        "run_id": None,
        "ts": ts,
        "domain": domain,
        "env": {"fingerprint": env_fp},
        "git": {"commit": commit, "dirty": bool(dirty)},
        "source": {"kind": source_kind, "path": path, "sha256": sha256_file(path)},
        "samples": samples,
    }
    if extra:
        payload["extra"] = extra
    # run_id 由稳定内容生成（排除 ts，保证同一次测量幂等、可去重）
    stable = json.dumps({k: v for k, v in payload.items() if k != "ts"}, sort_keys=True, default=str)
    payload["run_id"] = hashlib.sha256(stable.encode("utf-8")).hexdigest()[:12]
    return payload


def adapt_any(path, project_root):
    """按文件内容自动选适配器。已是 metrics-run-v1 的直接返回。"""
    with open(path, "r", encoding="utf-8") as f:
        head = f.read(4096)
    env_fp = env_fingerprint_auto()
    if '"schema"' in head and SCHEMA in head:
        data = _json_file(path)
        if data.get("schema") == SCHEMA:
            return data
    # 表格文本按表头嗅探（editor_perf 不是 JSON，先进这里）。
    if head.lstrip("\ufeff").lstrip().startswith("=== RustLua editor performance baseline"):
        return adapt_editor_perf(path, project_root, env_fp)
    # 按内容嗅探
    try:
        data = _json_file(path)
    except Exception:
        raise RuntimeError("无法解析输入: {}".format(path))
    if "report_version" in data:
        rv = data.get("report_version")
        if rv == "wp0-v3":
            return adapt_wp0(path, project_root, env_fp)
        if rv == "wp0-v4":
            return adapt_wp0(path, project_root, env_fp)
        if rv == "wp0-v5":
            return adapt_wp0(path, project_root, env_fp)
        raise RuntimeError("未知 report_version: {}（{}）".format(rv, path))
    if "task" in data and "jhyt_m19_soak" in json.dumps(data.get("source", "")):
        return adapt_m19_soak(path, project_root, env_fp)
    if data.get("schema") == "coverage-v1":
        return adapt_coverage(path, project_root, env_fp)
    if data.get("schema") == "compile-v1":
        return adapt_compile(path, project_root, env_fp)
    if data.get("schema") == "binary-v1":
        return adapt_binary(path, project_root, env_fp)
    if data.get("schema") == "package-v1":
        return adapt_package(path, project_root, env_fp)
    if isinstance(data, list) or (isinstance(data, dict) and "lines" in data):
        raise RuntimeError("无法确定适配器（列表/行文本缺少可识别表头）: {}".format(path))
    raise RuntimeError("无法确定适配器: {}".format(path))


# ── 数据库 ─────────────────────────────────────────────────────────────────

def connect(db_path):
    con = sqlite3.connect(db_path)
    con.execute(RUNS_TABLE)
    con.execute(SAMPLES_TABLE)
    con.execute(SAMPLES_INDEX)
    con.execute(ARTIFACTS_TABLE)
    con.execute(DB_TABLE)
    con.execute("INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(?,?)", (DB_VERSION, now_iso()))
    con.commit()
    return con


def ingest_run(con, run, history_dir):
    """校验 metrics-run-v1、按 run_id 去重、拒绝 git_dirty 的 baseline 更新。"""
    if run.get("schema") != SCHEMA:
        raise RuntimeError("坏 schema: 期望 {}，实际 {}".format(SCHEMA, run.get("schema")))
    rid = run.get("run_id")
    if not rid or not re.match(r"^[0-9a-f]{12}$", rid):
        raise RuntimeError("run_id 非法: {!r}".format(rid))
    if not run.get("samples"):
        raise RuntimeError("run {} 无 samples".format(rid))
    git = run.get("git", {})
    if run.get("baseline") and git.get("dirty"):
        raise RuntimeError("run {} 是 baseline 更新但 git_dirty，拒绝".format(rid))
    if con.execute("SELECT 1 FROM runs WHERE run_id=?", (rid,)).fetchone():
        return False  # 已存在，去重忽略
    env = run.get("env", {})
    src = run.get("source", {})
    con.execute(
        "INSERT INTO runs(run_id, schema, ts, domain, env_fingerprint, git_commit, git_dirty, "
        "source_kind, source_path, source_sha256, baseline, raw) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        (rid, run["schema"], run.get("ts"), run.get("domain", "?"), env.get("fingerprint", "unknown"),
         git.get("commit"), 1 if git.get("dirty") else 0,
         src.get("kind"), src.get("path"), src.get("sha256"),
         1 if run.get("baseline") else 0, json.dumps(run, ensure_ascii=False)),
    )
    for s in run.get("samples", []):
        con.execute(
            "INSERT INTO samples(run_id, domain, workload, metric, value, unit, created_at) "
            "VALUES(?,?,?,?,?,?,?)",
            (rid, run.get("domain", "?"), s.get("workload", "?"), s.get("metric", "?"),
             float(s["value"]), s.get("unit"), run.get("ts")),
        )
    for a in run.get("artifacts", []) or []:
        con.execute("INSERT INTO artifacts(run_id, path, url, sha256) VALUES(?,?,?,?)",
                    (rid, a.get("path"), a.get("url"), a.get("sha256")))
    con.commit()
    if history_dir:
        os.makedirs(history_dir, exist_ok=True)
        month = run.get("ts", now_iso())[:7]
        with open(os.path.join(history_dir, "{}.jsonl".format(month)), "a", encoding="utf-8") as f:
            f.write(json.dumps(run, ensure_ascii=False) + "\n")
    return True


def rebuild(history_dir, db_path):
    con = connect(db_path)
    con.execute("DELETE FROM samples")
    con.execute("DELETE FROM artifacts")
    con.execute("DELETE FROM runs")
    con.commit()
    n = 0
    for name in sorted(os.listdir(history_dir)):
        if not name.endswith(".jsonl"):
            continue
        with open(os.path.join(history_dir, name), "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                run = json.loads(line)
                if run.get("schema") != SCHEMA:
                    raise RuntimeError("history 中有非 {} 行（{}）".format(SCHEMA, name))
                ingest_run(con, run, None)
                n += 1
    return n


# ── 报告与检查 ────────────────────────────────────────────────────────────

def load_policy(policy_path):
    with open(policy_path, "r", encoding="utf-8") as f:
        return json.load(f)


def accept_baseline(input_path, reason, history_dir, db_path, project_root):
    """把一份已审阅的测量产物登记为**批准基线**（docs/70 §4 的 `--update` 流程）。

    - 要求 git 工作区干净（避免把未提交改动的读数当基线）；
    - run 带 `baseline: true` + `extra.baseline_reason`，写入 history 与派生库；
    - 之后 `check` 对 `compare_mode=baseline_delta` 的域**优先**用这条基线比较
      （不再被中间样本带偏；也避免"基线过期 ⇒ 每天都是新回归"的假红）。
    """
    commit, dirty = git_state(project_root)
    if dirty:
        raise RuntimeError("工作区不干净：基线更新要求 git status 干净（docs/70 §4 第 3 步）")
    run = adapt_any(input_path, project_root)
    run["baseline"] = True
    extra = dict(run.get("extra") or {})
    extra["baseline_reason"] = reason
    run["extra"] = extra
    # baseline 标记会进 payload，必须重算 run_id；否则同一次测量会与普通 run 撞 id 被去重。
    stable = json.dumps({k: v for k, v in run.items() if k != "ts"}, sort_keys=True, default=str)
    run["run_id"] = hashlib.sha256(stable.encode("utf-8")).hexdigest()[:12]
    con = connect(db_path)
    inserted = ingest_run(con, run, history_dir)
    return run["run_id"], inserted


def report(con, domain, since_days, fmt):
    since = (datetime.now(timezone.utc) - timedelta(days=since_days)).isoformat()
    if domain == "all":
        domains = ["perf", "coverage", "compile", "binary", "package"]
    else:
        domains = [domain]
    rows = []
    for d in domains:
        cur = con.execute(
            "SELECT workload, metric, value, unit, created_at FROM samples WHERE domain=? AND created_at>=? ORDER BY created_at, id",
            (d, since),
        )
        groups = {}
        for wl, m, v, u, ts in cur.fetchall():
            groups.setdefault((wl, m), []).append((ts, v, u))
        for (wl, m), pts in sorted(groups.items()):
            vals = [p[1] for p in pts]
            vals.sort()
            median = vals[len(vals) // 2] if vals else None
            last = vals[-1] if vals else None
            trend = ""
            if len(vals) >= 2:
                half = len(vals) // 2
                base = sorted(vals[:half])
                base_med = base[len(base) // 2]
                if last is not None and base_med:
                    pct = (last - base_med) / base_med * 100.0
                    trend = "{:+.1f}%".format(pct)
            rows.append({"domain": d, "workload": wl, "metric": m, "unit": u,
                         "n": len(pts), "median": median, "last": last, "delta_vs_first_half": trend})
    if fmt == "json":
        return json.dumps({"report": rows}, ensure_ascii=False, indent=2)
    lines = []
    lines.append("| domain | workload | metric | n | median | last | Δ vs 前半 |")
    lines.append("|---|---|---|---|---|---|---|")
    for r in rows:
        med = "{:.4g}".format(r["median"]) if r["median"] is not None else "-"
        last = "{:.4g}".format(r["last"]) if r["last"] is not None else "-"
        lines.append("| {} | {} | {} | {} | {} | {} | {} |".format(
            r["domain"], r["workload"], r["metric"], r["n"], med, last, r["delta_vs_first_half"]))
    return "\n".join(lines)


def rule_key_for(domain, workload, metric):
    """把样本的 (domain, workload, metric) 映射到 policy 该域 `rules` 的键。

    为什么需要：采样 schema 是"逐 artifact / 逐 package / 逐 crate 一条"，
    policy 的规则键却是 `exe/wasm/editor_exe/js`、`total/main/subpackage/component`。
    原实现直接 `rules.get(metric)`，两类键永远对不上 -> 二进制与包体的 +3% 红线
    在趋势库里从未触发过（2026-11-26 修复）。返回 `None` 表示该样本不受任何规则约束。
    """
    if domain == "binary":
        name = workload.split("|", 1)[-1].lower()
        if name.endswith(".wasm"):
            return "wasm"
        if name.endswith(".js"):
            return "js"
        if name.endswith(".exe"):
            return "editor_exe" if "editor" in name else "exe"
        return None
    if domain == "package":
        parts = workload.lower().split("|")
        platform = parts[0] if parts else ""
        name = parts[-1] if parts else ""
        if metric == "bytes_main":
            return "main"
        if metric == "bytes_sub":
            return "subpackage"
        if platform in ("rlb", "component"):
            return "component"
        if metric == "bytes_total" and (name.startswith("sub") or "|sub|" in workload.lower()):
            return "subpackage"
        return "total" if metric == "bytes_total" else None
    # 其它域（perf/coverage/compile/cfg）的采样 metric 名与 policy 规则键本来就一致。
    return metric


def check(con, policy, allowlist_path):
    """按域判红。

    口径（2026-11-26 收紧，详见 docs/70 §3.4）：
    - **同指纹**：只看与最新样本同一 `env_fingerprint` 的样本；跨机/跨 CI runner 的样本不参与
      比较（时间类读数跨机不可比，混进来只会制造假红/假绿）。
    - **compare_mode=baseline_delta 的确定性域**（coverage / binary / package）允许单样本比较：
      当前值 vs 上一条同指纹样本；时间类（trailing_median）仍需 `min_samples` 条同指纹样本，
      不足一律退 3（BLOCKED），不拿别的指纹凑数。
    - 规则键经 `rule_key_for` 映射后查 policy，不再是 `rules.get(metric)` 的直连。
    """
    min_samples = int(policy.get("min_samples", 3))
    domains = policy.get("domains", {})
    findings = []
    insufficient = []
    for domain, spec in domains.items():
        rules = spec.get("rules", {})
        compare_mode = spec.get("compare_mode", "same_fingerprint_trailing_median")
        default_min = 1 if compare_mode == "baseline_delta" else min_samples
        domain_min = int(spec.get("min_samples", default_min))
        cur_rows = con.execute(
            "SELECT s.workload, s.metric, s.value, s.created_at, r.env_fingerprint, r.baseline "
            "FROM samples s JOIN runs r ON r.run_id = s.run_id "
            "WHERE s.domain=? ORDER BY s.created_at DESC, s.id DESC",
            (domain,),
        ).fetchall()
        if not cur_rows:
            continue
        # 以最新一条样本的环境指纹为"当前环境"；旧机器/旧 runner 的样本留在库里供 report，
        # 但不参与判红。
        current_fp = cur_rows[0][4]
        groups = {}
        for wl, mt, v, _ts, fp, is_baseline in cur_rows:
            if fp != current_fp:
                continue
            groups.setdefault((wl, mt), []).append((v, bool(is_baseline)))
        for (wl, mt), samples in groups.items():
            rule = rules.get(rule_key_for(domain, wl, mt) or "")
            if not rule:
                continue
            vals = [value for value, _flag in samples]
            last_n = vals[:domain_min]
            if len(last_n) < domain_min:
                insufficient.append("{}|{}|{} n={} (need {})".format(
                    domain, wl, mt, len(last_n), domain_min))
                continue
            cur = vals[0]
            if compare_mode == "baseline_delta":
                # 已批准基线优先（`accept-baseline` 写入的带 baseline 标记的 run）；
                # 没有基线时退回"上一条同指纹样本"；只有一条时退化自比（不判红）。
                approved = [value for value, flag in samples if flag]
                if approved:
                    base = approved[0]
                elif len(vals) >= 2:
                    base = vals[1]
                else:
                    base = cur
            else:
                base = sorted(last_n)[len(last_n) // 2]
            better = rule.get("better", "lower")
            if rule.get("mode") == "any_change_red":
                if cur != base:
                    findings.append((domain, wl, mt, cur, base, "change"))
                continue
            thr_pct = rule.get("threshold_pct")
            if thr_pct is not None and base:
                if better == "lower" and cur > base * (1 + thr_pct / 100.0):
                    findings.append((domain, wl, mt, cur, base, "up"))
                elif better == "higher" and cur < base * (1 - thr_pct / 100.0):
                    findings.append((domain, wl, mt, cur, base, "down"))
            max_delta = rule.get("max_delta_pp")
            if max_delta is not None:
                if better == "higher" and cur < base - max_delta:
                    findings.append((domain, wl, mt, cur, base, "down_pp"))
                elif better == "lower" and cur > base + max_delta:
                    findings.append((domain, wl, mt, cur, base, "up_pp"))
    # allowlist 到期（expired_behavior=red -> 判 2，无黄期）
    expired_allow = []
    try:
        with open(allowlist_path, "r", encoding="utf-8") as f:
            allow = json.load(f)
        for entry in allow.get("entries", []) if isinstance(allow, dict) else []:
            try:
                d = datetime.fromisoformat(entry.get("review_by", ""))
                if d < datetime.now(timezone.utc):
                    expired_allow.append(entry.get("id", "?"))
            except Exception:
                pass
    except Exception:
        pass
    if insufficient:
        return 3, "insufficient samples: {}".format("; ".join(insufficient[:5]))
    if findings or expired_allow:
        msg = "; ".join("{}|{}|{} cur={:.4g} base={:.4g} ({})".format(*f) for f in findings[:5])
        if expired_allow:
            msg += "; allowlist expired: {}".format(",".join(expired_allow))
        return 2, msg
    return 0, "ok"


# ── 自测：正反两向证明"红线真的会红" ───────────────────────────────────────
#
# 为什么要有它：2026-11-26 复核发现趋势库的两类键位错配（coverage 嵌套字段 /
# binary+package 规则键）让"覆盖率 -0.5pp 判红""包体 +3% 判红"**从未触发过**。
# 看不见的红线等于没有红线，所以把"注入超阈值样本必须 exit 2"固化成自测，
# 由 check-metrics.ps1 每次跑（与 check-web-evidence-policy 的 --self-test 同思路）。

SELFTEST_POLICY = {
    "schema": 1,
    "min_samples": 3,
    "domains": {
        "coverage": {
            "compare_mode": "baseline_delta",
            "min_samples": 1,
            "rules": {
                "lines_pct": {"max_delta_pp": 0.5, "better": "higher"},
                "crate_lines_pct": {"max_delta_pp": 2.0, "better": "higher"},
            },
        },
        "binary": {
            "compare_mode": "baseline_delta",
            "rules": {"exe": {"threshold_pct": 3, "threshold_abs_bytes": 131072,
                              "better": "lower"}},
        },
        "package": {
            "compare_mode": "baseline_delta",
            "rules": {"total": {"threshold_pct": 3, "better": "lower"}},
        },
        "perf": {
            "compare_mode": "same_fingerprint_trailing_median",
            "min_samples": 3,
            "rules": {"p95_us": {"threshold_pct": 5, "better": "lower"}},
        },
    },
}


def run_selftest(keep=False):
    import shutil
    import tempfile

    tmp = tempfile.mkdtemp(prefix="trend-db-selftest-")
    failures = []
    checked = 0

    def write_json(path, obj):
        with open(path, "w", encoding="utf-8") as f:
            json.dump(obj, f, ensure_ascii=False)

    def make_run(domain, samples, minute, fingerprint="selftest-fp"):
        payload = {
            "schema": SCHEMA,
            "ts": "2026-11-26T00:{:02d}:00+00:00".format(minute),
            "domain": domain,
            "env": {"fingerprint": fingerprint},
            "git": {"commit": "selftest", "dirty": False},
            "source": {"kind": "selftest", "path": "selftest", "sha256": ""},
            "samples": samples,
        }
        stable = json.dumps({"payload": {k: v for k, v in payload.items() if k != "ts"},
                             "minute": minute},
                            sort_keys=True, default=str)
        payload["run_id"] = hashlib.sha256(stable.encode("utf-8")).hexdigest()[:12]
        return payload

    def db_from(runs, label):
        db = os.path.join(tmp, "selftest-{}.sqlite".format(label))
        con = connect(db)
        for run in runs:
            if not ingest_run(con, run, None):
                raise AssertionError("自测 run 未被入库（run_id 冲突）")
        return con

    def run_check(runs, label):
        con = db_from(runs, label)
        code, msg = check(con, SELFTEST_POLICY, allow_path)
        return code, msg

    def case(name, body):
        nonlocal checked
        checked += 1
        try:
            body()
            print("[PASS] self-test: " + name)
        except AssertionError as error:
            failures.append("{}：{}".format(name, error))

    policy_path = os.path.join(tmp, "policy.json")
    write_json(policy_path, SELFTEST_POLICY)
    allow_path = os.path.join(tmp, "allowlist.json")
    write_json(allow_path, {"entries": []})

    def coverage_adapter_reads_nested_percent():
        path = os.path.join(tmp, "coverage-adapter.json")
        write_json(path, {
            "schema": "coverage-v1",
            "overall": {"lines": {"percent": 80.99}, "functions": {"percent": 79.71},
                        "regions": {"percent": 80.89}},
            "crates": [{"name": "x", "lines": {"percent": 50.0}}],
            "reported": "reported",
        })
        run = adapt_any(path, os.path.dirname(tmp))
        values = {(s["workload"], s["metric"]): s["value"] for s in run["samples"]}
        assert values[("workspace", "lines_pct")] == 80.99, "coverage 嵌套 percent 未被读取：{}".format(values)
        assert values[("crate|x", "crate_lines_pct")] == 50.0, "crate 行应改用 crate_lines_pct：{}".format(values)

    def binary_adapter_rule_keys():
        path = os.path.join(tmp, "binary-adapter.json")
        write_json(path, {"schema": "binary-v1", "artifacts": [
            {"name": "game.exe", "bytes_raw": 1000},
            {"name": "editor.exe", "bytes_raw": 2000},
            {"name": "game_web.wasm", "bytes_raw": 3000},
            {"name": "web.js", "bytes_raw": 400},
        ]})
        run = adapt_any(path, os.path.dirname(tmp))
        keys = {(s["workload"], s["metric"]): rule_key_for("binary", s["workload"], s["metric"])
                for s in run["samples"]}
        assert keys[("artifact|game.exe", "bytes_raw")] == "exe", keys
        assert keys[("artifact|editor.exe", "bytes_raw")] == "editor_exe", keys
        assert keys[("artifact|game_web.wasm", "bytes_raw")] == "wasm", keys
        assert keys[("artifact|web.js", "bytes_raw")] == "js", keys

    def package_adapter_rule_keys():
        path = os.path.join(tmp, "package-adapter.json")
        write_json(path, {"schema": "package-v1", "packages": [
            {"platform": "pc", "form": "full", "name": "pc", "bytes_total": 1000},
            {"platform": "wechat", "form": "subpackage", "name": "main",
             "bytes_total": 1000, "bytes_main": 500, "bytes_sub": 500},
            {"platform": "rlb", "form": "resource", "name": "core.rlb", "bytes_total": 100},
        ]})
        run = adapt_any(path, os.path.dirname(tmp))
        keys = {(s["workload"], s["metric"]): rule_key_for("package", s["workload"], s["metric"])
                for s in run["samples"]}
        assert keys[("pc|full|pc", "bytes_total")] == "total", keys
        assert keys[("wechat|subpackage|main", "bytes_main")] == "main", keys
        assert keys[("wechat|subpackage|main", "bytes_sub")] == "subpackage", keys
        assert keys[("rlb|resource|core.rlb", "bytes_total")] == "component", keys

    def coverage_minus_one_pp_is_red():
        base = make_run("coverage", [
            {"workload": "workspace", "metric": "lines_pct", "value": 80.99, "unit": "pct"}], 0)
        newer = make_run("coverage", [
            {"workload": "workspace", "metric": "lines_pct", "value": 79.99, "unit": "pct"}], 1)
        code, msg = run_check([base, newer], "coverage-red")
        assert code == 2, "覆盖率 -1pp 应判 2，实际 {} ({})".format(code, msg)

    def coverage_half_pp_is_green():
        base = make_run("coverage", [
            {"workload": "workspace", "metric": "lines_pct", "value": 80.99, "unit": "pct"}], 0)
        newer = make_run("coverage", [
            {"workload": "workspace", "metric": "lines_pct", "value": 80.50, "unit": "pct"}], 1)
        code, msg = run_check([base, newer], "coverage-green")
        assert code == 0, "覆盖率 -0.49pp 应通过，实际 {} ({})".format(code, msg)

    def binary_plus_four_pct_is_red():
        base = make_run("binary", [
            {"workload": "artifact|game.exe", "metric": "bytes_raw", "value": 1000, "unit": "bytes"}], 0)
        newer = make_run("binary", [
            {"workload": "artifact|game.exe", "metric": "bytes_raw", "value": 1040, "unit": "bytes"}], 1)
        code, msg = run_check([base, newer], "binary-red")
        assert code == 2, "exe +4% 应判 2，实际 {} ({})".format(code, msg)

    def package_plus_four_pct_is_red():
        base = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 1000, "unit": "bytes"}], 0)
        newer = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 1040, "unit": "bytes"}], 1)
        code, msg = run_check([base, newer], "package-red")
        assert code == 2, "包体 +4% 应判 2，实际 {} ({})".format(code, msg)

    def other_fingerprint_is_ignored():
        base = make_run("binary", [
            {"workload": "artifact|game.exe", "metric": "bytes_raw", "value": 1000, "unit": "bytes"}], 0)
        newer = make_run("binary", [
            {"workload": "artifact|game.exe", "metric": "bytes_raw", "value": 9999, "unit": "bytes"}],
            1, fingerprint="other-machine")
        code, msg = run_check([base, newer], "fingerprint-isolated")
        assert code == 0, "跨指纹样本必须被忽略（否则 9999 会假红），实际 {} ({})".format(code, msg)

    def time_domain_insufficient_is_blocked():
        only = make_run("perf", [
            {"workload": "sprite_10000", "metric": "p95_us", "value": 1234, "unit": "us"}], 0)
        code, msg = run_check([only], "perf-insufficient")
        assert code == 3, "时间类域只有 1 个同指纹样本应退 3，实际 {} ({})".format(code, msg)

    def approved_baseline_wins():
        # 旧样本 1000 → 中间未批准样本 5000 → 批准基线 1000 → 当前 1900。
        # 只看"上一条"会以为从 5000 降到 1900（看似变好）；批准基线是 1000，
        # 当前 1900 是 +90% ⇒ 必须判 2。
        older = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 1000, "unit": "bytes"}], 0)
        middle = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 5000, "unit": "bytes"}], 1)
        approved = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 1000, "unit": "bytes"}], 2)
        approved["baseline"] = True
        current = make_run("package", [
            {"workload": "pc|full|pc", "metric": "bytes_total", "value": 1900, "unit": "bytes"}], 3)
        code, msg = run_check([older, middle, approved, current], "baseline-priority")
        assert code == 2, "批准基线应优先于'上一条样本'，实际 {} ({})".format(code, msg)

    case("coverage 适配器读取嵌套 percent（不再是 0.0）", coverage_adapter_reads_nested_percent)
    case("binary 采样映射到 policy 规则键（exe/editor_exe/wasm/js）", binary_adapter_rule_keys)
    case("package 采样映射到 policy 规则键（total/main/subpackage/component）", package_adapter_rule_keys)
    case("覆盖率 -1pp -> 2", coverage_minus_one_pp_is_red)
    case("覆盖率 -0.49pp -> 0", coverage_half_pp_is_green)
    case("exe +4% -> 2", binary_plus_four_pct_is_red)
    case("包体 +4% -> 2", package_plus_four_pct_is_red)
    case("跨指纹样本不参与判红", other_fingerprint_is_ignored)
    case("时间类域样本不足 -> 3（BLOCKED）", time_domain_insufficient_is_blocked)
    case("批准基线优先于'上一条样本'", approved_baseline_wins)

    if keep:
        print("[INFO] self-test 临时目录保留：{}".format(tmp))
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    if failures:
        for failure in failures:
            print("[FAIL] " + failure)
        return 1
    print("trend_db_selftest_ok cases={}".format(checked))
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description="RustLua 性能趋势数据库")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("rebuild")
    p.add_argument("--history", required=True)
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("ingest")
    p.add_argument("--input", required=True)
    p.add_argument("--history", default="")
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("report")
    p.add_argument("--domain", default="all")
    p.add_argument("--since", default="30d")
    p.add_argument("--format", default="md")
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("check")
    p.add_argument("--policy", required=True)
    p.add_argument("--allowlist", default="metrics/allowlist.json")
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("query")
    p.add_argument("--sql", required=True)
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("prune")
    p.add_argument("--keep-days", type=int, default=730)
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    p = sub.add_parser("selftest")
    p.add_argument("--keep", action="store_true", help="保留临时目录便于排查")

    p = sub.add_parser("accept-baseline")
    p.add_argument("--input", required=True, help="已审阅的测量产物（coverage/binary/package JSON）")
    p.add_argument("--reason", required=True, help="为什么接受这条基线（进 extra.baseline_reason）")
    p.add_argument("--history", default="metrics/history")
    p.add_argument("--db", default="artifacts/metrics/trend.sqlite")

    args = ap.parse_args(argv)
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    os.chdir(project_root)

    if args.cmd == "selftest":
        return run_selftest(keep=args.keep)

    if args.cmd == "accept-baseline":
        run_id, inserted = accept_baseline(args.input, args.reason, args.history, args.db, project_root)
        print("trend_db_ok accept_baseline run_id={} inserted={} reason={}".format(
            run_id, inserted, args.reason))
        return 0

    if args.cmd == "rebuild":
        n = rebuild(args.history, args.db)
        print("trend_db_ok rebuilt runs={}".format(n))
        return 0

    if args.cmd == "ingest":
        con = connect(args.db)
        if os.path.isdir(args.input):
            files = [os.path.join(args.input, f) for f in sorted(os.listdir(args.input))
                     if f.endswith((".json", ".jsonl", ".txt"))]
        else:
            files = [args.input]
        inserted = skipped = 0
        for f in files:
            try:
                run = adapt_any(f, project_root)
                if ingest_run(con, run, args.history or None):
                    inserted += 1
                else:
                    skipped += 1
            except Exception as e:
                print("trend_db_ingest_error file={} msg={}".format(f, e))
                return 1
        print("trend_db_ok ingested={} skipped_dup={}".format(inserted, skipped))
        return 0

    if args.cmd == "report":
        con = connect(args.db)
        since = args.since
        days = 30
        m = re.match(r"^(\d+)d$", since)
        if m:
            days = int(m.group(1))
        print(report(con, args.domain, days, args.format))
        return 0

    if args.cmd == "check":
        con = connect(args.db)
        code, msg = check(con, load_policy(args.policy), args.allowlist)
        if code == 0:
            print("trend_db_ok regressions=0 insufficient=0")
        elif code == 2:
            print("trend_db_regression {}".format(msg))
        else:
            print("trend_db_blocked insufficient=1 {}".format(msg))
        return code

    if args.cmd == "query":
        sql = args.sql.strip().lower()
        if not sql.startswith("select"):
            print("trend_db_error query 只允许 SELECT")
            return 1
        con = connect(args.db)
        cur = con.execute(args.sql)
        cols = [d[0] for d in cur.description] if cur.description else []
        for row in cur.fetchall():
            print(json.dumps(dict(zip(cols, row)), ensure_ascii=False, default=str))
        return 0

    if args.cmd == "prune":
        con = connect(args.db)
        cutoff = (datetime.now(timezone.utc) - timedelta(days=args.keep_days)).isoformat()
        con.execute("DELETE FROM samples WHERE created_at < ?", (cutoff,))
        con.execute("DELETE FROM artifacts WHERE run_id NOT IN (SELECT DISTINCT run_id FROM samples)")
        con.execute("DELETE FROM runs WHERE run_id NOT IN (SELECT DISTINCT run_id FROM samples)")
        con.commit()
        print("trend_db_ok pruned keep_days={}".format(args.keep_days))
        return 0

    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except SystemExit:
        raise
    except Exception as e:
        print("trend_db_error {}".format(e))
        sys.exit(1)
