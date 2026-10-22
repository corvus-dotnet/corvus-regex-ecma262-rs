//! The Industry benchmark: each workload counted by each engine, as the V8 harness of the .NET reference engine
//! (`tools/benchmarks/v8/industry.mjs`) counts it, in nanoseconds per count.
//!
//!   industry <workloads.json> [--engines corvus-utf8,corvus-utf16,regex,regress,v8] [--v8 <industry.mjs>]
//!            [--rounds 3] [--seconds 0.7] [--passes 7] [--spread 0.05] [--filter <substring of the id>]
//!            [--out <results.json>]
//!
//! The protocol is interleaved, so that what changes on the host over a run (its clock, its heat, its other work)
//! falls on every engine alike rather than on whichever engine was running then: the workloads are the outer loop,
//! each workload's rounds next and the engines innermost, their order turned by one each round. A workload's rounds
//! are therefore seconds apart, as its engines are: on an unpinned host, whose speed drifts over minutes, rounds
//! minutes apart spread a figure by more than the engines differ. Each figure is a child process of
//! its own (as BenchmarkDotNet runs out of process), so that one engine's caches and allocations do not reach
//! another's figures; it warms for two fifths of its seconds, then times `passes` passes of as many counts as fit
//! the rest and reports their median. A workload's figure for an engine is the median of its rounds, with the
//! fastest and the slowest round as its spread. Where the spread, (slowest - fastest) / median, is over `--spread`,
//! the figure is listed in the run's facts (`spread_over`) and on stderr: it is not a clean figure. A child that does
//! not finish in two minutes is stopped and recorded as such. A count is the global search of ECMA-262: after an
//! empty match the search goes on one code point later.
//!
//! The engines: this crate over UTF-8 (`&str`) and over UTF-16 (`&[u16]`), with the workload's flags and `u`, as
//! the Corvus rows of the reference run carry; `regex`, the platform's standard engine, with RE2 semantics, which
//! is given the pattern as it is and the flags i, m and s, and whose row is absent where the pattern does not
//! compile; `regress`, an ECMA-262 engine, with the workload's flags and `u`; and V8 (`--v8`, the harness run by
//! `node`), with `gu` added. An engine written `name@path` is run by the harness binary at `path`, so that two
//! builds can be timed side by side in the same rounds (an interleaved A/B).
//!
//! The texts are those of the reference run, cached in `~/.local/share/corvus-regex-benchmarks` (or
//! `CORVUS_REGEX_TEXTS`) and fetched from dotnet/performance when missing. The run unzips each once into a directory
//! of its own (`CORVUS_REGEX_PLAIN`), which the children read.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use corvus_regex_ecma262_tools::json::{self, Json};

const SOURCE: &str = "https://raw.githubusercontent.com/dotnet/performance/main/src/benchmarks/micro/libraries/System.Text.RegularExpressions/TestData/";
const ENGINES: [&str; 4] = ["corvus-utf8", "corvus-utf16", "regex", "regress"];

struct Workload {
    id: String,
    pattern: String,
    flags: String,
    file: String,
    /// V8's count, which the central run writes into the workloads once, for a run without V8 (V8=0) to check against.
    v8_count: Option<f64>,
}

fn workloads(path: &str) -> Vec<Workload> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let all = json::parse(&text).unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .map(|w| Workload {
            id: w.get("id").and_then(Json::as_string).unwrap(),
            pattern: w.get("pattern").and_then(Json::as_string).unwrap(),
            flags: w.get("flags").and_then(Json::as_string).unwrap(),
            file: w.get("file").and_then(Json::as_string).unwrap(),
            v8_count: w.get("v8Count").and_then(Json::as_f64),
        })
        .collect()
}

fn text_dir() -> std::path::PathBuf {
    match std::env::var_os("CORVUS_REGEX_TEXTS") {
        Some(d) => d.into(),
        None => std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"))
            .join(".local/share/corvus-regex-benchmarks"),
    }
}

/// The text of a file: gunzipped, without a byte order mark (read as it is from `CORVUS_REGEX_PLAIN` when the run
/// has unzipped it there).
fn text(file: &str) -> String {
    if let Some(plain) = std::env::var_os("CORVUS_REGEX_PLAIN") {
        let path = std::path::Path::new(&plain).join(file.trim_end_matches(".gz"));
        if let Ok(s) = std::fs::read_to_string(&path) {
            return s;
        }
    }
    let path = text_dir().join(file);
    if !path.exists() {
        std::fs::create_dir_all(text_dir()).unwrap();
        let status =
            Command::new("curl").args(["-sSfL", "-o"]).arg(&path).arg(format!("{SOURCE}{file}")).status().unwrap();
        assert!(status.success(), "could not fetch {file}");
    }
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&path).unwrap()).read_to_end(&mut bytes).unwrap();
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    String::from_utf8(bytes).unwrap()
}

/// The workload's flags with `u` added, each once.
fn flags_u(flags: &str) -> String {
    let mut f: Vec<char> = flags.chars().collect();
    if !f.contains(&'u') && !f.contains(&'v') {
        f.push('u');
    }
    f.into_iter().collect()
}

/// Times a count: warm, then passes. Returns the median, the fastest and the slowest pass in nanoseconds per count,
/// the count, and the counts timed.
fn measure(seconds: f64, passes: usize, mut count: impl FnMut() -> usize) -> (f64, f64, f64, usize, usize) {
    let expected = count();
    let warm_end = Instant::now() + Duration::from_secs_f64(seconds * 0.4);
    let mut reps = 0usize;
    while Instant::now() < warm_end {
        assert_eq!(count(), expected, "the count changed");
        reps += 1;
    }
    let per_pass = ((reps as f64 * 0.6 / passes as f64).round() as usize).max(1);
    let mut times = Vec::with_capacity(passes);
    for _ in 0..passes {
        let t0 = Instant::now();
        let mut sink = 0;
        for _ in 0..per_pass {
            sink += count();
        }
        std::hint::black_box(sink);
        times.push(t0.elapsed().as_nanos() as f64 / per_pass as f64);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[passes / 2], times[0], times[passes - 1], expected, per_pass * passes)
}

/// Runs one workload on one engine in this process and prints its figures as one line of JSON.
fn one(path: &str, index: usize, engine: &str, seconds: f64, passes: usize) {
    let w = &workloads(path)[index];
    let s = text(&w.file);
    let flags = flags_u(&w.flags);
    let figures = match engine {
        "corvus-utf8" => {
            let re = corvus_regex_ecma262::Regex::new(&w.pattern, &flags).map_err(|e| e.to_string());
            re.map(|re| measure(seconds, passes, || re.find_iter(&s).count()))
        }
        "corvus-utf16" => {
            let s16: Vec<u16> = s.encode_utf16().collect();
            let re = corvus_regex_ecma262::Regex::new(&w.pattern, &flags).map_err(|e| e.to_string());
            re.map(|re| measure(seconds, passes, || re.find_iter_utf16(&s16).count()))
        }
        "regex" => {
            let re = regex::RegexBuilder::new(&w.pattern)
                .case_insensitive(w.flags.contains('i'))
                .multi_line(w.flags.contains('m'))
                .dot_matches_new_line(w.flags.contains('s'))
                .build()
                .map_err(|e| e.to_string().lines().last().unwrap_or("").to_string());
            re.map(|re| measure(seconds, passes, || re.find_iter(&s).count()))
        }
        "regress" => {
            let re = regress::Regex::with_flags(&w.pattern, flags.as_str()).map_err(|e| e.to_string());
            re.map(|re| measure(seconds, passes, || re.find_iter(&s).count()))
        }
        other => panic!("unknown engine {other}"),
    };
    match figures {
        Ok((median, min, max, count, reps)) => {
            println!("{{\"median\":{median:.1},\"min\":{min:.1},\"max\":{max:.1},\"count\":{count},\"reps\":{reps}}}")
        }
        Err(e) => println!("{{\"error\":{}}}", quote(&e)),
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn first_line(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::to_string))
        .unwrap_or_default()
}

/// What the run is: the host, the compiler, the affinity, the load and the commit, read from the run itself.
fn facts(seconds: &str, passes: &str, extra: &[(&str, String)]) -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|c| {
            c.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default());
    let affinity = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Cpus_allowed_list"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_default();
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|l| l.split(' ').next().unwrap_or("").to_string())
        .unwrap_or_default();
    let fields = [
        ("cpu", cpu),
        ("cores", std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_default()),
        ("affinity", affinity),
        ("os", first_line("uname", &["-sr"])),
        ("load", load),
        ("rustc", env!("CORVUS_RUSTC_VERSION").to_string()),
        // The commit the image was built from (CORVUS_COMMIT, set by the Dockerfile), or the checkout's.
        (
            "commit",
            std::env::var("CORVUS_COMMIT")
                .ok()
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| first_line("git", &["rev-parse", "HEAD"])),
        ),
        ("date", first_line("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])),
        ("seconds", seconds.to_string()),
        ("passes", passes.to_string()),
        // On a GitHub-hosted runner, what the runner says it is.
        ("runner", std::env::var("RUNNER_NAME").unwrap_or_default()),
        ("runner_os", std::env::var("RUNNER_OS").unwrap_or_default()),
        (
            "image",
            format!(
                "{} {}",
                std::env::var("ImageOS").unwrap_or_default(),
                std::env::var("ImageVersion").unwrap_or_default()
            )
            .trim()
            .to_string(),
        ),
        ("corvus-regex-ecma262", env!("CORVUS_VERSION").to_string()),
        ("regex", env!("REGEX_VERSION").to_string()),
        ("regress", env!("REGRESS_VERSION").to_string()),
    ];
    let body: Vec<String> = fields.iter().chain(extra).map(|(k, v)| format!("{}:{}", quote(k), quote(v))).collect();
    format!("{{{}}}", body.join(","))
}

/// One figure: `engine` over workload `index` in a child process. The child's line of JSON, or one with the error.
fn figure(
    exe: &std::path::Path,
    v8: Option<&str>,
    path: &str,
    index: usize,
    engine: &str,
    seconds: &str,
    passes: &str,
) -> String {
    // An engine named `name@path` is `name` as the harness binary at `path` runs it: two builds side by side, for an
    // interleaved A/B.
    let (engine, exe) = match engine.split_once('@') {
        Some((name, path)) => (name, std::path::Path::new(path)),
        None => (engine, exe),
    };
    let mut command = if engine == "v8" {
        let mut c = Command::new("node");
        c.arg(v8.expect("the V8 harness"));
        c
    } else {
        Command::new(exe)
    };
    if engine == "v8" {
        command.args(["--one", path, &index.to_string(), seconds, passes]);
    } else {
        command.args(["--one", path, &index.to_string(), engine, seconds, passes]);
    }
    let mut child = match command.stdout(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return format!("{{\"error\":{}}}", quote(&e.to_string())),
    };
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let line = BufReader::new(stdout).lines().next().and_then(Result::ok);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Some(line)) => line,
        Ok(None) => "{\"error\":\"the process ended without figures\"}".to_string(),
        Err(_) => "{\"error\":\"did not finish in two minutes\"}".to_string(),
    };
    let _ = child.kill();
    let _ = child.wait();
    line
}

/// The median of a sorted list.
fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 }
}

/// Unzips each text the workloads name into `dir`, once, for the children to read.
fn unzip_texts(all: &[Workload], dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let mut done: Vec<&str> = Vec::new();
    for w in all {
        if done.contains(&w.file.as_str()) {
            continue;
        }
        done.push(&w.file);
        std::fs::write(dir.join(w.file.trim_end_matches(".gz")), text(&w.file)).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--one") {
        one(&args[1], args[2].parse().unwrap(), &args[3], args[4].parse().unwrap(), args[5].parse().unwrap());
        return;
    }
    let opt = |name: &str| args.iter().position(|a| a == name).map(|k| args[k + 1].clone());
    let path = &args[0];
    let v8 = opt("--v8");
    let engines: Vec<String> =
        opt("--engines").map(|e| e.split(',').map(String::from).collect()).unwrap_or_else(|| {
            let mut e: Vec<String> = ENGINES.map(String::from).to_vec();
            if v8.is_some() {
                e.push("v8".into());
            }
            e
        });
    let rounds: usize = opt("--rounds").map_or(3, |r| r.parse().unwrap());
    let seconds = opt("--seconds").unwrap_or_else(|| "0.7".into());
    let passes = opt("--passes").unwrap_or_else(|| "7".into());
    let limit: f64 = opt("--spread").map_or(0.05, |s| s.parse().unwrap());
    let filter = opt("--filter");
    let all = workloads(path);
    let chosen: Vec<usize> =
        (0..all.len()).filter(|&k| filter.as_ref().is_none_or(|f| all[k].id.contains(f.as_str()))).collect();
    // The texts unzipped once, for every child.
    let plain = std::env::temp_dir().join(format!("corvus-regex-texts-{}", std::process::id()));
    unzip_texts(&all, &plain);
    // SAFETY: set before any thread is started.
    unsafe { std::env::set_var("CORVUS_REGEX_PLAIN", &plain) };
    let exe = std::env::current_exe().unwrap();
    // figures[e][k]: each round's median, and the last line of each (for the count and errors).
    let mut figures: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); all.len()]; engines.len()];
    let mut lines: Vec<Vec<Option<String>>> = vec![vec![None; all.len()]; engines.len()];
    let mut v8_engine = String::new();
    for (done, &k) in chosen.iter().enumerate() {
        for round in 0..rounds {
            for turn in 0..engines.len() {
                let e = (turn + round) % engines.len();
                let line = figure(&exe, v8.as_deref(), path, k, &engines[e], &seconds, &passes);
                let parsed = json::parse(&line).ok();
                match parsed.as_ref().and_then(|j| j.get("median")).and_then(Json::as_f64) {
                    Some(m) => figures[e][k].push(m),
                    None => eprintln!("{:<13} {:<62} {line}", engines[e], all[k].id),
                }
                if engines[e] == "v8"
                    && let Some(name) = parsed.as_ref().and_then(|j| j.get("engine")).and_then(Json::as_string)
                {
                    v8_engine = name;
                }
                lines[e][k] = Some(line);
            }
        }
        eprintln!("workload {} of {} done", done + 1, chosen.len());
    }
    let _ = std::fs::remove_dir_all(&plain);
    let mut over: Vec<String> = Vec::new();
    let mut differ: Vec<String> = Vec::new();
    let mut results: Vec<String> = Vec::new();
    // V8 is the oracle of the counts: an ECMA-262 engine that counts otherwise is wrong on the row, and regex,
    // which does not claim ECMA-262, is marked where its count differs.
    let count_of = |line: &Option<String>| {
        line.as_ref().and_then(|l| json::parse(l).ok()).and_then(|j| j.get("count").and_then(Json::as_f64))
    };
    let v8_counts: Vec<Option<f64>> = match engines.iter().position(|e| e == "v8") {
        Some(e) => lines[e].iter().map(count_of).collect(),
        // Without V8 in the run, its counts as the workloads carry them, when they do.
        None => all.iter().map(|w| w.v8_count).collect(),
    };
    for (e, engine) in engines.iter().enumerate() {
        let mut rows = Vec::new();
        for &k in &chosen {
            let line = lines[e][k].clone().unwrap_or_default();
            let mut times = figures[e][k].clone();
            let in_order: Vec<String> = times.iter().map(|t| format!("{t:.1}")).collect();
            let entry = if times.len() < rounds {
                // A round with no figure (an error, a limit) makes the figure an error.
                if line.contains("\"error\"") { line } else { "{\"error\":\"a round gave no figure\"}".to_string() }
            } else {
                times.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let (m, lo, hi) = (median(&times), times[0], times[times.len() - 1]);
                let spread = if m > 0.0 { (hi - lo) / m } else { 0.0 };
                if spread > limit {
                    over.push(format!("{engine} {} {:.1}%", all[k].id, spread * 100.0));
                    eprintln!(
                        "SPREAD {engine:<13} {:<62} {:.1}% over {:.1}%",
                        all[k].id,
                        spread * 100.0,
                        limit * 100.0
                    );
                }
                let count = count_of(&Some(line.clone())).unwrap_or(0.0);
                let figure = format!(
                    "\"median\":{m:.1},\"min\":{lo:.1},\"max\":{hi:.1},\"count\":{count},\"rounds\":{},\"spread\":{spread:.4},\"each\":[{}]",
                    times.len(),
                    in_order.join(",")
                );
                match v8_counts[k] {
                    Some(oracle) if oracle != count && engine != "v8" => {
                        differ.push(format!("{engine} {} {count} vs V8 {oracle}", all[k].id));
                        eprintln!("COUNT  {engine:<13} {:<62} {count} against V8's {oracle}", all[k].id);
                        if engine == "regex" {
                            format!("{{{figure},\"differs\":\"regex {count}, V8 {oracle}\"}}")
                        } else {
                            format!("{{\"error\":\"counted {count} where V8 counts {oracle}\"}}")
                        }
                    }
                    _ => format!("{{{figure}}}"),
                }
            };
            rows.push(format!("{}:{entry}", quote(&all[k].id)));
        }
        results.push(format!("{}:{{{}}}", quote(engine), rows.join(",")));
    }
    let facts = facts(
        &seconds,
        &passes,
        &[
            ("rounds", rounds.to_string()),
            ("protocol", "interleaved".to_string()),
            ("spread_limit", format!("{limit}")),
            ("spread_over", over.join("; ")),
            ("counts_differ", differ.join("; ")),
            ("v8", v8_engine),
        ],
    );
    // The time the run ended, read when it did.
    let end = first_line("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]);
    let out = format!("{{\"facts\":{facts},\"end\":{},\"results\":{{{}}}}}\n", quote(&end), results.join(","));
    match opt("--out") {
        Some(file) => std::fs::write(file, out).unwrap(),
        None => print!("{out}"),
    }
}
