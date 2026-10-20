#!/usr/bin/env python3
"""Writes the data file of an Industry benchmark run, in the shape of the results page
(tools/benchmarks/results-page/generate_site.py documents it: meta, vocab, measures, impls, schemas, results). The
central benchmark site (https://corvus-oss.org/corvus-regex-ecma262-benchmarks/) runs this repository's harness image
each night and builds the one page of figures from its data file; this repository publishes no results of its own.

Usage: industry-results.py [--workloads <workloads.json>] --run <harness results json> --v8 <V8 results json> --out <data file>
                           --compliance-corvus <runner output> --compliance-regress <runner output>
                           --compliance-v8 <tools/benchmarks/v8/dataset.mjs output> [--run-url <url>]

The run is the output of the harness (`cargo run --release --bin industry -- tools/benchmarks/workloads.json --out
run.json`), whose facts are the run's own record of itself: the processor, the cores it was given, the system, the
load when it started, the runner when it is a GitHub-hosted one, the compiler, the engines' versions, the commit, the
seconds and passes of each figure, and the time it ended. V8's results are those of tools/benchmarks/v8/industry.mjs,
which names its engine. Nothing is typed in: what is not in a record is not said.

Each engine that claims ECMA-262 carries its compliance over the ECMA-262 test data, from the last line of its
data-set runner: this crate's and regress's by `cargo run --release --bin dataset` (with `--engine regress`), V8's by
tools/benchmarks/v8/dataset.mjs. regex does not claim it, and carries "not ECMA-262".
"""
import argparse, json, os, re

ap = argparse.ArgumentParser()
ap.add_argument("--run", required=True)
ap.add_argument("--v8", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--compliance-corvus", required=True)
ap.add_argument("--compliance-regress", required=True)
ap.add_argument("--compliance-v8", required=True)
ap.add_argument("--run-url", default="")
ap.add_argument("--workloads", help="the workloads.json of the run (tools/benchmarks/workloads.json when not given)")
args = ap.parse_args()

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
run = json.load(open(args.run, encoding="utf-8"))
v8 = json.load(open(args.v8, encoding="utf-8"))
facts = run["facts"]
workloads = json.load(open(args.workloads or os.path.join(root, "tools", "benchmarks", "workloads.json"), encoding="utf-8"))
ids = [w["id"] for w in workloads]


def compliance(path, note=None):
    """The counts of a data-set runner's last line of counts. Every answer the engine never gave (a limit, a crash, a
    hang or a worker that gave up) is in `limited`, as the judging rule of the test data counts it: the runner's
    limit and errors."""
    lines = [l for l in open(path, encoding="utf-8").read().splitlines() if l.startswith("records ")]
    if not lines:
        raise SystemExit(f"{path} has no line of counts")
    c = {k: int(v) for k, v in re.findall(r"(right|wrong|refused-valid|accepted-invalid|limit|errors) (\d+)", lines[-1])}
    out = {"right": c["right"], "wrong": c["wrong"], "refused": c["refused-valid"], "acceptedInvalid": c["accepted-invalid"],
           "limited": c.get("limit", 0) + c.get("errors", 0)}
    if note:
        out["note"] = note
    return out


ENGINES = [
    ("corvus-utf8", "corvus-regex-ecma262 UTF-8", "Rust", facts.get("corvus-regex-ecma262", ""), compliance(args.compliance_corvus)),
    ("corvus-utf16", "corvus-regex-ecma262 UTF-16", "Rust", facts.get("corvus-regex-ecma262", ""), compliance(args.compliance_corvus)),
    ("regex", "regex", "Rust", facts.get("regex", ""), "not ECMA-262"),
    ("regress", "regress", "Rust", facts.get("regress", ""), compliance(args.compliance_regress,
        "captures.jsonl is judged over UTF-16 only, since regress has no UTF-8 interface")),
]
results = {}
for key, name, _, _, _ in ENGINES:
    rows = run["results"].get(key, {})
    results[name] = {}
    for w in ids:
        r = rows.get(w)
        if r is None:
            continue
        results[name][w] = None if "error" in r else {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}}
v8_name = "V8"
ENGINES.append(("v8", v8_name, "V8", v8["engine"], compliance(args.compliance_v8)))
results[v8_name] = {}
for w in ids:
    r = v8["results"].get(w)
    results[v8_name][w] = {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}} if r else None

TEXTS = {"sherlock": "Sherlock Holmes (0.6 M chars)", "leipzig": "Leipzig corpus (16 M chars)", "mariomka": "mariomka (0.05 M chars)",
         "rebar-ru": "OpenSubtitles ru (1.2 M chars)", "rebar-zh": "OpenSubtitles zh (0.3 M chars)"}
runner = " ".join(x for x in (facts.get("runner", ""), facts.get("runner_os", ""), facts.get("image", "")) if x)
host = f"{facts.get('cpu', '')}, {facts.get('os', '')}, {facts.get('cores', '')} cores (affinity {facts.get('affinity', '')}), load {facts.get('load', '')} at the start"
job = f"each figure the median of {facts.get('passes')} passes after a warm-up, {facts.get('seconds')} s for each workload of each engine, each in a process of its own"
data = {
    "meta": {
        "title": "corvus-regex-ecma262: the Industry benchmarks",
        "lede": "This crate over UTF-8 and UTF-16 beside regex, regress and V8 over the Industry workloads of dotnet/performance.",
        "repo": "corvus-dotnet/corvus-regex-ecma262-rs",
        "commit": facts.get("commit", "")[:7],
        "date": run.get("end") or facts.get("date", ""),
        "start": facts.get("date", ""),
        "runner": runner,
        "host": host,
        "runtime": facts.get("rustc", ""),
        "job": job,
        "runUrl": args.run_url,
        "runs": int(facts.get("passes") or 0),
        "versions": {name: version for _, name, _, version, _ in ENGINES},
        "facts": [["Host", host]] + ([["Runner", runner]] if runner else []) + [["Runtime", facts.get("rustc", "") + "; " + v8["engine"]], ["Harness", job]],
        "method": "<p>Each figure is one count of every match a global search finds over the whole text, after an empty match going on one character later. "
                  "This crate and regress run with the workload's flags and u, regex with the pattern as it is and the flags i, m and s, V8 with gu added.</p>",
    },
    "vocab": {
        "impl": "engine", "impls": "engines", "Impls": "Engines",
        "schema": "workload", "schemas": "workloads", "Schema": "Workload", "Schemas": "Workloads",
        "failed": "did not run", "centre": "Median", "spread": "fastest to slowest pass",
        "barLegend": "median of the passes", "whiskerLegend": "whisker: the fastest and the slowest pass",
    },
    "measures": {"time": {"label": "Time", "long": "time per count", "parts": ["time"]}},
    "impls": [{"id": name, "name": name, "group": group, "version": version, "compliance": c} for _, name, group, version, c in ENGINES],
    "schemas": [{"name": w, "note": "", "meta": [["text", TEXTS.get(w.split(":")[0], w.split(":")[0])]]} for w in ids],
    "results": results,
}
os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
with open(args.out, "w", encoding="utf-8") as fo:
    json.dump(data, fo, indent=1, ensure_ascii=False)
print(f"wrote {args.out}: {len(ids)} workloads, {len(ENGINES)} engines")
