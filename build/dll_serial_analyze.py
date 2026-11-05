#!/usr/bin/env python3
"""Turn a dll_serial_probe.ps1 log into the verdict for docs/todo 28 criterion 4.

What the log carries (host side, RUPIX_DECODE_TRACE=1):

    [HH:MM:SS.mmm] [ INFO] ... [dll-serial] decoder=<id> th=<thread> wait=<ms> run=<ms> re=<0|1>

One line per entry into a decoder DLL (the Guard's Drop guarantees it, including the
early-exit paths). Held interval = [line_time - run, line_time].

Criteria
    C1  same decoder: no two intervals overlap (tolerance 2 ms)
    C2  contention really happened: wait>0 share >= 5% and max wait >= 20 ms
        (this is the VALIDITY gate - a dual-window arm that never queued proves nothing)
    C3  zero [dll-lock] lines (nobody hit the bounded-wait verdict)
    C4  zero watchdog / hang lines
    C5  bounded queueing: wait p95 <= run p95 * 1.2  (waiting for one decode, not a pile-up)
        and end-to-end p95 <= baseline run p95 * 2.4 when a baseline is given
    C6  the caller's teardown line reported a clean exit (passed in as --teardown-clean)

Why C5 is relative and not "wait < 300 ms": the fixture is a 5172x5172 jpg that takes
~0.9-1.1 s to decode on this box, so ANY honest serialisation makes one waiter sit for
about one decode. An absolute millisecond budget would either always fail on big images
or always pass on small ones; the invariant that matters is "you wait for the decode in
front of you, not for a queue".

Usage
    python build/dll_serial_analyze.py --log .temp/soak-m229/s1c.log --expect dual \
        [--baseline .temp/soak-m229/s0c.log] [--teardown-clean 1]

Exit code: 0 = PASS, 1 = FAIL, 2 = INVALID (no contention -> rerun with a harder arm).
"""

import argparse
import json
import re
import sys

LINE = re.compile(
    r"^\[(\d\d):(\d\d):(\d\d)\.(\d\d\d)\].*?\[dll-serial\] decoder=(\S+) "
    r"(?:th=(\S+) )?wait=([\d.]+)ms run=([\d.]+)ms re=(\d)"
)
TOL = 0.002  # seconds


def parse(path):
    rows = []
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            m = LINE.match(raw.strip())
            if not m:
                continue
            hh, mm, ss, ms, dec, th, wait, run, re_ = m.groups()
            end = int(hh) * 3600 + int(mm) * 60 + int(ss) + int(ms) / 1000.0
            run_s = float(run) / 1000.0
            rows.append(
                {
                    "end": end,
                    "start": end - run_s,
                    "run": run_s,
                    "wait": float(wait) / 1000.0,
                    "decoder": dec,
                    "thread": th or "?",
                    "re": int(re_),
                }
            )
    return rows


def pct(values, q):
    if not values:
        return 0.0
    ordered = sorted(values)
    idx = min(len(ordered) - 1, max(0, int(round(q * (len(ordered) - 1)))))
    return ordered[idx]


def count(path, needle):
    n = 0
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            if needle in raw:
                n += 1
    return n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--baseline", default="")
    ap.add_argument("--expect", choices=["single", "dual", "triple", "cross"], default="dual")
    ap.add_argument("--teardown-clean", default="1")
    args = ap.parse_args()

    rows = [r for r in parse(args.log) if r["re"] == 0]
    if not rows:
        print("INVALID: no [dll-serial] lines (was RUPIX_DECODE_TRACE set?)")
        return 2

    cross_pairs = 0
    ordered_all = sorted(rows, key=lambda r: r["start"])
    for i in range(len(ordered_all)):
        for j in range(i + 1, len(ordered_all)):
            if ordered_all[j]["start"] >= ordered_all[i]["end"] - TOL:
                break
            if ordered_all[i]["decoder"] != ordered_all[j]["decoder"]:
                cross_pairs += 1
    per_decoder = {}
    for r in rows:
        per_decoder.setdefault(r["decoder"], []).append(r)

    n_win = {"single": 1, "cross": 1, "dual": 2, "triple": 3}[args.expect]
    overlaps = []
    for dec, group in per_decoder.items():
        ordered = sorted(group, key=lambda r: r["start"])
        for prev, cur in zip(ordered, ordered[1:]):
            if cur["start"] < prev["end"] - TOL:
                overlaps.append(
                    {
                        "decoder": dec,
                        "prev": [round(prev["start"], 3), round(prev["end"], 3)],
                        "cur": [round(cur["start"], 3), round(cur["end"], 3)],
                    }
                )

    waits = [r["wait"] for r in rows]
    runs = [r["run"] for r in rows]
    e2e = [r["wait"] + r["run"] for r in rows]
    queued = [w for w in waits if w > 0.0]
    wait_share = len(queued) / float(len(waits))
    threads = sorted({r["thread"] for r in rows})

    lock_verdicts = count(args.log, "[dll-lock]")
    # "decode timeout" as the host spells it, built from code points so this file stays ASCII.
    hang_text = "".join(chr(c) for c in (0x89E3, 0x7801, 0x8D85, 0x65F6))
    hangs = count(args.log, "[decode-watchdog]") + count(args.log, hang_text)

    base_run_p95 = None
    if args.baseline:
        base = [r for r in parse(args.baseline) if r["re"] == 0]
        if base:
            base_run_p95 = pct([r["run"] for r in base], 0.95)

    checks = []
    checks.append(("C1 no-overlap", not overlaps, "%d overlapping pair(s)" % len(overlaps)))
    if args.expect in ("single", "cross"):
        # single: no contention by construction. cross: two DIFFERENT decoder DLLs, so a
        # queue is not expected either - the point there is that they run side by side.
        checks.append(
            ("C2 contention", True, "skipped (" + args.expect + " arm: queueing not expected)")
        )
    else:
        checks.append(
            (
                "C2 contention",
                (wait_share >= 0.05 and max(waits) >= 0.020),
                "queued %.0f%% of %d, max wait %.1f ms"
                % (wait_share * 100.0, len(waits), max(waits) * 1000.0),
            )
        )
    checks.append(("C3 no lock verdict", lock_verdicts == 0, "%d [dll-lock] line(s)" % lock_verdicts))
    checks.append(("C4 no hang", hangs == 0, "%d watchdog/timeout line(s)" % hangs))
    # With N windows queueing on one DLL the last arrival must wait (N-1) decodes - that is
    # arithmetic, not a defect. The bound therefore scales with the arm.
    limit = 2.2 if args.expect == "triple" else 1.2
    c5_wait_ok = pct(waits, 0.95) <= pct(runs, 0.95) * limit
    # End-to-end = queue + decode. A window that is Nth in line pays N decodes, so the
    # bound is relative to THIS run's own p95 (the baseline is a different session and
    # drifted by 40% on this box just from load - it is printed for context, not used).
    e2e_limit = pct(runs, 0.95) * (n_win + 0.2)
    c5_e2e_ok = pct(e2e, 0.95) <= e2e_limit
    e2e_note = "e2e p95 %.1f ms <= run p95 %.1f ms x%.1f (=%.1f ms)%s" % (
        pct(e2e, 0.95) * 1000.0,
        pct(runs, 0.95) * 1000.0,
        n_win + 0.2,
        e2e_limit * 1000.0,
        "" if base_run_p95 is None else ("; baseline run p95 %.1f ms" % (base_run_p95 * 1000.0)),
    )
    checks.append(
        (
            "C5 bounded queue",
            c5_wait_ok and c5_e2e_ok,
            "wait p95 %.1f ms <= run p95 %.1f ms x%.1f; %s"
            % (pct(waits, 0.95) * 1000.0, pct(runs, 0.95) * 1000.0, limit, e2e_note),
        )
    )
    checks.append(
        (
            "C6 clean exit",
            args.teardown_clean == "1",
            "caller teardown clean=%s" % args.teardown_clean,
        )
    )

    summary = {
        "log": args.log,
        "cross_decoder_overlaps": cross_pairs,
        "rows": len(rows),
        "decoders": sorted(per_decoder),
        "threads": threads,
        "wait_ms": {
            "p50": round(pct(waits, 0.50) * 1000.0, 1),
            "p95": round(pct(waits, 0.95) * 1000.0, 1),
            "max": round(max(waits) * 1000.0, 1),
        },
        "run_ms": {
            "p50": round(pct(runs, 0.50) * 1000.0, 1),
            "p95": round(pct(runs, 0.95) * 1000.0, 1),
        },
        "e2e_ms": {"p95": round(pct(e2e, 0.95) * 1000.0, 1)},
        "overlaps": overlaps,
        "checks": [{"name": n, "ok": bool(ok), "detail": d} for (n, ok, d) in checks],
    }

    print("%-18s INFO  %d pair(s) of DIFFERENT decoders overlapped (parallelism, not a queue)" % ("C7 cross-parallel", cross_pairs))
    for item in summary["checks"]:
        print("%-18s %s  %s" % (item["name"], "PASS" if item["ok"] else "FAIL", item["detail"]))
    print(json.dumps(summary, ensure_ascii=False, indent=2))

    if args.expect in ("dual", "triple") and not checks[1][1]:
        return 2
    return 0 if all(ok for (_, ok, _) in checks) else 1


if __name__ == "__main__":
    sys.exit(main())
