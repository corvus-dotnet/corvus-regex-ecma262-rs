#!/usr/bin/env bash
# The C benchmark harness, which the central benchmark site runs each night (as the image, tools/c/Dockerfile) and
# anyone can run: it builds the C library and the C tools, works out the compliance of each ECMA-262 engine over the
# test data, runs every engine over the Industry workloads under the protocol of tools/benchmarks/run.sh, and writes
# the run's one data file in the results page's shape.
#
#   tools/c/run.sh <workloads.json> <texts directory> <out.json> [test data directory]
#
# It needs Rust (rustup installs the toolchain rust-toolchain.toml names), a C compiler, CMake and Node 24 on PATH.
# The arguments, and ROUNDS, SECONDS_EACH, PASSES, SPREAD, JOB=short, CORES, SETTLE, SKIP_BUILD, RUN_URL and KEEP, are
# those of tools/benchmarks/run.sh. QUICKJS_COMPLIANCE=1 works out QuickJS's compliance again (some tens of minutes,
# since many patterns take libregexp to its limit) rather than take tools/c/compliance-quickjs.txt.
set -euo pipefail
here="$(cd "$(dirname "$0")/../.." && pwd)"
abs() { mkdir -p "$(dirname "$1")"; echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
workloads="$(abs "${1:?workloads.json}")"
texts="$(mkdir -p "${2:?texts directory}" && cd "$2" && pwd)"
out_file="$(abs "${3:?out.json}")"
data="${4:-}"
# The protocol's numbers: tools/benchmarks/protocol.sh, shared with the Rust harness.
. "$here/tools/benchmarks/protocol.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
timed() { if [ -n "${CORES:-}" ]; then taskset -c "$CORES" "$@"; else "$@"; fi; }
cd "$here"
if [ -z "$data" ]; then
  git clone --quiet --depth 1 https://github.com/corvus-dotnet/ecma262-regex-test-data "$work/test-data"
  data="$work/test-data/data"
fi
bin="${CRE_TOOLS_BIN:-$here/build/c-harness/bin}"
# The image (tools/c/Dockerfile) has them built already.
if [ "${SKIP_BUILD:-0}" != 1 ]; then
  cargo build --release -j 4 -p corvus-regex-ecma262-capi
  cmake -S tools/c -B build/c-harness -DCRE_LIB_DIR="$here/target/release" -DCRE_BENCH=ON -DCMAKE_BUILD_TYPE=Release
  cmake --build build/c-harness -j 4
fi
# COUNT_GATE=1 (the harness-image job): every workload counted by this library's engines beside V8, one round of the
# least timing, each count checked against V8's by count-gate.py; nothing else runs.
if [ "${COUNT_GATE:-0}" = 1 ]; then
  export CORVUS_REGEX_TEXTS="$texts"
  "$bin/industry" "$workloads" --engines corvus-utf8,corvus-utf16,v8 --v8 tools/benchmarks/v8/industry.mjs --rounds 1 --seconds 0.01 \
    --passes 1 --out "$work/counts.json" 2> "$work/counts.log" || true
  python3 tools/benchmarks/count-gate.py "$workloads" "$work/counts.json" corvus-utf8 corvus-utf16
  exit $?
fi
"$bin/dataset" "$data" "$work/wrong-corvus.tsv" > "$work/compliance-corvus.txt" 2>&1 || true
# V8=0 (the central run, for every image but one): no V8, whose counts the workloads carry (v8Count).
v8=(--v8 tools/benchmarks/v8/industry.mjs)
compliance_v8=(--compliance-v8 "$work/compliance-v8.txt")
if [ "${V8:-1}" = 0 ]; then
  v8=()
  compliance_v8=()
else
  node tools/benchmarks/v8/dataset.mjs "$data" > "$work/compliance-v8.txt" 2>&1 || true
fi
if [ "${QUICKJS_COMPLIANCE:-0}" = 1 ]; then
  "$bin/dataset-quickjs" "$data" "$work/wrong-quickjs.tsv" > "$work/compliance-quickjs.txt" 2>&1 || true
else
  cp tools/c/compliance-quickjs.txt "$work/compliance-quickjs.txt"
fi
tail -n 1 "$work/compliance-corvus.txt" $([ -f "$work/compliance-v8.txt" ] && echo "$work/compliance-v8.txt") "$work/compliance-quickjs.txt"
if [ "${SETTLE:-0}" -gt 0 ]; then
  echo "settling for $SETTLE s"
  sleep "$SETTLE"
fi
export CORVUS_REGEX_TEXTS="$texts"
timed "$bin/industry" "$workloads" "${v8[@]}" --rounds "$rounds" --seconds "$seconds" \
  --passes "$passes" --spread "$spread" --out "$work/run.json" 2> "$work/run.log"
grep -E '^(SPREAD|COUNT) ' "$work/run.log" || true
python3 tools/c/industry-results.py --workloads "$workloads" --run "$work/run.json" --out "$out_file" \
  --compliance-corvus "$work/compliance-corvus.txt" --compliance-quickjs "$work/compliance-quickjs.txt" \
  "${compliance_v8[@]}" --run-url "${RUN_URL:-}"
if [ -n "${KEEP:-}" ]; then mkdir -p "$KEEP" && cp "$work"/*.json "$work"/*.txt "$work"/*.log "$KEEP"/; fi
