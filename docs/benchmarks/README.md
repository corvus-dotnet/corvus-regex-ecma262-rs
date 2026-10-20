# Benchmarks

The Industry workloads of dotnet/performance (`Perf.Regex.Industry`: the Sherlock Holmes, Leipzig and mariomka texts
and the rebar names), as the .NET reference engine's benchmarks run them, counted by five engines: this crate over
UTF-8 and over UTF-16, `regex` (Rust's standard engine, with RE2 semantics), `regress` (an ECMA-262 engine in Rust),
and V8 (Node 24's `RegExp`, the oracle of the ECMA-262 test data).

Each figure is one count of every match a global search finds over the whole text, after an empty match going on
one character later, as `matchAll` does: the median of five passes after a warm-up, each workload of each engine in a
process of its own. This crate and regress run with the workload's flags and `u`; regex is given the pattern as it is
with the flags i, m and s (its row is absent where the pattern does not compile, and its counts differ where its
`\w`, `.` and `$` do); V8 runs with `gu` added.

Beside its figures each engine carries its compliance with the
[ECMA-262 test data](https://github.com/corvus-dotnet/ecma262-regex-test-data): the counts of right and wrong answers,
of valid patterns refused and of invalid ones accepted, or "not ECMA-262" for regex, which does not claim it.

**The results** are on the central page, [corvus-oss.org/corvus-regex-ecma262-benchmarks](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/),
beside the engines of the other languages. This repository publishes the data of its nightly run, a GitHub-hosted
runner with every engine in one job, as
[benchmarks/latest.json](https://raw.githubusercontent.com/corvus-dotnet/corvus-regex-ecma262-rs/gh-pages/benchmarks/latest.json)
on its gh-pages branch, with every day's file under `benchmarks/runs/`.

**To run them**, with Rust and Node 24 on the path:

```sh
tools/benchmarks/run.sh tools/benchmarks/workloads.json <texts directory> out.json [<ecma262-regex-test-data>/data]
```

The script builds the engines, works out the compliance figures (cloning the test data when no directory is given),
runs every workload and writes the data file in the shape of the results page
(`tools/benchmarks/results-page/generate_site.py --data out.json --out page.html` renders it, to look at). For a
measured run on a quiet machine, `CORES=0-11 SETTLE=180` pins the timed parts to those cores after a settle of three
minutes.

**As an image**, the same harness with everything built in (`Dockerfile`), released as
`ghcr.io/corvus-dotnet/corvus-regex-ecma262-rs:<version>` and `:latest`:

```sh
docker run --rm -v <workloads.json>:/workloads.json:ro -v <texts dir>:/texts:ro -v <out dir>:/out ghcr.io/corvus-dotnet/corvus-regex-ecma262-rs:latest
```

writes `<out dir>/industry.json`; the test data is cloned inside unless its `data/` directory is mounted at `/data`.
The nightly workflow builds and runs this image.
