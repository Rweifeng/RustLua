#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""test-trend-db.py —— trend_db 自测（P4-04，6 条）。

用例：
  1. 同域注入 +8%（超 p95 5% 阈值）-> check 判 2
  2. 样本不足（min_samples=3 只有 2 个）-> 退 3
  3. 重复 run_id -> ingest 去重忽略（skipped_dup=1）
  4. 坏 schema -> ingest 退 1
  5. git_dirty 的 baseline 更新 -> ingest 拒绝（退 1）
  6. trailing median 对离群点稳健：1 个离群点被中位数吸收 -> 不判红

输出：trend_db_selftest_ok cases=6（全过）或 trend_db_selftest_failed + 非零退出。
"""

import json
import os
import re
import hashlib
import subprocess
import sys
import tempfile
import shutil

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TREND = os.path.join(ROOT, "scripts", "metrics", "trend_db.py")
POLICY = os.path.join(ROOT, "metrics", "policy.json")


def run_trend(*args):
    return subprocess.run([sys.executable, TREND] + list(args),
                          capture_output=True, text=True, cwd=ROOT)


def make_run(run_id, domain="perf", samples=None, baseline=False, git_dirty=False, schema="metrics-run-v1"):
    if not re.match(r"^[0-9a-f]{12}$", run_id):
        run_id = hashlib.sha256(run_id.encode()).hexdigest()[:12]
    return {
        "schema": schema,
        "run_id": run_id,
        "ts": "2026-10-20T00:00:00+00:00",
        "domain": domain,
        "env": {"fingerprint": "win-x64|test"},
        "git": {"commit": "test", "dirty": git_dirty},
        "source": {"kind": "test", "path": "selftest", "sha256": "0" * 64},
        "samples": samples or [{"workload": "w1", "metric": "p95_us", "value": 100.0, "unit": "us"}],
        "baseline": baseline,
    }


def write(path, obj):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False)


def main():
    tmp = tempfile.mkdtemp(prefix="trend-db-test-")
    failed = 0

    try:
        # ── 用例 2：样本不足退 3 ─────────────────────────────────────────
        db2 = os.path.join(tmp, "t2.sqlite")
        for i in range(2):
            r = make_run("run2-{:02d}".format(i), samples=[{"workload": "w1", "metric": "p95_us", "value": 100.0 + i, "unit": "us"}])
            write(os.path.join(tmp, "in2-{}.json".format(i)), r)
            res = run_trend("ingest", "--input", os.path.join(tmp, "in2-{}.json".format(i)), "--db", db2)
            if res.returncode != 0:
                print("[FAIL] t2 ingest: {}".format(res.stdout + res.stderr))
                return 1
        res = run_trend("check", "--policy", POLICY, "--db", db2)
        if res.returncode != 3:
            print("[FAIL] t2 样本不足应退 3，实际 {}".format(res.returncode))
            failed += 1
        else:
            print("[PASS] t2 样本不足退 3")

        # ── 用例 3：重复 run_id 忽略 ────────────────────────────────────
        db3 = os.path.join(tmp, "t3.sqlite")
        r = make_run("run3-dup", samples=[{"workload": "w1", "metric": "p95_us", "value": 200.0, "unit": "us"}])
        p3 = os.path.join(tmp, "in3.json")
        write(p3, r)
        res = run_trend("ingest", "--input", p3, "--db", db3)
        if res.returncode != 0:
            print("[FAIL] t3 首次 ingest: {}".format(res.stdout + res.stderr))
            return 1
        res = run_trend("ingest", "--input", p3, "--db", db3)
        if res.returncode != 0 or "skipped_dup=1" not in res.stdout:
            print("[FAIL] t3 重复应 skipped_dup=1: {}".format(res.stdout))
            failed += 1
        else:
            print("[PASS] t3 重复 run_id 忽略")

        # ── 用例 4：坏 schema 退 1 ──────────────────────────────────────
        db4 = os.path.join(tmp, "t4.sqlite")
        bad = make_run("run4-bad", schema="not-metrics-run-v1")
        p4 = os.path.join(tmp, "in4.json")
        write(p4, bad)
        res = run_trend("ingest", "--input", p4, "--db", db4)
        if res.returncode != 1:
            print("[FAIL] t4 坏 schema 应退 1，实际 {}".format(res.returncode))
            failed += 1
        else:
            print("[PASS] t4 坏 schema 退 1")

        # ── 用例 5：git_dirty 的 baseline 更新被拒 ──────────────────────
        db5 = os.path.join(tmp, "t5.sqlite")
        dirty_base = make_run("run5-dirty", baseline=True, git_dirty=True)
        p5 = os.path.join(tmp, "in5.json")
        write(p5, dirty_base)
        res = run_trend("ingest", "--input", p5, "--db", db5)
        if res.returncode != 1 or "git_dirty" not in (res.stdout + res.stderr):
            print("[FAIL] t5 dirty baseline 应拒绝: {}".format(res.stdout + res.stderr))
            failed += 1
        else:
            print("[PASS] t5 git_dirty 不能更新基线")

        # ── 用例 1：+8% 判 2（p95 阈值 5%）──────────────────────────────
        db1 = os.path.join(tmp, "t1.sqlite")
        # 基线：3 个同 workload 样本（值 100, 100, 100），中位数 100
        for i in range(3):
            r = make_run("run1-base-{:02d}".format(i), samples=[{"workload": "w1", "metric": "p95_us", "value": 100.0, "unit": "us"}])
            write(os.path.join(tmp, "in1b-{}.json".format(i)), r)
            res = run_trend("ingest", "--input", os.path.join(tmp, "in1b-{}.json".format(i)), "--db", db1)
            if res.returncode != 0:
                print("[FAIL] t1 基线 ingest: {}".format(res.stdout + res.stderr))
                return 1
        # 最新一次 +8%（108 > 100*1.05=105）-> 判 2
        r = make_run("run1-hot", samples=[{"workload": "w1", "metric": "p95_us", "value": 108.0, "unit": "us"}])
        write(os.path.join(tmp, "in1h.json"), r)
        res = run_trend("ingest", "--input", os.path.join(tmp, "in1h.json"), "--db", db1)
        if res.returncode != 0:
            print("[FAIL] t1 hot ingest: {}".format(res.stdout + res.stderr))
            return 1
        res = run_trend("check", "--policy", POLICY, "--db", db1)
        if res.returncode != 2:
            print("[FAIL] t1 +8% 应判 2，实际 {}".format(res.returncode))
            failed += 1
        else:
            print("[PASS] t1 +8% 判 2")

        # ── 用例 6：trailing median 对离群点稳健 ────────────────────────
        db6 = os.path.join(tmp, "t6.sqlite")
        # 基线 3 个样本：100, 100, 500（离群点）。中位数=100。最新 104（+4% < 5%）不判红。
        for i, v in enumerate((100.0, 100.0, 500.0)):
            r = make_run("run6-{:02d}".format(i), samples=[{"workload": "w1", "metric": "p95_us", "value": v, "unit": "us"}])
            write(os.path.join(tmp, "in6-{}.json".format(i)), r)
            res = run_trend("ingest", "--input", os.path.join(tmp, "in6-{}.json".format(i)), "--db", db6)
            if res.returncode != 0:
                print("[FAIL] t6 ingest: {}".format(res.stdout + res.stderr))
                return 1
        r = make_run("run6-hot", samples=[{"workload": "w1", "metric": "p95_us", "value": 104.0, "unit": "us"}])
        write(os.path.join(tmp, "in6h.json"), r)
        res = run_trend("ingest", "--input", os.path.join(tmp, "in6h.json"), "--db", db6)
        if res.returncode != 0:
            print("[FAIL] t6 hot ingest: {}".format(res.stdout + res.stderr))
            return 1
        res = run_trend("check", "--policy", POLICY, "--db", db6)
        if res.returncode != 0:
            print("[FAIL] t6 离群点应被中位数吸收（应 0），实际 {}".format(res.returncode))
            failed += 1
        else:
            print("[PASS] t6 trailing median 稳健")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("")
    if failed > 0:
        print("trend_db_selftest_failed cases=6 failures={}".format(failed))
        return 1
    print("trend_db_selftest_ok cases=6")
    return 0


if __name__ == "__main__":
    sys.exit(main())
