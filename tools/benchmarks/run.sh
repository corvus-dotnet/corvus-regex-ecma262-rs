#!/usr/bin/env bash
# The measured benchmark run, the record in docs/benchmarks: the compliance of each engine over the ECMA-262 test
# data, then every engine over the Industry workloads, then the record and its page.
#
#   tools/benchmarks/run.sh <output directory> <test data directory> [seconds] [passes]
#
# Before it runs nothing else may run on the machine (no build, no other benchmark), and the load must be below 1.5.
# The engines are built and the compliance runs are made first; then the machine settles for three minutes, and the
# timed runs are made under taskset on the cores CORES names (0-11, the performance cores of the reference machine).
# NODE is the Node 24 the test data was written with (V8 13.6). The workloads are tools/benchmarks/workloads.json,
# written from the .NET reference engine's Industry.cs by tools/benchmarks/v8/workloads.py.
set -euo pipefail
here="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mkdir -p "${1:?output directory}" && cd "$1" && pwd)"
data="${2:?test data directory (ecma262-regex-test-data/data)}"
seconds="${3:-2}"
passes="${4:-5}"
cores="${CORES:-0-11}"
node="${NODE:-node}"
cd "$here"
load="$(cut -d' ' -f1 /proc/loadavg)"
if awk "BEGIN { exit !($load >= 1.5) }"; then
  echo "load is $load; wait for it to fall below 1.5" >&2
  exit 1
fi
cargo build --release -j 4 --bin industry --bin dataset
./target/release/dataset "$data" "$out/wrong-corvus.tsv" > "$out/compliance-corvus.txt" 2>&1 || true
./target/release/dataset "$data" "$out/wrong-regress.tsv" --engine regress > "$out/compliance-regress.txt" 2>&1 || true
tail -1 "$out/compliance-corvus.txt" "$out/compliance-regress.txt"
echo "built; settling for 3 minutes"
sleep 180
echo "load $(cut -d' ' -f1 /proc/loadavg) after the settle"
taskset -c "$cores" ./target/release/industry tools/benchmarks/workloads.json --seconds "$seconds" --passes "$passes" --out "$out/run.json" 2> "$out/run.log"
taskset -c "$cores" "$node" tools/benchmarks/v8/industry.mjs tools/benchmarks/workloads.json "$seconds" "$passes" > "$out/v8.json" 2> "$out/v8.log"
python3 tools/benchmarks/industry-results.py --run "$out/run.json" --v8 "$out/v8.json" --compliance-corvus "$out/compliance-corvus.txt" --compliance-regress "$out/compliance-regress.txt" --host pinned
python3 tools/benchmarks/results-page/generate_site.py --data docs/benchmarks/industry.json --out docs/benchmarks/index.html
cp "$out/run.json" docs/benchmarks/industry-run.json
cp "$out/v8.json" docs/benchmarks/industry-v8.json
echo "done $(date -u +%FT%TZ)"
