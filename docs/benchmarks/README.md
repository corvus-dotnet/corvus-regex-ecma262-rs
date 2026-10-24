# Benchmarks

The Industry workloads of dotnet/performance (`Perf.Regex.Industry`: the Sherlock Holmes, Leipzig and mariomka texts
and the rebar names), as the .NET reference engine's benchmarks run them, counted by five engines: this crate over
UTF-8 and over UTF-16, `regex` (Rust's standard engine, with RE2 semantics), `regress` (an ECMA-262 engine in Rust),
and V8 (Node 24's `RegExp`, the oracle of the ECMA-262 test data).

Each figure is the time of one count of every match a global search finds over the whole text, after an empty match
going on one character later, as `matchAll` does. This crate and regress run with the workload's flags and `u`; regex
is given the pattern as it is with the flags i, m and s (its row is absent where the pattern does not compile); V8 runs
with `gu` added.

## The protocol

The harness (`tools/src/bin/industry.rs`, driven by `tools/benchmarks/run.sh`) interleaves the engines, so that what
changes on the host during a run (its clock, its heat, its other work) falls on every engine alike:

- **Workloads outside, rounds next, engines inside.** The run goes over every workload in turn; each workload has 3
  rounds, and each round runs every engine in turn, the order of the engines turned by one each round. A workload's
  figures for two engines are therefore seconds apart, and so are its rounds: on an unpinned host, whose speed drifts
  over minutes, rounds minutes apart spread a figure by more than the engines differ.
- **One process per figure.** Each turn is a process of its own (`industry --one`, or `node industry.mjs --one` for
  V8), which warms for two fifths of 0.7 seconds and times 7 passes of as many counts as fit the rest, and reports
  their median. The texts are unzipped once by the run and read by every process.
- **The figure and its spread.** A workload's figure for an engine is the median of its 3 rounds; the fastest and the
  slowest round are kept as its spread. Where (slowest - fastest) / median is over 5%, the run says so on its output
  (`SPREAD` lines) and lists the figure in the data's facts (`spread_over`, and the page's Spread fact): such a figure
  is not a clean one.
- **A limit on each figure.** A figure that has not finished in 30 s is stopped, and is recorded as giving no result
  within 30 s. It is not run again in the workload's later rounds, so an engine that cannot count a workload in time
  costs the run 30 s rather than a limit in every round. The page shows such a figure as "> 30 s": the engine did not
  produce a result within 30 s. (The limit was two minutes in every round until 2026-10-23, when R's PCRE2 row spent
  78 minutes of a nightly run on 13 workloads that never gave a figure.)
- **The counts.** Every engine's count is compared with V8's, the oracle of the test data. An ECMA-262 engine that
  counts otherwise is an error on that workload; regex, which does not claim ECMA-262, keeps its figure and the
  workload carries "counts differ" with both counts.
- **V8 once.** Run with `V8=0` (the central benchmark run gives it to every image but the one that times V8), a harness
  times no V8 and runs no V8 over the test data: it compares each count with the workload's `v8Count`, V8's count,
  which the central run writes into the workloads once before any image runs, and its data file has no V8 row.

`ROUNDS`, `SECONDS_EACH`, `PASSES` and `SPREAD` change the protocol's numbers; `JOB=short` is 3 rounds of 0.1 s for a
look. The numbers were chosen on the reference machine (an unpinned i7-13800H laptop under WSL, cores 0-11): there the
earlier protocol, 7 rounds of 0.3 s and 3 passes with the rounds outermost, gave a median spread of 12.5% a figure; 3
rounds of 0.7 s and 7 passes, in about the same time, gave 6.7%; and with each workload's rounds back to back, as now,
the full run gives 6.0% (Rust) and 7.0% (C) in 497 s and 623 s. The same figure timed again and again back to back
moves by about 2%, so what is left is the host's own drift between processes, which neither longer turns (3 rounds
of 1.4 s gave 8.6% in twice the time) nor more passes remove. On that host most figures are still listed as over 5%;
a pinned host is needed for clean figures.

For the central results contract, every port's image follows the same protocol:

> A run measures every engine on every workload in rounds: workloads are the outer loop, each workload's rounds the
> next and engines the innermost, with the engine order turned by one each round, and each figure timed in a process
> of its own after a warm-up. A workload's figure is the median over the rounds, and its spread is the fastest and the slowest round.
> A figure that does not finish in 30 s is stopped, recorded as `{"error": "did not produce a result within 30 s",
> "limit": 30}` (in the data file, `{"limit": 30}`), and not run in the workload's later rounds.
> A figure whose spread, (slowest - fastest) / median, is over the run's limit (5% unless the run says otherwise) is
> listed in the run's facts and is never presented as a clean figure. Every engine's match count is compared with
> V8's: an ECMA-262 engine whose count differs is an error on that workload, and any other engine's differing count
> is shown on the workload with both counts.

Beside its figures each engine carries its compliance with the
[ECMA-262 test data](https://github.com/corvus-dotnet/ecma262-regex-test-data), judged by the data's own rule: the
counts of right and wrong answers, of valid patterns refused, of invalid ones accepted, and of answers never given
(`limited`: an answer that passed the 2 seconds every runner gives one, or that the engine crashed or hung in), or
"not ECMA-262" for regex, which does not claim it. regress is judged in both encodings, with the global search, the
sticky search, replace and split written over its exec (it has none of them). It has no limit on a match, so it runs
in child processes: a watchdog thread in each times every answer by the CPU time of the thread that runs regress, from
just before regress is called, and ends the process at one that passes 2 seconds, and a new child goes on from the next
answer. Once three answers of a record have reached the limit, its other answers are unanswered without being asked.

**The results** are on one page, the central benchmark site,
[corvus-oss.org/corvus-regex-ecma262-benchmarks](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), which runs
this repository's harness image each night on its runner beside the engines of the other languages. This repository
publishes no figures of its own.

**To run them**, with Rust and Node 24 on the path:

```sh
tools/benchmarks/run.sh tools/benchmarks/workloads.json <texts directory> out.json [<ecma262-regex-test-data>/data]
```

The script builds the engines, works out the compliance figures (cloning the test data when no directory is given),
runs every workload and writes the data file in the shape of the results page
(`tools/benchmarks/results-page/generate_site.py --data out.json --out page.html` renders it, to look at). For a
measured run on a quiet machine, `CORES=0-11 SETTLE=180` pins the timed parts to those cores after a settle of three
minutes.

**As an image**, the same harness with everything built in (`Dockerfile`), published as
`ghcr.io/corvus-dotnet/corvus-regex-ecma262-rs:latest` from main and with the release's tag from each release
(`.github/workflows/harness-image.yml`, which smoke-runs it on two workloads first):

```sh
docker run --rm -v <workloads.json>:/workloads.json:ro -v <texts dir>:/texts:ro -v <out dir>:/out ghcr.io/corvus-dotnet/corvus-regex-ecma262-rs:latest
```

writes `<out dir>/industry.json`; the test data is cloned inside unless its `data/` directory is mounted at `/data`,
and `-e JOB=short` makes a quick look of it.

## From C

The C library has a harness of its own, in C (`tools/c/industry.c`, driven by `tools/c/run.sh`), under the same
protocol and writing the same data file, with the engines a C program would choose between: the C library over UTF-8
and over UTF-16 (through its header, a global search made once per text and reset for each count, its matches taken
256 at a time with `cre_matches_next_batch`, as a C program that wants many of them takes them), QuickJS's
`libregexp` (quickjs-ng 0.17.0, an ECMA-262 engine in C, over the text as QuickJS holds a string: 8-bit when every
character is Latin-1, UTF-16 otherwise), PCRE2 10.49 with its JIT (not ECMA-262: the pattern as it is in UTF mode with
the flags i, m and s, its row marked where its count differs from V8's) and V8. The C library and libregexp run with
the workload's flags and `u`.

Their compliance comes from the C data-set runner (`tools/c/dataset.c`), a C program that judges the C library through
its header exactly as `tools/src/bin/dataset.rs` judges the crate, with the same counts, and judges `libregexp` the way
the Rust runner judges regress: with the global search, replace and split written over its exec, each match given 2
seconds and 256 MB. libregexp has no UTF-8 interface, so it is judged over UTF-16 alone. QuickJS's compliance is worked
out once and kept in `tools/c/compliance-quickjs.json`, since many patterns take libregexp to its limit and the run
takes some tens of minutes (`QUICKJS_COMPLIANCE=1` works it out again).

Every runner writes its engine's compliance file (`--compliance <file>`), which the results scripts give the central
page as it is: the counts, `judge` (the runner and the calls it makes in each encoding) and `excludes` (what it leaves
out, of the kinds the page allows: for libregexp, UTF-8 and the operations written over its exec; for regress, those
operations). Both checked-in files,
`tools/c/compliance-quickjs.json` and `tools/benchmarks/compliance-regress.json`, also give the commit of the test data
they were made at (`dataCommit`): the page judges the counts against what that commit asks, and says so when it is not
the commit the night's run judged the other engines at. CI fails when a file's commit is not the one CI checks the data
out at, so moving CI's pin means working both out again (a run with the data cloned writes the commit itself), and
releasing them as a harness release.

The C image, `ghcr.io/corvus-dotnet/corvus-regex-ecma262-c` (`tools/c/Dockerfile`, published by
`.github/workflows/c-harness-image.yml` as the Rust image is), has the same entry point contract, and the central site
runs it as a source of its own:

```sh
docker run --rm -v <workloads.json>:/workloads.json:ro -v <texts dir>:/texts:ro -v <out dir>:/out ghcr.io/corvus-dotnet/corvus-regex-ecma262-c:latest
```
