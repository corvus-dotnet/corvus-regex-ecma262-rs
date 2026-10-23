#!/usr/bin/env python3
"""The count gate: every workload, counted by this library's engines in a harness run with V8 beside them, must give
V8's count. A harness marks an ECMA-262 engine whose count differs from V8's as an error ("counted N where V8 counts
M") and drops its figure, which the central page shows as a wrong count; this gate fails CI on one, and on any figure
that is missing or failed, so that a wrong count is found on the pull request and not on the nightly page.

    count-gate.py <workloads.json> <the harness's --out results.json> <engine> [<engine> ...]

The harness is run first, over every workload, with the engines named here and v8, at one round of the least timing
(the figures are not the point; the counts are).

Python 3 standard library only.
"""
import json
import sys


def main():
    if len(sys.argv) < 4:
        raise SystemExit(__doc__.strip().splitlines()[5].strip())
    workloads = [w["id"] for w in json.load(open(sys.argv[1], encoding="utf-8"))]
    results = json.load(open(sys.argv[2], encoding="utf-8"))["results"]
    v8 = results.get("v8", {})
    bad = []
    for engine in sys.argv[3:]:
        rows = results.get(engine)
        if rows is None:
            bad.append(f"{engine}: not in the run")
            continue
        for w in workloads:
            cell = rows.get(w)
            oracle = (v8.get(w) or {}).get("count")
            if not cell:
                bad.append(f"{engine} {w}: no figure")
            elif "error" in cell:
                bad.append(f"{engine} {w}: {cell['error']}")
            elif oracle is not None and cell.get("count") != oracle:
                bad.append(f"{engine} {w}: counted {cell.get('count')} where V8 counts {oracle}")
    counted = len(workloads) * (len(sys.argv) - 3)
    if bad:
        print(f"count gate: {len(bad)} of {counted} wrong or missing")
        for line in bad:
            print("  " + line)
        raise SystemExit(1)
    print(f"count gate: {counted} counts, every one V8's")


if __name__ == "__main__":
    main()
