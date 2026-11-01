# The C benchmark harness as an image: the C library built in release, the C tools over it (the data-set runners and
# the harness, with QuickJS's libregexp and PCRE2 at their pinned releases) and Node 24 for the V8 rows, with
# tools/c/run.sh as the entry point. Built from the repository's root:
#
#   docker build -f tools/c/Dockerfile -t corvus-regex-ecma262-c .
#   docker run --rm -v <workloads.json>:/workloads.json:ro -v <texts dir>:/texts:ro -v <out dir>:/out <image>
#
# which writes /out/industry.json, the run's data file in the results page's shape. The ECMA-262 test data is cloned
# at depth 1 inside the container unless its data/ directory is mounted at /data. JOB=short, SECONDS_EACH, PASSES,
# CORES, SETTLE and RUN_URL pass through (see tools/benchmarks/run.sh). The central benchmark site runs this image each
# night; .github/workflows/c-harness-image.yml builds, smoke-runs and publishes it.
FROM node:24.22.1-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS node

FROM rust:1.99-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b
RUN apt-get update && apt-get install -y --no-install-recommends python3 git curl ca-certificates util-linux cmake \
    && rm -rf /var/lib/apt/lists/*
COPY --from=node /usr/local/bin/node /usr/local/bin/node
WORKDIR /harness
COPY . .
# rustup installs the toolchain rust-toolchain.toml names; the C library is built in release, and the C tools against
# it as a consumer builds them.
RUN rustup show && cargo build --release -j 4 --locked -p corvus-regex-ecma262-capi \
    && cmake -S tools/c -B build/c-harness -DCRE_LIB_DIR=/harness/target/release -DCRE_BENCH=ON -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build/c-harness -j 4 \
    && rm -rf target/release/build target/release/deps target/release/incremental build/c-harness/_deps
# The commit the image is built from, for the run's record (the image has no .git).
ARG COMMIT=""
ENV SKIP_BUILD=1 CORVUS_COMMIT=$COMMIT
ENTRYPOINT ["/harness/tools/c/container-entry.sh"]
