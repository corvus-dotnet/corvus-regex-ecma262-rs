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


def failure(r):
    """A figure the harness gave no time for, as the data file carries it: stopped at the harness's limit
    ({"limit": s}), a count of matches that differs from V8's ({"wrongCount": {"counted": n, "v8": m}}), or another
    failure with what the harness said ({"error": "..."})."""
    if "limit" in r:
        return {"limit": r["limit"]}
    m = re.match(r"counted (\d+) where V8 counts (\d+)", str(r.get("error", "")))
    if m:
        return {"wrongCount": {"counted": int(m.group(1)), "v8": int(m.group(2))}}
    return {"error": r["error"]} if "error" in r else None

ap = argparse.ArgumentParser()
ap.add_argument("--run", required=True)
ap.add_argument("--v8", help="the V8 harness's own results, for a run whose harness did not run V8 itself")
ap.add_argument("--out", required=True)
ap.add_argument("--compliance-corvus", required=True)
ap.add_argument("--compliance-regress", required=True)
ap.add_argument("--compliance-v8", help="V8's data-set run; absent for a run without V8 (V8=0)")
ap.add_argument("--run-url", default="")
ap.add_argument("--workloads", help="the workloads.json of the run (tools/benchmarks/workloads.json when not given)")
args = ap.parse_args()

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
run = json.load(open(args.run, encoding="utf-8"))
facts = run["facts"]
# V8 runs inside the interleaved run (results["v8"], its engine in the facts); a separate V8 run is still read. A run
# without V8 (V8=0, whose counts were checked against the workloads' v8Count) has no V8 row.
if "v8" in run["results"]:
    v8 = {"engine": facts.get("v8", "V8"), "results": run["results"]["v8"]}
elif args.v8:
    v8 = json.load(open(args.v8, encoding="utf-8"))
else:
    v8 = None
workloads = json.load(open(args.workloads or os.path.join(root, "tools", "benchmarks", "workloads.json"), encoding="utf-8"))
ids = [w["id"] for w in workloads]


def compliance(path, note=None):
    """The counts of a data-set runner's last line of counts. Every answer the engine never gave (a limit, a crash, a
    hang or a worker that gave up) is in `limited`, as the judging rule of the test data counts it: the runner's
    limit and errors."""
    text = open(path, encoding="utf-8").read().splitlines()
    lines = [l for l in text if l.startswith("records ")]
    if not lines:
        raise SystemExit(f"{path} has no line of counts")
    c = {k: int(v) for k, v in re.findall(r"(right|wrong|refused-valid|accepted-invalid|limit|errors) (\d+)", lines[-1])}
    out = {"right": c["right"], "wrong": c["wrong"], "refused": c["refused-valid"], "acceptedInvalid": c["accepted-invalid"],
           "limited": c.get("limit", 0) + c.get("errors", 0)}
    # Counts checked in beside the harness name the commit of the test data they were made at on their first line
    # ("data <commit>"): the page judges them against what that commit asks.
    if text and text[0].startswith("data ") and len(text[0].split()) == 2:
        out["dataCommit"] = text[0].split()[1]
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
        results[name][w] = failure(r) if "error" in r else {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}}
if v8:
    v8_name = "V8"
    ENGINES.append(("v8", v8_name, "V8", v8["engine"], compliance(args.compliance_v8)))
    results[v8_name] = {}
    for w in ids:
        r = v8["results"].get(w)
        results[v8_name][w] = {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}} if r and "error" not in r else (failure(r) if r else None)
# Where regex counts otherwise than V8 (the oracle), the row says so; an ECMA-262 engine that does is an error above.
differs = {w: r["differs"] for w, r in run["results"].get("regex", {}).items() if isinstance(r, dict) and "differs" in r}

TEXTS = {"sherlock": "Sherlock Holmes (0.6 M chars)", "leipzig": "Leipzig corpus (16 M chars)", "mariomka": "mariomka (0.05 M chars)",
         "rebar-ru": "OpenSubtitles ru (1.2 M chars)", "rebar-zh": "OpenSubtitles zh (0.3 M chars)"}
runner = " ".join(x for x in (facts.get("runner", ""), facts.get("runner_os", ""), facts.get("image", "")) if x)
host = f"{facts.get('cpu', '')}, {facts.get('os', '')}, {facts.get('cores', '')} cores (affinity {facts.get('affinity', '')}), load {facts.get('load', '')} at the start"
if facts.get("protocol") == "interleaved":
    job = (f"interleaved: every workload in turn, each in {facts.get('rounds')} rounds of the engines in turn (their order "
           f"turned each round), each figure a process of its own warmed and timed for {facts.get('seconds')} s "
           f"({facts.get('passes')} passes); a workload's figure is the median of its rounds, the spread its fastest "
           f"and slowest round")
    over = facts.get("spread_over", "")
    spread = (f"over {float(facts.get('spread_limit', 0)) * 100:.0f}% (slowest - fastest) / median: {over}" if over
              else f"every figure within {float(facts.get('spread_limit', 0)) * 100:.0f}% (slowest - fastest) / median")
else:
    job = f"each figure the median of {facts.get('passes')} passes after a warm-up, {facts.get('seconds')} s for each workload of each engine, each in a process of its own"
    spread = ""

# The encodings each engine's runner answers in, which the central benchmark page's compliance figure is a share of:
# every answer the test data asks in those encodings, so an answer the runner did not ask counts against the engine.
ENCODINGS = {"v8": ["utf16"]}
for key, _, _, _, counts in ENGINES:
    if isinstance(counts, dict):
        counts["encodings"] = ENCODINGS.get(key, ["utf16", "utf8"])

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
        "facts": [["Host", host]] + ([["Runner", runner]] if runner else []) + [["Runtime", facts.get("rustc", "") + ("; " + v8["engine"] if v8 else "")], ["Harness", job]]
                 + ([["Spread", spread]] if spread else [])
                 + ([["Counts", "regex counts otherwise than V8 on " + str(len(differs)) + " workloads (each says so)"]] if differs else []),
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
    "schemas": [{"name": w, "note": ("counts differ: " + differs[w]) if w in differs else "",
                 "meta": [["text", TEXTS.get(w.split(":")[0], w.split(":")[0])]]} for w in ids],
    "results": results,
}
os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
with open(args.out, "w", encoding="utf-8") as fo:
    json.dump(data, fo, indent=1, ensure_ascii=False)
print(f"wrote {args.out}: {len(ids)} workloads, {len(ENGINES)} engines")
