# The benchmark harness as an image: this crate built in release, the other Rust engines (regex, regress) and Node
# 24 for the V8 rows, with tools/benchmarks/run.sh as the entry point. The whole invocation is
#
#   docker run --rm -v <workloads.json>:/workloads.json:ro -v <texts dir>:/texts:ro -v <out dir>:/out <image>
#
# which writes /out/industry.json, the run's data file in the results page's shape. The ECMA-262 test data is cloned
# at depth 1 inside the container unless its data/ directory is mounted at /data. JOB=short, SECONDS_EACH, PASSES,
# CORES, SETTLE and RUN_URL pass through (see tools/benchmarks/run.sh). The central benchmark site runs this image each
# night; .github/workflows/harness-image.yml builds, smoke-runs and publishes it.
FROM node:24.22.1-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS node

FROM rust:1.99-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b
RUN apt-get update && apt-get install -y --no-install-recommends python3 git curl ca-certificates util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY --from=node /usr/local/bin/node /usr/local/bin/node
WORKDIR /harness
COPY . .
# rustup installs the toolchain rust-toolchain.toml names; the harness and the data-set runner are built in release.
RUN rustup show && cargo build --release -j 4 --locked --bin industry --bin dataset \
    && rm -rf target/release/build target/release/deps/*.rlib target/release/incremental
# The commit the image is built from, for the run's record (the image has no .git).
ARG COMMIT=""
ENV SKIP_BUILD=1 CORVUS_COMMIT=$COMMIT
ENTRYPOINT ["/harness/tools/benchmarks/container-entry.sh"]
