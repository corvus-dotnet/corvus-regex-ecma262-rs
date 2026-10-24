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
# since many patterns take libregexp to its limit) rather than take tools/c/compliance-quickjs.json.
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
# Each runner writes its engine's compliance file (--compliance): the counts, the runner and the calls it makes, and
# what it leaves out, which the results script gives the page as it is.
"$bin/dataset" "$data" "$work/wrong-corvus.tsv" --compliance "$work/compliance-corvus.json" > "$work/dataset-corvus.txt" 2>&1 || true
# V8=0 (the central run, for every image but one): no V8, whose counts the workloads carry (v8Count).
v8=(--v8 tools/benchmarks/v8/industry.mjs)
compliance_v8=(--compliance-v8 "$work/compliance-v8.json")
if [ "${V8:-1}" = 0 ]; then
  v8=()
  compliance_v8=()
else
  node tools/benchmarks/v8/dataset.mjs "$data" --compliance "$work/compliance-v8.json" > "$work/dataset-v8.txt" 2>&1 || true
fi
if [ "${QUICKJS_COMPLIANCE:-0}" = 1 ]; then
  "$bin/dataset-quickjs" "$data" "$work/wrong-quickjs.tsv" --compliance "$work/compliance-quickjs.json" > "$work/dataset-quickjs.txt" 2>&1 || true
  # The commit of the data, when it is a checkout, goes in the file as dataCommit, as the checked-in file's does.
  if commit=$(git -C "$data/.." rev-parse HEAD 2>/dev/null) && [ -f "$work/compliance-quickjs.json" ]; then
    python3 -c 'import json, sys; c = json.load(open(sys.argv[1])); c["dataCommit"] = sys.argv[2]; json.dump(c, open(sys.argv[1], "w"))' \
      "$work/compliance-quickjs.json" "$commit"
  fi
else
  cp tools/c/compliance-quickjs.json "$work/compliance-quickjs.json" 2>/dev/null || true
fi
# Each runner's line of counts. A runner that failed, or a checked-in compliance file that is missing, writes none:
# the timing still runs and the results script writes that engine without a compliance, so the night keeps every
# figure, and this says in its log why the engine has no compliance.
for name in corvus quickjs $([ -f "$work/compliance-v8.json" ] && echo v8); do
  f="$work/compliance-$name.json"
  if [ -s "$f" ]; then
    echo "$name: $(tail -n 1 "$work/dataset-$name.txt" 2>/dev/null || echo 'checked in')"
  else
    echo "WARNING: compliance-$name.json is missing, so $name has no compliance in this run's data file"
    if [ -f "$work/dataset-$name.txt" ]; then tail -n 20 "$work/dataset-$name.txt" | sed 's/^/  /'; fi
  fi
done
if [ "${SETTLE:-0}" -gt 0 ]; then
  echo "settling for $SETTLE s"
  sleep "$SETTLE"
fi
export CORVUS_REGEX_TEXTS="$texts"
timed "$bin/industry" "$workloads" "${v8[@]}" --rounds "$rounds" --seconds "$seconds" \
  --passes "$passes" --spread "$spread" --out "$work/run.json" 2> "$work/run.log"
grep -E '^(SPREAD|COUNT) ' "$work/run.log" || true
python3 tools/c/industry-results.py --workloads "$workloads" --run "$work/run.json" --out "$out_file" \
  --compliance-corvus "$work/compliance-corvus.json" --compliance-quickjs "$work/compliance-quickjs.json" \
  "${compliance_v8[@]}" --run-url "${RUN_URL:-}"
if [ -n "${KEEP:-}" ]; then mkdir -p "$KEEP" && cp "$work"/*.json "$work"/*.txt "$work"/*.log "$KEEP"/; fi
