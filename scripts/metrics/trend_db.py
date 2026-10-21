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

退出码：0 = 通过；1 = 错误（坏 schema / 解析失败 / 拒绝）；2 = 回归；3 = 样本不足（BLOCKED）。

适配器（输入原始产物 -> metrics-run-v1）：
  wp0_3d_stress / wp0_scene_frame / wp0_ecs_storage / ecs_stress / sprite_stress /
  tilemap_stress（通用 perf JSON）、jhyt_m19_soak、editor_perf（31 场景文本）、
  coverage-v1 / compile-v1 / binary-v1 / package-v1。
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


def adapt_editor_perf(path, project_root, env_fp):
    """editor --perf-baseline 的 31 场景文本：每行 `场景名 <数值>(us|ms)`。

    真实输出格式以 editor 实际打印为准；解析失败标 source.kind=unparsed 并退 1。
    """
    commit, dirty = git_state(project_root)
    samples = []
    unparsed = 0
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            m = re.match(r"^(\S+)\s+([\d.]+)\s*(us|ms)?$", line)
            if not m:
                unparsed += 1
                continue
            name, val, unit = m.group(1), float(m.group(2)), (m.group(3) or "us")
            if unit == "ms":
                val = val * 1000.0
                unit = "us"
            samples.append({"workload": "editor|{}".format(name), "metric": "p95_us", "value": val, "unit": unit})
    if not samples:
        raise RuntimeError("editor_perf 无有效行（或全部 unparsed）")
    return build_run(path, "perf", "editor_perf", samples, commit, dirty, env_fp,
                     {"source": {"kind": "unparsed" if unparsed else "parsed", "unparsed_lines": unparsed}})


def adapt_coverage(path, project_root, env_fp):
    data = _json_file(path)
    if data.get("schema") != "coverage-v1":
        raise RuntimeError("coverage 输入不是 coverage-v1: {}".format(path))
    commit, dirty = git_state(project_root)
    overall = data.get("overall", {})
    samples = [
        {"workload": "workspace", "metric": "lines_pct", "value": float(overall.get("lines_pct", 0)), "unit": "pct"},
        {"workload": "workspace", "metric": "functions_pct", "value": float(overall.get("functions_pct", 0)), "unit": "pct"},
        {"workload": "workspace", "metric": "regions_pct", "value": float(overall.get("regions_pct", 0)), "unit": "pct"},
    ]
    for crate in data.get("crates", []):
        wl = "crate|{}".format(crate.get("name"))
        samples.append({"workload": wl, "metric": "lines_pct", "value": float(crate.get("lines_pct", 0)), "unit": "pct"})
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
        raise RuntimeError("无法确定适配器（疑似 editor_perf 文本，需扩展）: {}".format(path))
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


def check(con, policy, allowlist_path):
    """按域判红。同指纹最近 min_samples 个 run 的中位数为参考；样本不足退 3；
    allowlist 到期判 2；覆盖率/几何量/字节数仍判红，时间类可标 advisory。"""
    min_samples = int(policy.get("min_samples", 3))
    domains = policy.get("domains", {})
    findings = []
    insufficient = []
    for domain, spec in domains.items():
        rules = spec.get("rules", {})
        cur_rows = con.execute(
            "SELECT workload, metric, value, created_at FROM samples WHERE domain=? ORDER BY created_at DESC, id DESC",
            (domain,),
        ).fetchall()
        if not cur_rows:
            continue
        groups = {}
        for wl, mt, v, ts in cur_rows:
            groups.setdefault((wl, mt), []).append(v)
        for (wl, mt), vals in groups.items():
            rule = rules.get(mt)
            if not rule:
                continue
            last_n = vals[:min_samples]
            if len(last_n) < min_samples:
                insufficient.append("{}|{}|{} n={}".format(domain, wl, mt, len(last_n)))
                continue
            base = sorted(last_n)[len(last_n) // 2]
            cur = vals[0]
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

    args = ap.parse_args(argv)
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    os.chdir(project_root)

    if args.cmd == "rebuild":
        n = rebuild(args.history, args.db)
        print("trend_db_ok rebuilt runs={}".format(n))
        return 0

    if args.cmd == "ingest":
        con = connect(args.db)
        if os.path.isdir(args.input):
            files = [os.path.join(args.input, f) for f in sorted(os.listdir(args.input))
                     if f.endswith((".json", ".jsonl"))]
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
