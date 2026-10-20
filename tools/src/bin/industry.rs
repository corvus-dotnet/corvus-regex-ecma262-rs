//! The Industry benchmark: each workload counted by each engine, as the V8 harness of the .NET reference engine
//! (`tools/benchmarks/v8/industry.mjs`) counts it: warm for two fifths of the budget, then passes of as many counts
//! as fit the rest, and the median, the fastest and the slowest pass, in nanoseconds per count.
//!
//!   industry <workloads.json> [--engines corvus-utf8,corvus-utf16,regex,regress] [--seconds 1.5] [--passes 5]
//!            [--filter <substring of the id>] [--out <results.json>]
//!
//! Each workload of each engine runs in a child process of its own (as BenchmarkDotNet runs out of process), so that
//! one engine's caches and allocations do not reach another's figures, and a run that does not finish in two minutes
//! is stopped and recorded as such. A count is the global search of ECMA-262: after an empty match the search goes
//! on one code point later.
//!
//! The engines: this crate over UTF-8 (`&str`) and over UTF-16 (`&[u16]`), with the workload's flags and `u`, as
//! the Corvus rows of the reference run carry; `regex`, the platform's standard engine, with RE2 semantics, which
//! is given the pattern as it is and the flags i, m and s, and whose row is absent where the pattern does not
//! compile; and `regress`, an ECMA-262 engine, with the workload's flags and `u`.
//!
//! The texts are those of the reference run, cached in `~/.local/share/corvus-regex-benchmarks` (or
//! `CORVUS_REGEX_TEXTS`) and fetched from dotnet/performance when missing.

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

/// The text of a file: gunzipped, without a byte order mark.
fn text(file: &str) -> String {
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
fn facts() -> String {
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
        ("commit", first_line("git", &["rev-parse", "HEAD"])),
        ("date", first_line("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])),
    ];
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{}", quote(k), quote(v))).collect();
    format!("{{{}}}", body.join(","))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--one") {
        one(&args[1], args[2].parse().unwrap(), &args[3], args[4].parse().unwrap(), args[5].parse().unwrap());
        return;
    }
    let opt = |name: &str| args.iter().position(|a| a == name).map(|k| args[k + 1].clone());
    let path = &args[0];
    let engines: Vec<String> = opt("--engines")
        .map(|e| e.split(',').map(String::from).collect())
        .unwrap_or(ENGINES.map(String::from).to_vec());
    let seconds = opt("--seconds").unwrap_or_else(|| "1.5".into());
    let passes = opt("--passes").unwrap_or_else(|| "5".into());
    let filter = opt("--filter");
    let all = workloads(path);
    let facts = facts();
    let mut results: Vec<String> = Vec::new();
    for engine in &engines {
        let mut rows = Vec::new();
        for (k, w) in all.iter().enumerate() {
            if filter.as_ref().is_some_and(|f| !w.id.contains(f.as_str())) {
                continue;
            }
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--one", path, &k.to_string(), engine, &seconds, &passes])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
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
            eprintln!("{engine:<13} {:<62} {line}", w.id);
            rows.push(format!("{}:{line}", quote(&w.id)));
        }
        results.push(format!("{}:{{{}}}", quote(engine), rows.join(",")));
    }
    let out = format!("{{\"facts\":{facts},\"results\":{{{}}}}}\n", results.join(","));
    match opt("--out") {
        Some(file) => std::fs::write(file, out).unwrap(),
        None => print!("{out}"),
    }
}
