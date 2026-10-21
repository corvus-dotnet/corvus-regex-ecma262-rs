#!/usr/bin/env bash
# The C image's entry point: tools/c/run.sh over the mounted workloads, texts and (when mounted) test data.
set -euo pipefail
data=()
if [ -d /data ]; then data=(/data); fi
mkdir -p /out
exec /harness/tools/c/run.sh /workloads.json /texts /out/industry.json "${data[@]}"
