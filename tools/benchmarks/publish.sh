#!/usr/bin/env bash
# Publishes a run's data file to this repository's gh-pages branch: benchmarks/latest.json (replaced) and
# benchmarks/runs/<name>.json (added; every day's file is kept), with benchmarks/runs/index.html listing the runs.
# The rest of the branch is left as it is. Run it from a checkout whose 'origin' can push.
#
#   tools/benchmarks/publish.sh <data file> <run name, such as 2026-10-20-abc1234>
set -euo pipefail
file="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
name="${2:?run name}"
branch=gh-pages
work="$(mktemp -d)"
trap 'git worktree remove --force "$work" >/dev/null 2>&1 || rm -rf "$work"; git worktree prune' EXIT
for attempt in 1 2 3 4 5; do
  if git fetch --quiet origin "+refs/heads/$branch:refs/remotes/origin/$branch" 2>/dev/null; then
    git worktree add --quiet --detach "$work" "origin/$branch"
  else
    git worktree add --quiet --detach "$work" HEAD
    git -C "$work" checkout --quiet --orphan "$branch"
    git -C "$work" rm -rfq . >/dev/null 2>&1 || true
  fi
  mkdir -p "$work/benchmarks/runs"
  cp "$file" "$work/benchmarks/latest.json"
  cp "$file" "$work/benchmarks/runs/$name.json"
  {
    echo '<!doctype html><html lang="en"><head><meta charset="utf-8"><title>corvus-regex-ecma262: benchmark runs</title></head><body>'
    echo '<h1>Benchmark runs</h1><p>The data file of each daily run of the Industry benchmarks. The latest is <a href="../latest.json">latest.json</a>; the page is at <a href="https://corvus-oss.org/regex-benchmarks/">corvus-oss.org/regex-benchmarks</a>.</p><ul>'
    for f in $(ls "$work/benchmarks/runs" | grep '\.json$' | sort -r); do echo "<li><a href=\"$f\">$f</a></li>"; done
    echo '</ul></body></html>'
  } > "$work/benchmarks/runs/index.html"
  touch "$work/.nojekyll"
  git -C "$work" add --all
  git -C "$work" -c user.name='github-actions[bot]' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
    commit --quiet -m "Publish the benchmark run $name" || { echo "nothing to publish"; exit 0; }
  if git -C "$work" push --quiet origin "HEAD:refs/heads/$branch"; then
    echo "published benchmarks/latest.json and benchmarks/runs/$name.json"
    exit 0
  fi
  echo "the push was refused (attempt $attempt); starting again from the branch's tip"
  git worktree remove --force "$work"; git worktree prune; work="$(mktemp -d)"
done
exit 1
