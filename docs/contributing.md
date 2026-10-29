# Contributing

This guide is for someone who wants to build the crate and the C library, run their tests and gates, and open a pull
request. It covers the tools, the build step by step, the gates every change must pass, and how a pull request is
released. The design documents of [the crate](design.md) and [the C library](design-c.md) describe what is underneath.

## 1. Tools

| Tool | Needed for | Version |
|---|---|---|
| Rust | Everything | The toolchain `rust-toolchain.toml` pins, which `rustup` installs on first use |
| Git | The repository and the test data | Any recent version |
| A C compiler and CMake | The C library's tests and the C test-data runner | Any C99 compiler; CMake 3.16 or later |
| PowerShell 7 | The Unicode table generators, the C API reference's generator, packaging and the release scripts | 7.4 or later |
| Node.js | The test data for other versions of Unicode and for ECMAScript 2020, and the V8 rows of the harnesses | 24 (the V8 it carries is the oracle) |

A contributor who only builds the crate and runs its tests needs Rust and Git.

- **Windows:** `winget install Rustlang.Rustup Git.Git Kitware.CMake Microsoft.PowerShell OpenJS.NodeJS.LTS`, with
  the Visual Studio Build Tools' C++ workload for the linker and the C compiler.
- **Linux:** `rustup` from [rustup.rs](https://rustup.rs); Git, a C compiler and CMake from the distribution;
  PowerShell from Microsoft's feed or `snap install powershell --classic`; Node.js 24 from nodejs.org or the
  NodeSource feed.
- **macOS:** `brew install rustup git cmake powershell node@24`, then `rustup-init`, with the Xcode command line tools.

## 2. Getting the sources

```
git clone https://github.com/corvus-dotnet/corvus-regex-ecma262-rs.git
git clone https://github.com/corvus-dotnet/ecma262-regex-test-data.git   # beside it: the test data
cd corvus-regex-ecma262-rs
```

The test data is the oracle for the engine (design, section 6). It is not vendored; the commands below assume it at
`../ecma262-regex-test-data`.

## 3. Building and testing, step by step

Every command runs from the repository root.

1. **Formatted, built and linted.** `cargo fmt --all --check`, `cargo build --workspace --all-targets` and
   `cargo clippy --workspace --all-targets -- -D warnings` must pass.
2. **The tests.** `cargo test --workspace` runs the unit and integration tests, the allocation test and the
   getting-started page's examples (as doctests). `RUSTFLAGS="--cfg corvus_memo_from_the_start" cargo test -p
   corvus-regex-ecma262` (with its own `CARGO_TARGET_DIR`) runs them with the memo of failed places taken into use from
   the first step.
3. **The stack probe.** `cargo run --release --bin stack-probe` asks the deep, wide and long patterns, each in a process
   of its own on stacks of 8 MB, 1 MB and 256 KB; none may end the process.
4. **The test data.** `cargo run --release --bin dataset -- ../ecma262-regex-test-data/data wrong-cases.tsv` must report
   0 wrong, 0 refused and 0 at a limit. For the other versions of Unicode and for ECMAScript 2020, write the data with
   its own scripts and run it with the engine set to match:

   ```
   node ../ecma262-regex-test-data/unicode.js apply 13.0 /tmp/u13 ../ecma262-regex-test-data/data
   cargo run --release --bin dataset -- /tmp/u13 wrong-u13.tsv --unicode 13
   node ../ecma262-regex-test-data/edition.js apply /tmp/es2020 "" ../ecma262-regex-test-data/data
   cargo run --release --bin dataset -- /tmp/es2020 wrong-es2020.tsv --edition es2020
   ```

5. **The C library.** Build it and run the C tests and the C test-data runner against it:

   ```
   cargo build --release -p corvus-regex-ecma262-capi
   cmake -S corvus-regex-ecma262-capi/tests/native -B build/native -DCRE_LIB_DIR=$PWD/target/release
   cmake --build build/native --config Release && ctest --test-dir build/native -C Release --output-on-failure
   cmake -S tools/c -B build/tools -DCRE_LIB_DIR=$PWD/target/release && cmake --build build/tools --config Release
   build/tools/bin/dataset ../ecma262-regex-test-data/data wrong-cases-c.tsv
   ```

   The C runner's counts must be the Rust runner's, file for file. The C tests include the C getting-started page's
   programs.
6. **The C API reference.** After a change to the header, run `pwsh tools/docs/gen-api-c.ps1`;
   `pwsh tools/docs/gen-api-c.ps1 -Check` must pass.

## 4. The Unicode tables and the packages

The tables of each version of Unicode, the properties of strings and the property names of ECMAScript 2020 are made
by `tools/unicode` from the Unicode Character Database, regress's `gen-unicode` and the test data, every download
checked against its SHA-256. `pwsh tools/unicode/check-tables.ps1 -Properties
../ecma262-regex-test-data/es2020-properties.json -WorkDirectory <a scratch directory>` makes them all again and fails
unless they are the checked-in files, as CI runs it. A new version of Unicode is added to those scripts' lists, with
its hashes. The Go, Java, Julia, Pascal and OCaml ports take their tables from these, so a change here is carried to
them.

`corvus-regex-ecma262-capi/package.ps1` makes a platform's archive, and `xcframework.ps1` the XCFramework;
`capi.yml` builds and tests every platform's archive as a consumer would on every change.

## 5. Benchmarks

`tools/benchmarks/run.sh <workloads.json> <texts directory> <out.json>` is the Rust harness (this crate, the `regex`
crate, regress and V8), and `tools/c/run.sh` with the same arguments the C harness (the C library, PCRE2, QuickJS and
V8); `JOB=short` gives a quick look, and each script's header lists its settings. The published figures are on
[the benchmark page](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), which runs both harness images each
night beside every other engine; a run on another machine is comparable with itself, never with the page.

## 6. What a pull request needs

- **The gates of section 3** passing on the change as pushed.
- **The case in the test data first** for a change to the engine's semantics: add the pattern to the data repository's
  family it belongs to, regenerate with its `generate.js`, check with `verify.js`, and pin the new data commit in
  `ci.yml`. V8 is the oracle; a fault V8 has is marked in the data, never copied into the engine.
- **A failing test first** for a bug: the test that reproduces it, then the fix.
- **A measured run** for a change made for speed, with the rows it moved; an optimisation of the search layer is
  considered for the ports too.
- **The design documents kept current**, describing the crate and the library as they are.
- **Doc comments** on every public item, since the API reference on docs.rs is generated from them, and comments on
  every function of the C header, since the C API reference is.
- **The getting-started pages** kept true: their examples are run by the tests.

### Conventions

- Rust formatted by `rustfmt`, clean under clippy at `-D warnings`; the header C99.
- Nothing on the match path allocates once warm: the working memory is the thread's, and the allocation test holds
  that line.
- No panic crosses the C boundary, and every C function sets its out-parameters on failure.

## 7. Releases

Every pull request merged into main releases the crate and the C library with the next patch version, unless it is
labelled `no_release`: `auto-release.yml` runs `tools/release` on the merge, which makes a GitHub release tagged
`v<version>`; `crates-publish.yml` publishes the crate to crates.io, and `capi-publish.yml` attaches every platform's
archive and the XCFramework. A change to the published documents releases a patch too, since the documentation site
takes them from the latest release. A change to a benchmark harness alone is a harness release, which publishes the
harness images. A pull request that needs a minor or major version sets `nextVersion` in `.github/release.json`.

`main` is protected by a ruleset: changes arrive by pull request, and the `CI gate`, `Harness image` and `C harness
image` checks must be green on the branch as it will merge.
