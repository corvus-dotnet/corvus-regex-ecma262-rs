#!/usr/bin/env python3
"""Writes the Industry benchmark record from a run: docs/benchmarks/industry.json (the data of the results page, see
tools/benchmarks/results-page/generate_site.py) and docs/benchmarks/industry.md (the table).

Usage: industry-results.py --run <harness results json> --v8 <V8 results json>
                           --compliance-corvus <data-set runner output> [--compliance-regress <its output>]
                           [--host pinned|ci] [--out-dir <dir>] [--run-url <url>]

The run is the output of the harness (`cargo run --release --bin industry -- tools/benchmarks/workloads.json --out
run.json`), whose `facts` are the run's own record of itself: the processor, the cores it was given, the system,
the load when it started, the compiler, the commit, the date, the seconds and passes of each figure. V8's results are
those of tools/benchmarks/v8/industry.mjs over the same workloads, which names its engine. Nothing on the page is
typed in: what is not in a record is not said.

The compliance of each engine comes from the data-set runner over the ECMA-262 test data
(`cargo run --release --bin dataset -- <data> <wrong.tsv> [--engine regress]`): its last line has the counts. The
`regex` crate does not claim ECMA-262 and is marked so; V8 is the oracle the data was written from.

The host is "pinned" for the procedure of tools/benchmarks/run.sh (the machine quiet, the run on the cores its
affinity names, after a settle) or "ci" for the daily run on a GitHub-hosted runner, where nothing is pinned; the two
are kept apart and never read against each other.
"""
import argparse, json, math, os, re, statistics, string

ap = argparse.ArgumentParser()
ap.add_argument("--run", required=True)
ap.add_argument("--v8")
ap.add_argument("--compliance-corvus", required=True)
ap.add_argument("--compliance-regress")
ap.add_argument("--host", choices=["pinned", "ci"], default="pinned")
ap.add_argument("--out-dir")
ap.add_argument("--run-url", default="")
args = ap.parse_args()

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
out_dir = args.out_dir or os.path.join(root, "docs", "benchmarks")
os.makedirs(out_dir, exist_ok=True)
run = json.load(open(args.run, encoding="utf-8"))
facts = run["facts"]
commit = facts["commit"][:7]
date = facts["date"]
workloads = json.load(open(os.path.join(root, "tools", "benchmarks", "workloads.json"), encoding="utf-8"))
ids = [w["id"] for w in workloads]


def compliance(path):
    """The counts of the data-set runner's last line."""
    if not path:
        return None
    lines = [l for l in open(path, encoding="utf-8").read().splitlines() if l.startswith("records ")]
    if not lines:
        raise SystemExit(f"{path} has no line of counts")
    line = lines[-1]
    counts = {k: int(v) for k, v in re.findall(r"(right|wrong|refused-valid|accepted-invalid|limit|errors) (\d+)", line)}
    counts["line"] = line
    return counts


corvus_counts = compliance(args.compliance_corvus)
regress_counts = compliance(args.compliance_regress)


def counts_text(c, scope):
    return (f"{c['right']:,} right, {c['wrong']:,} wrong, {c['refused-valid']:,} valid patterns refused, "
            f"{c['accepted-invalid']:,} invalid patterns accepted, {c['limit']:,} at a limit ({scope})")


# The engines: the harness's names, the names the page shows, their group and their compliance.
ENGINES = [
    ("corvus-utf8", "corvus-regex-ecma262 UTF-8", "Corvus",
     counts_text(corvus_counts, "both encodings, every file") if corvus_counts else ""),
    ("corvus-utf16", "corvus-regex-ecma262 UTF-16", "Corvus",
     counts_text(corvus_counts, "both encodings, every file") if corvus_counts else ""),
    ("regex", "regex", "Rust", "not ECMA-262"),
    ("regress", "regress", "Rust",
     counts_text(regress_counts, "test answers over UTF-16 and UTF-8, every file but captures.jsonl") if regress_counts else ""),
]
results = {}
counts = {}
errors = {}
for key, name, _, _ in ENGINES:
    rows = run["results"].get(key, {})
    results[name] = {}
    for w in ids:
        r = rows.get(w)
        if r is None:
            continue
        if "error" in r:
            results[name][w] = None
            errors[(name, w)] = r["error"]
            continue
        results[name][w] = {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}}
        counts[(name, w)] = r["count"]

V8 = None
if args.v8:
    v8 = json.load(open(args.v8, encoding="utf-8"))
    node = v8["engine"].split("Node ")[-1].rstrip(")").split(".")[0]
    V8 = ("V8 (Node " + node + ")", v8["engine"])
    ENGINES.append(("v8", V8[0], "V8", "the oracle the test data was written from"))
    results[V8[0]] = {}
    for w in ids:
        r = v8["results"].get(w)
        results[V8[0]][w] = {"time": {"median": r["median"], "min": r["min"], "max": r["max"]}} if r else None
        if r:
            counts[(V8[0], w)] = r["count"]

C8 = "corvus-regex-ecma262 UTF-8"


def ratios(a, b):
    out = []
    for w in ids:
        x, y = results[a].get(w), results.get(b, {}).get(w)
        if x and y:
            out.append(x["time"]["median"] / y["time"]["median"])
    return out


def gm(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs))


TEXTS = {"sherlock": "Sherlock Holmes (0.6 M chars)", "leipzig": "Leipzig corpus (16 M chars)", "mariomka": "mariomka (0.05 M chars)",
         "rebar-ru": "OpenSubtitles ru (1.2 M chars)", "rebar-zh": "OpenSubtitles zh (0.3 M chars)"}
cpu, cores, affinity, os_name, load, rustc = (facts.get(k, "") for k in ("cpu", "cores", "affinity", "os", "load", "rustc"))
job = f"each figure the median of {facts.get('passes')} passes after a warm-up, {facts.get('seconds')} s for each workload of each engine, in a process of its own"
if args.host == "pinned":
    host_line = f"{cpu}, {os_name}, the run on cores {affinity} of {cores} after a settle, the load {load} when it started"
    title_suffix, lede_tail = "", ""
else:
    host_line = f"a GitHub-hosted runner ({cpu}, {os_name}, {cores} cores, the load {load} when it started), every engine in one job, nothing pinned"
    title_suffix, lede_tail = " (daily CI run)", " This is the run of the day on a GitHub-hosted runner; the committed record is the reference."
FACTS = [["Host", host_line], ["Runtime", rustc + (f"; {V8[1]}" if V8 else "")], ["Harness", job], ["Commit", commit]]

compliance_html = "".join(f"<li><b>{name}</b>: {html}</li>" for _, name, _, html in ENGINES if html)
method_html = (
    "<p><b>Each figure is one count</b> over the whole text: every match a global search finds, after an empty match the "
    "search going on one character later, as <code>matchAll</code> goes on. This crate and <code>regress</code> run with the "
    "workload's flags and <code>u</code>, as the Corvus rows of the reference engine's run carry; <code>regex</code> is given "
    "the pattern as it is with the flags i, m and s, and its row is absent where the pattern does not compile. regex reads "
    "<code>\\w</code>, <code>.</code> and <code>$</code> by its own rules, so its counts differ on some rows (the table of the "
    "record lists them).</p>"
    f"<p><b>The figures</b> are {job}: the bar is the median and the whisker the fastest and the slowest pass. A ratio below "
    "1.0 is this crate being faster.</p>"
    "<p><b>Compliance</b> over the ECMA-262 test data (github.com/corvus-dotnet/ecma262-regex-test-data), by the data-set "
    f"runner of this repository:</p><ul>{compliance_html}</ul>"
    f"<p><b>The host</b>: {host_line}. {rustc}. Commit {commit}, {date}.</p>"
)

data = {
    "meta": {
        "title": "corvus-regex-ecma262: the Industry benchmarks" + title_suffix,
        "lede": "This crate over UTF-8 and UTF-16 beside regex, regress and V8 over the Industry workloads of dotnet/performance. Pick the engines to compare; every view below follows." + lede_tail,
        "repo": "corvus-dotnet/corvus-regex-ecma262-rs",
        "commit": commit,
        "date": date,
        "runUrl": args.run_url,
        "runs": int(facts.get("passes") or 0),
        "facts": FACTS,
        "method": method_html,
    },
    "vocab": {
        "impl": "engine", "impls": "engines", "Impls": "Engines",
        "schema": "workload", "schemas": "workloads", "Schema": "Workload", "Schemas": "Workloads",
        "failed": "did not run", "centre": "Median", "spread": "fastest to slowest pass",
        "barLegend": "median of the passes", "whiskerLegend": "whisker: the fastest and the slowest pass",
    },
    "measures": {"time": {"label": "Time", "long": "time per count", "parts": ["time"]}},
    "impls": [{"id": name, "name": name, "group": group, "version": V8[1] if V8 and name == V8[0] else (rustc if group != "V8" else ""),
               "compliance": html} for _, name, group, html in ENGINES],
    "schemas": [{"name": w, "note": "", "meta": [["text", TEXTS.get(w.split(":")[0], w.split(":")[0])]]} for w in ids],
    "results": results,
}
with open(os.path.join(out_dir, "industry.json"), "w", encoding="utf-8") as fo:
    json.dump(data, fo, indent=1, ensure_ascii=False)


def us(ns):
    x = ns / 1000
    return f"{x:,.1f}" if x >= 10 else f"{x:.2f}" if x >= 1 else f"{x:.3f}"


def cell(name, w):
    x = results.get(name, {}).get(w)
    return x["time"]["median"] if x else None


lines = []
apart = []
for w in ids:
    ww = "<code>" + "".join("\\" + ch if ch in string.punctuation else ch for ch in w) + "</code>"
    row = [ww]
    c8 = cell(C8, w)
    for _, name, _, _ in ENGINES:
        v = cell(name, w)
        row.append(us(v) if v else ("absent" if (name, w) in errors else ""))
    for other in ["regex", "regress"] + ([V8[0]] if V8 else []):
        v = cell(other, w)
        row.append(f"{c8 / v:.2f}" if c8 and v else "")
    lines.append("| " + " | ".join(row) + " |")
    mine = counts.get((C8, w))
    for _, name, _, _ in ENGINES:
        theirs = counts.get((name, w))
        if theirs is not None and mine is not None and theirs != mine:
            apart.append(f"- {ww}: {name} counts {theirs:,} to this crate's {mine:,}.")
absent = [f"- {name} on <code>{w}</code>: {e}" for (name, w), e in sorted(errors.items())]

summary = []
for other in ["regex", "regress"] + ([V8[0]] if V8 else []):
    r = ratios(C8, other)
    if r:
        summary.append(f"- UTF-8 against {other}: median {statistics.median(r):.2f}, geometric mean {gm(r):.2f}, at or below 1.0 on {sum(1 for x in r if x <= 1.0)} of {len(r)}.")
r16 = ratios("corvus-regex-ecma262 UTF-16", C8)
if r16:
    summary.append(f"- UTF-16 against UTF-8 (this crate): median {statistics.median(r16):.2f}.")

head = " | ".join(["Workload"] + [name for _, name, _, _ in ENGINES] + [f"UTF-8 / {o}" for o in ["regex", "regress"] + ([V8[0]] if V8 else [])])
sep = "|".join(["---"] * (1 + len(ENGINES) + 2 + (1 if V8 else 0)))
host_md = f"- **Host** {host_line}." if args.host == "pinned" else f"- **Host** {host_line}."
md = f"""# Benchmarks: the Industry workloads

- **Commit** {commit}, run on {date}.
{host_md}
- **Runtime and harness** {rustc}{"; " + V8[1] if V8 else ""}; {job}.

The data of this run is `industry.json` beside this file, and the page `index.html` is generated from it by
`tools/benchmarks/results-page/generate_site.py`; `tools/benchmarks/industry-results.py` writes both records from a run.

The workloads are the Industry set of dotnet/performance (`Perf.Regex.Industry`: the Sherlock Holmes, Leipzig and
mariomka texts and the rebar names), as the .NET reference engine's benchmarks run them
(`tools/benchmarks/workloads.json`). Each figure is one count of every match over the whole text, in microseconds. This
crate and regress run with the workload's flags and `u`; regex is given the pattern as it is, with the flags i, m and s.
V8 is Node's `RegExp` over the same texts with `gu` added, its matches counted by a `test` loop that advances
`lastIndex` as `matchAll` does (`tools/benchmarks/v8/industry.mjs`).

## Compliance

Over the ECMA-262 test data, by `cargo run --release --bin dataset`:

""" + "\n".join(f"- **{name}**: {html}." for _, name, _, html in ENGINES if html) + f"""

## Summary

""" + "\n".join(summary) + f"""

## The rows

| {head} |
|{sep}|
""" + "\n".join(lines) + "\n" + (("\n## Counts that differ\n\nregex is not ECMA-262: it reads `\\w`, `.`, `$` and case by its own rules.\n\n" + "\n".join(apart) + "\n") if apart else "") + (("\n## Rows absent\n\n" + "\n".join(absent) + "\n") if absent else "")
with open(os.path.join(out_dir, "industry.md"), "w", encoding="utf-8") as fo:
    fo.write(md)
print("\n".join(summary))
