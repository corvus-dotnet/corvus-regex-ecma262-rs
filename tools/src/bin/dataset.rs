//! Runs an engine over the ECMA-262 regular expression test data (github.com/corvus-dotnet/ecma262-regex-test-data).
//!
//!   dataset <data directory> <wrong-cases output file> [file filter] [utf16|utf8|both] [--engine corvus|regress]
//!           [--compliance <file>]
//!
//! Each record is asked about under each grammar V8 found it valid in: the record's flags with u added (the
//! `unicode` answers), the record's flags alone (the `legacy` answers) and, in the file that has them, the flags with
//! v added (the `unicode_sets` answers). A text is matched as UTF-16 and as UTF-8; a text with a lone surrogate has no
//! UTF-8 form and is matched as UTF-16 alone. Records marked v8_deviation are left out. Any difference from V8 is a
//! wrong answer; a refusal of a valid pattern, an acceptance of an invalid one, and a match that reaches its limit are
//! counted apart and listed too. `captures.jsonl` is judged on exec, the global search, the sticky flag, replace and
//! split, as the .NET runner of the reference engine judges it. `ill-formed-utf8.jsonl` holds bytes that are not
//! UTF-8, whose right answer over UTF-8 is a refusal: a `str` cannot hold them, so the bytes are refused where they
//! would become one, before the engine, as a program that uses it refuses them (both engines take a `str`).
//!
//! The judging rules are those of `Corvus.Regex.Ecma262.DatasetRunner`, the runner of the .NET engine.
//!
//! With `--engine regress` the `regress` crate is judged instead: over UTF-16 by its UTF-16 search with the u or v
//! flag and its UCS-2 search without, which is how ECMA-262 reads the text, and over UTF-8 by its search of a `str`.
//! `exec/test262.jsonl`, the exec calls of test262 (in a directory of its own), is judged last: each call one exec
//! with the record's flags from its lastIndex, right when the positions of the match and of every group, the named
//! groups and, with g or y, the lastIndex after are V8's; one answer over UTF-16 and, when the text holds no lone
//! surrogate and lastIndex is not between the halves of a pair, one over UTF-8 with positions in bytes. regress has
//! no g or y flag: it searches from lastIndex, and with y a match counts only where it starts at lastIndex.
//!
//! regress has no global search, sticky flag, replace or split of its own, so on `captures.jsonl` the runner writes
//! them over its exec, as a JavaScript engine's String methods are written, in both encodings. regress has no limit on
//! a match, so each file is run in child processes (each with its address space capped at 2 GB) that report each
//! answer as they give it. In the child a watchdog thread times each answer by the CPU time of the thread that runs
//! regress, from just before regress is called, and ends the process when one passes the 2 seconds every runner gives
//! a match; that answer, or one whose child dies, is unanswered, and a new child goes on from the next answer. Once
//! three answers of a record have reached the limit, its other answers are unanswered without being asked, so that a
//! pattern regress cannot answer on any text costs seconds, not one limit for each of its answers.
//!
//! With `--compliance <file>`, the engine's compliance object is written to the file, as the benchmark page reads it
//! (docs/implementing-a-source.md in corvus-regex-ecma262-benchmarks): the counts, this runner and the calls it makes
//! in each encoding, and what it leaves out (nothing for this crate; for regress, the operations written over its
//! exec).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use corvus_regex_ecma262::{Error, MatchError, Regex};
use corvus_regex_ecma262_tools::json::{self, Json, escape};

const LIMIT: Duration = Duration::from_secs(2);
/// How long a child that runs regress may go without a word before it is taken for lost (its watchdog ends it within
/// LIMIT of an answer that does not finish; this catches a child stuck anywhere else).
const SILENCE: Duration = Duration::from_secs(60);
/// The answers of a record that may reach the limit before its other answers are unanswered without being asked.
const LIMITS_PER_RECORD: usize = 3;
const REPLACEMENT: &str = "<$&|$1|$2|$10|$<a>|$$|$`|$'>";

#[derive(Default)]
struct Counts {
    right: AtomicI64,
    wrong: AtomicI64,
    refused: AtomicI64,
    accepted: AtomicI64,
    limited: AtomicI64,
    errors: AtomicI64,
}

impl Counts {
    fn add(&self, other: &Counts) {
        for (a, b) in [
            (&self.right, &other.right),
            (&self.wrong, &other.wrong),
            (&self.refused, &other.refused),
            (&self.accepted, &other.accepted),
            (&self.limited, &other.limited),
            (&self.errors, &other.errors),
        ] {
            a.fetch_add(b.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }

    fn get(&self) -> [i64; 6] {
        [&self.right, &self.wrong, &self.refused, &self.accepted, &self.limited, &self.errors]
            .map(|c| c.load(Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum EngineKind {
    Corvus,
    Regress,
}

struct Options {
    utf16: bool,
    utf8: bool,
    engine: EngineKind,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--worker") {
        // dataset --worker <file> <first record> <first answer of it> <spent: 0|1> <utf16|utf8|both>
        worker(&args[1], args[2].parse().unwrap(), args[3].parse().unwrap(), args[4] == "1", &args[5]);
        return;
    }
    let mut args = args;
    let compliance = args.iter().position(|a| a == "--compliance").map(|k| {
        let path = args[k + 1].clone();
        args.drain(k..k + 2);
        path
    });
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let engine = match args.iter().position(|a| a == "--engine").map(|k| args[k + 1].as_str()) {
        None | Some("corvus") => EngineKind::Corvus,
        Some("regress") => EngineKind::Regress,
        Some(other) => panic!("unknown engine {other}"),
    };
    let positional: Vec<&String> =
        positional.into_iter().filter(|a| !matches!(a.as_str(), "corvus" | "regress")).collect();
    let (directory, wrong_path) = (positional[0], positional[1]);
    let filter = positional.get(2).map(|s| s.as_str()).unwrap_or("*.jsonl");
    let encodings = positional.get(3).map(|s| s.as_str()).unwrap_or("both");
    let options =
        Options { utf16: matches!(encodings, "both" | "utf16"), utf8: matches!(encodings, "both" | "utf8"), engine };
    let mut files: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter(|p| glob(filter, &p.file_name().unwrap().to_string_lossy()))
        .collect();
    files.sort();
    // The exec calls of test262, in a directory of their own: with the default filter, or one that names them.
    let exec = std::path::Path::new(directory).join(EXEC);
    if (filter == "*.jsonl" || filter.starts_with("exec")) && exec.is_file() {
        files.push(exec);
    }
    let total = Counts::default();
    let wrong_cases = Mutex::new(Vec::<String>::new());
    let (records, skipped) = (AtomicI64::new(0), AtomicI64::new(0));
    let clock = Instant::now();
    for file in &files {
        let name = data_name(file);
        let counts = Counts::default();
        if engine == EngineKind::Regress {
            supervise(file.to_str().unwrap(), encodings, &counts, &wrong_cases, &records, &skipped);
        } else {
            let text = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let next = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        let mut wrong = Vec::new();
                        loop {
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(line) = lines.get(k) else { break };
                            let r = json::parse(line).unwrap_or_else(|e| panic!("{name}:{}: {e}", k + 1));
                            if r.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
                                skipped.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            records.fetch_add(1, Ordering::Relaxed);
                            judge_record(&name, &r, &options, &counts, &mut wrong);
                        }
                        wrong_cases.lock().unwrap().extend(wrong);
                    });
                }
            });
        }
        let [right, wrong, refused, accepted, limited, _] = counts.get();
        println!(
            "{name:<24} right {right:>10}  wrong {wrong:>6}  refused-valid {refused:>4}  accepted-invalid {accepted:>4}  limit {limited:>4}  {:>6.1}s",
            clock.elapsed().as_secs_f64()
        );
        total.add(&counts);
    }
    let mut cases = wrong_cases.into_inner().unwrap();
    cases.sort();
    std::fs::write(wrong_path, cases.join("\n") + if cases.is_empty() { "" } else { "\n" }).unwrap();
    let [right, wrong, refused, accepted, limited, errors] = total.get();
    if let Some(path) = compliance {
        write_compliance(&path, engine, &options, [right, wrong, refused, accepted, limited + errors]);
    }
    println!();
    println!(
        "records {} (skipped {} deviations)  right {right}  wrong {wrong}  refused-valid {refused}  accepted-invalid {accepted}  limit {limited}  errors {errors}  in {:.1}s",
        records.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed),
        clock.elapsed().as_secs_f64()
    );
    std::process::exit(if wrong + refused + accepted + errors + limited == 0 { 0 } else { 1 });
}

/// Writes the engine's compliance as the benchmark page reads it: the counts (right, wrong, refused, accepted invalid,
/// unanswered), this runner and the calls it makes in each encoding it asked in, and what it leaves out.
fn write_compliance(path: &str, engine: EngineKind, options: &Options, counts: [i64; 5]) {
    let (utf16, utf8, excludes): (&[&str], &[&str], &str) = match engine {
        EngineKind::Corvus => (
            &[
                "Regex::is_match_utf16",
                "Regex::captures_at_within_utf16",
                "Regex::find_iter_utf16",
                "Regex::replace_utf16",
                "Regex::split_utf16",
            ],
            &["Regex::is_match", "Regex::captures_at_within", "Regex::find_iter", "Regex::replace"],
            "[]",
        ),
        EngineKind::Regress => (
            &["Regex::find_from_utf16", "Regex::find_from_ucs2"],
            &["Regex::find_from"],
            r#"[{"class":"operations","leaves":["global","sticky","replace","split"]}]"#,
        ),
    };
    let list = |calls: &[&str]| calls.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
    let mut calls = Vec::new();
    if options.utf16 {
        calls.push(format!("\"utf16\":[{}]", list(utf16)));
    }
    if options.utf8 {
        calls.push(format!("\"utf8\":[{}]", list(utf8)));
    }
    let [right, wrong, refused, accepted, limited] = counts;
    let json = format!(
        "{{\"right\":{right},\"wrong\":{wrong},\"refused\":{refused},\"acceptedInvalid\":{accepted},\"limited\":{limited},\
         \"judge\":{{\"runner\":\"https://github.com/corvus-dotnet/corvus-regex-ecma262-rs/blob/main/tools/src/bin/dataset.rs\",\
         \"calls\":{{{}}}}},\"excludes\":{excludes}}}\n",
        calls.join(",")
    );
    std::fs::write(path, json).unwrap();
}

/// Whether a file name matches a filter with `*` wildcards.
fn glob(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (k, part) in parts.iter().enumerate() {
        if k == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if k == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

fn has_lone_surrogate(s: &[u16]) -> bool {
    let mut k = 0;
    while k < s.len() {
        let u = s[k];
        if (0xD800..=0xDBFF).contains(&u) {
            if k + 1 < s.len() && (0xDC00..=0xDFFF).contains(&s[k + 1]) {
                k += 2;
                continue;
            }
            return true;
        }
        if (0xDC00..=0xDFFF).contains(&u) {
            return true;
        }
        k += 1;
    }
    false
}

/// The modes a record is asked about: the flags to add, the name of the answers, and whether V8 found it valid.
fn modes(r: &Json) -> Vec<(&'static str, &'static str, bool)> {
    let valid = r.get("valid").unwrap();
    let mut modes = vec![
        ("u", "unicode", valid.get("unicode").and_then(Json::as_bool).unwrap()),
        ("", "legacy", valid.get("legacy").and_then(Json::as_bool).unwrap()),
    ];
    if let Some(v) = valid.get("unicode_sets") {
        modes.push(("v", "unicode_sets", v.as_bool().unwrap()));
    }
    modes
}

/// An engine judged on the answers of `test`.
trait Engine {
    type Re;
    /// Compiles a pattern: the regular expression, or the error's kind and message.
    fn compile(pattern: &str, flags: &str) -> Result<Self::Re, (bool, String)>;
    fn test_utf16(re: &Self::Re, text: &[u16]) -> Result<bool, String>;
    fn test_utf8(re: &Self::Re, text: &str) -> Result<bool, String>;
}

struct Corvus;

impl Engine for Corvus {
    type Re = Regex;

    fn compile(pattern: &str, flags: &str) -> Result<Regex, (bool, String)> {
        Regex::new(pattern, flags).map_err(|e| match e {
            Error::Invalid(m) => (false, format!("PARSE: {m}")),
            Error::TooLarge(m) => (false, format!("TOO-LARGE: {m}")),
            other => (true, format!("ERROR: {other}")),
        })
    }

    fn test_utf16(re: &Regex, text: &[u16]) -> Result<bool, String> {
        re.is_match_within_utf16(text, LIMIT).map_err(limit_name)
    }

    fn test_utf8(re: &Regex, text: &str) -> Result<bool, String> {
        re.is_match_within(text, LIMIT).map_err(limit_name)
    }
}

fn limit_name(e: MatchError) -> String {
    match e {
        MatchError::Memory => "memory".to_string(),
        _ => "time".to_string(),
    }
}

struct Regress;

impl Engine for Regress {
    type Re = (regress::Regex, bool);

    fn compile(pattern: &str, flags: &str) -> Result<(regress::Regex, bool), (bool, String)> {
        let unicode = flags.contains('u') || flags.contains('v');
        match std::panic::catch_unwind(|| regress::Regex::with_flags(pattern, flags)) {
            Ok(Ok(re)) => Ok((re, unicode)),
            Ok(Err(e)) => Err((false, format!("PARSE: {e}"))),
            Err(_) => Err((true, "ERROR: panic while compiling".to_string())),
        }
    }

    fn test_utf16((re, unicode): &(regress::Regex, bool), text: &[u16]) -> Result<bool, String> {
        std::panic::catch_unwind(|| {
            if *unicode {
                re.find_from_utf16(text, 0).next().is_some()
            } else {
                re.find_from_ucs2(text, 0).next().is_some()
            }
        })
        .map_err(|_| "panic".to_string())
    }

    fn test_utf8((re, _): &(regress::Regex, bool), text: &str) -> Result<bool, String> {
        std::panic::catch_unwind(|| re.find(text).is_some()).map_err(|_| "panic".to_string())
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Answers in a child process that runs regress.
//
// regress cannot be stopped in a match, so its supervisor runs it in child processes. In a child, `answer` wraps each
// answer the data asks: it writes the counts that changed before it (a verdict) and after it (the answer itself, then
// "." or, for one right answer and nothing else, "+"), so that the supervisor has every answer given when a child
// ends; and it notes when regress is about to be called, so that the child's watchdog thread can end the process when
// one answer passes LIMIT of the CPU time of the thread that runs regress (which the load of the machine does not
// lengthen). The supervisor counts that answer unanswered and starts a child at the next one, which skips the answers
// before it (`answer` does not run them, and drops the verdicts among them, which were written); when the record has
// had LIMITS_PER_RECORD answers at the limit, the child counts its other answers unanswered without running them.
// Outside a child, `answer` runs the answer and nothing else.

/// Whether this process is a child of the supervisor.
static WORKER: AtomicBool = AtomicBool::new(false);
/// The number of the next answer of the record being judged, and the first that this child gives.
static NEXT_ANSWER: AtomicUsize = AtomicUsize::new(0);
static FIRST_ANSWER: AtomicUsize = AtomicUsize::new(0);
/// Whether the first record of this child has had LIMITS_PER_RECORD answers at the limit.
static SPENT: AtomicBool = AtomicBool::new(false);
/// When the answer being given began, in nanoseconds since BASE plus one; 0 when none is being given.
static STARTED: AtomicU64 = AtomicU64::new(0);
static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// The counts and the number of wrong cases already written for the record being judged.
static WRITTEN: Mutex<([i64; 6], usize)> = Mutex::new(([0; 6], 0));

/// The clock answers are timed by: the CPU time of the thread that runs regress where the system gives it (set by
/// `use_thread_clock` on that thread), else the time since BASE; in nanoseconds plus one.
#[cfg(target_os = "linux")]
static CLOCK: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(target_os = "linux")]
mod thread_clock {
    #[repr(C)]
    pub struct Timespec {
        pub sec: i64,
        pub nsec: i64,
    }
    unsafe extern "C" {
        pub fn pthread_self() -> usize;
        pub fn pthread_getcpuclockid(thread: usize, clock: *mut i32) -> i32;
        pub fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
}

/// Marks the process as one that dumps no core when it dies. A match that passes the cap on the child's memory aborts
/// it, which is counted as unanswered; where the system pipes cores to a program (WSL does), writing the core of a
/// child of over a gigabyte would hold the child for a minute.
fn no_core_dump() {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn prctl(option: i32, ...) -> i32;
        }
        const PR_SET_DUMPABLE: i32 = 4;
        // SAFETY: PR_SET_DUMPABLE takes one integer argument and changes nothing but whether the process dumps core.
        unsafe {
            prctl(PR_SET_DUMPABLE, 0u64);
        }
    }
}

/// Times answers by the CPU time of the calling thread from now on.
fn use_thread_clock() {
    #[cfg(target_os = "linux")]
    {
        let mut clock = 0;
        // SAFETY: pthread_self names the calling thread, and clock is a valid place for the clock's id.
        if unsafe { thread_clock::pthread_getcpuclockid(thread_clock::pthread_self(), &mut clock) } == 0 {
            CLOCK.store(clock, Ordering::SeqCst);
        }
    }
}

fn now_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let clock = CLOCK.load(Ordering::SeqCst);
        if clock >= 0 {
            let mut t = thread_clock::Timespec { sec: 0, nsec: 0 };
            // SAFETY: clock is the id pthread_getcpuclockid gave, and t is a valid place for the time.
            if unsafe { thread_clock::clock_gettime(clock, &mut t) } == 0 {
                return (t.sec as u64) * 1_000_000_000 + t.nsec as u64 + 1;
            }
        }
    }
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

/// Gives one answer: `f` counts it into `counts` and `wrong`.
fn answer(counts: &Counts, wrong: &mut Vec<String>, f: impl FnOnce(&Counts, &mut Vec<String>)) {
    if !WORKER.load(Ordering::Relaxed) {
        f(counts, wrong);
        return;
    }
    if NEXT_ANSWER.fetch_add(1, Ordering::Relaxed) < FIRST_ANSWER.load(Ordering::Relaxed) {
        // Given by the child before this one, with whatever was counted before it.
        *WRITTEN.lock().unwrap() = (counts.get(), wrong.len());
        return;
    }
    if SPENT.load(Ordering::Relaxed) {
        // The record has had LIMITS_PER_RECORD answers at the limit: this one is unanswered, not asked.
        write_counts(counts, wrong, false);
        counts.limited.fetch_add(1, Ordering::Relaxed);
        write_counts(counts, wrong, true);
        return;
    }
    write_counts(counts, wrong, false);
    STARTED.store(now_ns(), Ordering::SeqCst);
    f(counts, wrong);
    STARTED.store(0, Ordering::SeqCst);
    write_counts(counts, wrong, true);
}

/// Writes what was counted since the last write: "D" and the six changes, then each new wrong case as "W"; with `done`,
/// the end of an answer, "." (or "+" alone for one right answer and nothing else).
fn write_counts(counts: &Counts, wrong: &[String], done: bool) {
    let mut written = WRITTEN.lock().unwrap();
    let now = counts.get();
    let delta: Vec<i64> = now.iter().zip(written.0).map(|(a, b)| a - b).collect();
    let mut out = std::io::stdout().lock();
    if done && delta == [1, 0, 0, 0, 0, 0] && wrong.len() == written.1 {
        writeln!(out, "+").unwrap();
    } else {
        if delta.iter().any(|&d| d != 0) {
            writeln!(out, "D {} {} {} {} {} {}", delta[0], delta[1], delta[2], delta[3], delta[4], delta[5]).unwrap();
        }
        for w in &wrong[written.1..] {
            writeln!(out, "W {w}").unwrap();
        }
        if done {
            writeln!(out, ".").unwrap();
        }
    }
    out.flush().unwrap();
    *written = (now, wrong.len());
}

/// The child's watchdog: ends the process, writing "L", when an answer has run for LIMIT.
fn watchdog() {
    std::thread::spawn(|| {
        let limit = LIMIT.as_nanos() as u64;
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let started = STARTED.load(Ordering::SeqCst);
            if started != 0 && now_ns().saturating_sub(started) > limit {
                end_at_limit();
            }
        }
    });
}

/// Writes "L" and ends the process at once: regress is in a match on the other thread, holding what it holds, so
/// nothing that could take a lock (the standard output's, the allocator's) is run.
#[cfg(unix)]
fn end_at_limit() -> ! {
    unsafe extern "C" {
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        fn _exit(status: i32) -> !;
    }
    // SAFETY: write and _exit take no lock and allocate nothing; the buffer is a static string.
    unsafe {
        write(1, b"\nL\n".as_ptr(), 3);
        _exit(0)
    }
}

#[cfg(not(unix))]
fn end_at_limit() -> ! {
    let _ = std::io::stdout().write_all(b"\nL\n");
    std::process::exit(0)
}

fn judge_record(name: &str, r: &Json, options: &Options, counts: &Counts, wrong: &mut Vec<String>) {
    let pattern = r.get("pattern").and_then(Json::as_string).unwrap();
    if name == EXEC {
        match options.engine {
            EngineKind::Corvus => judge_exec(r, &pattern, options, counts, wrong),
            EngineKind::Regress => judge_exec_regress(r, &pattern, options, counts, wrong),
        }
        return;
    }
    if name == "ill-formed-utf8.jsonl" {
        if options.utf8 {
            match options.engine {
                EngineKind::Corvus => judge_ill_formed::<Corvus>(r, &pattern, counts, wrong),
                EngineKind::Regress => judge_ill_formed::<Regress>(r, &pattern, counts, wrong),
            }
        }
        return;
    }
    if name == "captures.jsonl" {
        match options.engine {
            EngineKind::Corvus if options.utf16 => judge_captures(r, &pattern, counts, wrong),
            EngineKind::Corvus => {}
            EngineKind::Regress => judge_captures_regress(r, &pattern, options, counts, wrong),
        }
        return;
    }
    match options.engine {
        EngineKind::Corvus => judge_tests::<Corvus>(name, r, &pattern, options, counts, wrong),
        EngineKind::Regress => judge_tests::<Regress>(name, r, &pattern, options, counts, wrong),
    }
}

fn judge_tests<E: Engine>(
    name: &str,
    r: &Json,
    pattern: &str,
    options: &Options,
    counts: &Counts,
    wrong: &mut Vec<String>,
) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    for (suffix, answer_name, expected_valid) in modes(r) {
        let mode_flags = format!("{flags}{suffix}");
        let re = match std::panic::catch_unwind(|| E::compile(pattern, &mode_flags)) {
            Ok(Ok(re)) => re,
            Ok(Err((error, message))) => {
                if error {
                    counts.errors.fetch_add(1, Ordering::Relaxed);
                }
                if expected_valid {
                    counts.refused.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("REFUSED\t{name}\t/{shown}/{mode_flags}\t{}", first_line(&message)));
                }
                continue;
            }
            Err(_) => {
                counts.errors.fetch_add(1, Ordering::Relaxed);
                if expected_valid {
                    counts.refused.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("REFUSED\t{name}\t/{shown}/{mode_flags}\tERROR: panic"));
                }
                continue;
            }
        };
        if !expected_valid {
            counts.accepted.fetch_add(1, Ordering::Relaxed);
            wrong.push(format!("ACCEPTED-INVALID\t{name}\t/{shown}/{mode_flags}"));
            continue;
        }
        let Some(tests) = r.get("tests").and_then(Json::as_array) else { continue };
        for t in tests {
            let Some(expected) = t.get(answer_name).and_then(Json::as_bool) else { continue };
            let text = t.get("text").and_then(Json::as_units).unwrap();
            let check =
                |counts: &Counts, wrong: &mut Vec<String>, encoding: &str, result: Result<bool, String>| match result {
                    Ok(actual) if actual == expected => {
                        counts.right.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(_) => {
                        counts.wrong.fetch_add(1, Ordering::Relaxed);
                        wrong.push(format!(
                            "WRONG\t{name}\t/{shown}/{mode_flags}\t{}\t{encoding}\texpected {expected}",
                            escape(text)
                        ));
                    }
                    Err(why) if why == "panic" => {
                        counts.errors.fetch_add(1, Ordering::Relaxed);
                        wrong
                            .push(format!("ERROR\t{name}\t/{shown}/{mode_flags}\t{}\t{encoding}\tpanic", escape(text)));
                    }
                    Err(why) => {
                        counts.limited.fetch_add(1, Ordering::Relaxed);
                        wrong
                            .push(format!("LIMIT\t{name}\t/{shown}/{mode_flags}\t{}\t{encoding}\t{why}", escape(text)));
                    }
                };
            if options.utf16 {
                answer(counts, wrong, |counts, wrong| check(counts, wrong, "utf16", E::test_utf16(&re, text)));
            }
            if options.utf8 && !has_lone_surrogate(text) {
                let s = String::from_utf16(text).unwrap();
                answer(counts, wrong, |counts, wrong| check(counts, wrong, "utf8", E::test_utf8(&re, &s)));
            }
        }
    }
}

/// Judges a record of `ill-formed-utf8.jsonl`: under each grammar the pattern is valid in, each test's bytes are one
/// answer over UTF-8, and the right one is a refusal of them as UTF-8.
fn judge_ill_formed<E: Engine>(r: &Json, pattern: &str, counts: &Counts, wrong: &mut Vec<String>) {
    let name = "ill-formed-utf8.jsonl";
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    for (suffix, _, expected_valid) in modes(r) {
        let mode_flags = format!("{flags}{suffix}");
        let re = match std::panic::catch_unwind(|| E::compile(pattern, &mode_flags)) {
            Ok(Ok(re)) => re,
            _ => {
                if expected_valid {
                    counts.refused.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("REFUSED\t{name}\t/{shown}/{mode_flags}"));
                }
                continue;
            }
        };
        if !expected_valid {
            counts.accepted.fetch_add(1, Ordering::Relaxed);
            wrong.push(format!("ACCEPTED-INVALID\t{name}\t/{shown}/{mode_flags}"));
            continue;
        }
        for t in r.get("tests").and_then(Json::as_array).unwrap_or(&[]) {
            let hex = t.get("bytes").and_then(Json::as_string).unwrap();
            let bytes: Vec<u8> =
                (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            answer(counts, wrong, |counts, wrong| match std::str::from_utf8(&bytes) {
                Err(_) => {
                    counts.right.fetch_add(1, Ordering::Relaxed);
                }
                Ok(s) => {
                    // Not reached with the data, whose bytes are never UTF-8; were they, the engine would answer.
                    let got = E::test_utf8(&re, s);
                    counts.wrong.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("WRONG\t{name}\t/{shown}/{mode_flags}\t{hex}\tutf8\tread as UTF-8: {got:?}"));
                }
            });
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

// ---------------------------------------------------------------------------------------------------------------------
// captures.jsonl

/// The groups of an `exec` answer: `None` for no match, else the bounds of the match and of each group, `None` for
/// a group that did not take part.
type Exec = Option<Vec<Option<(usize, usize)>>>;

fn exec_of(e: &Json) -> Exec {
    let groups = e.as_array()?;
    Some(
        groups
            .iter()
            .map(|g| g.as_array().map(|b| (b[0].as_f64().unwrap() as usize, b[1].as_f64().unwrap() as usize)))
            .collect(),
    )
}

/// For each UTF-16 code unit index of a text (and the index past its end), the byte offset in UTF-8; the low half
/// of a pair is its character's third byte.
fn byte_offsets(s: &[u16]) -> Vec<usize> {
    let mut at = vec![0; s.len() + 1];
    let mut b = 0;
    let mut k = 0;
    while k < s.len() {
        at[k] = b;
        let c = s[k];
        if (0xD800..=0xDBFF).contains(&c) && k + 1 < s.len() && (0xDC00..=0xDFFF).contains(&s[k + 1]) {
            at[k + 1] = b + 2;
            b += 4;
            k += 2;
        } else {
            b += if c < 0x80 {
                1
            } else if c < 0x800 {
                2
            } else {
                3
            };
            k += 1;
        }
    }
    at[s.len()] = b;
    at
}

fn same_slots(expected: &Exec, slots: Option<&[Option<(usize, usize)>]>, map: &dyn Fn(usize) -> usize) -> bool {
    match (expected, slots) {
        (None, None) => true,
        (Some(e), Some(got)) => {
            e.len() == got.len()
                && e.iter().zip(got).all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some((s, t)), Some((gs, gt))) => map(*s) == *gs && map(*t) == *gt,
                    _ => false,
                })
        }
        _ => false,
    }
}

fn groups_of<H: corvus_regex_ecma262::Haystack + ?Sized>(
    caps: &corvus_regex_ecma262::Captures<'_, H>,
) -> Vec<Option<(usize, usize)>> {
    caps.iter().map(|m| m.map(|m| (m.start(), m.end()))).collect()
}

fn judge_captures(r: &Json, pattern: &str, counts: &Counts, wrong: &mut Vec<String>) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let valid = r.get("valid").unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return };
    let mut cache: HashMap<String, Option<Regex>> = HashMap::new();
    let mut make =
        |f: String| -> Option<Regex> { cache.entry(f.clone()).or_insert_with(|| Regex::new(pattern, &f).ok()).clone() };
    for t in tests {
        if t.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            continue;
        }
        let text = t.get("text").and_then(Json::as_units).unwrap();
        for mode in ["unicode", "legacy"] {
            if valid.get(mode).and_then(Json::as_bool) != Some(true) {
                continue;
            }
            let Some(a) = t.get(mode) else { continue };
            let mode_flags = if mode == "unicode" { format!("{flags}u") } else { flags.clone() };
            let where_ = format!("/{shown}/{mode_flags}\t{}", escape(text));
            let (Some(re), Some(g), Some(y)) =
                (make(mode_flags.clone()), make(format!("{mode_flags}g")), make(format!("{mode_flags}y")))
            else {
                counts.refused.fetch_add(1, Ordering::Relaxed);
                wrong.push(format!("REFUSED\tcaptures.jsonl\t{where_}"));
                continue;
            };
            let mut check = |what: &str, ok: bool| {
                if ok {
                    counts.right.fetch_add(1, Ordering::Relaxed);
                } else {
                    counts.wrong.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("WRONG\tcaptures.jsonl\t{where_}\t{what}"));
                }
            };
            let lone = has_lone_surrogate(text);
            let utf8 = if lone { String::new() } else { String::from_utf16(text).unwrap() };
            let byte_at = if lone { Vec::new() } else { byte_offsets(text) };
            let identity = |k: usize| k;
            let to_byte = |k: usize| byte_at[k];

            // exec
            let exec = exec_of(a.get("exec").unwrap());
            let got = re.captures_at_within_utf16(text, 0, LIMIT);
            let Ok(got) = got else {
                counts.limited.fetch_add(1, Ordering::Relaxed);
                wrong.push(format!("LIMIT\tcaptures.jsonl\t{where_}"));
                continue;
            };
            check("exec", same_slots(&exec, got.as_ref().map(groups_of).as_deref(), &identity));
            if !lone {
                let got8 = re.captures(&utf8).unwrap();
                check("exec-utf8", same_slots(&exec, got8.as_ref().map(groups_of).as_deref(), &to_byte));
            }

            // all
            let all: Vec<Exec> = a.get("all").and_then(Json::as_array).unwrap().iter().map(exec_of).collect();
            let judge_all = |found: &mut dyn Iterator<Item = (usize, usize)>, map: &dyn Fn(usize) -> usize| -> bool {
                let mut n = 0;
                for (from, to) in found {
                    if n >= all.len() {
                        // One match more than the data has: wrong, unless the data kept only the first twenty.
                        return all.len() == 20;
                    }
                    let Some(e) = &all[n] else { return false };
                    let (s, t) = e[0].unwrap();
                    if map(s) != from || map(t) != to {
                        return false;
                    }
                    n += 1;
                }
                !((all.len() < 20 && n != all.len()) || (all.len() == 20 && n < 20))
            };
            // The groups of each match too, over UTF-16.
            let mut all_ok = true;
            let mut n = 0;
            for caps in g.captures_iter_utf16(text).map(Result::unwrap) {
                if n >= all.len() {
                    all_ok &= all.len() == 20;
                    break;
                }
                if !same_slots(&all[n], Some(&groups_of(&caps)), &identity) {
                    all_ok = false;
                    break;
                }
                n += 1;
            }
            if (all.len() < 20 && n != all.len()) || (all.len() == 20 && n < 20) {
                all_ok = false;
            }
            check(
                "all",
                all_ok
                    && judge_all(
                        &mut g.find_iter_utf16(text).map(Result::unwrap).map(|m| (m.start(), m.end())),
                        &identity,
                    ),
            );
            if !lone {
                check(
                    "all-utf8",
                    judge_all(&mut g.find_iter(&utf8).map(Result::unwrap).map(|m| (m.start(), m.end())), &to_byte),
                );
            }

            // sticky at 1
            if !text.is_empty() {
                let sticky = exec_of(a.get("sticky").unwrap());
                let got = y.captures_at_utf16(text, 1).unwrap();
                check("sticky", same_slots(&sticky, got.as_ref().map(groups_of).as_deref(), &identity));
                if !lone && utf8.is_char_boundary(byte_at[1]) {
                    let got8 = y.captures_at(&utf8, byte_at[1]).unwrap();
                    check("sticky-utf8", same_slots(&sticky, got8.as_ref().map(groups_of).as_deref(), &to_byte));
                }
            }

            // replace
            let template: Vec<u16> = REPLACEMENT.encode_utf16().collect();
            let replace = a.get("replace").and_then(Json::as_units).unwrap();
            check("replace", *re.replace_utf16(text, &template).unwrap() == *replace);
            let replace_all = a.get("replaceAll").and_then(Json::as_units).unwrap();
            check("replaceAll", *g.replace_utf16(text, &template).unwrap() == *replace_all);
            if !lone {
                // Over UTF-8 a half character is written as U+FFFD, which is what a lossy conversion of V8's answer
                // writes for it.
                check("replace-utf8", re.replace(&utf8, REPLACEMENT).unwrap() == String::from_utf16_lossy(replace));
                check(
                    "replaceAll-utf8",
                    g.replace(&utf8, REPLACEMENT).unwrap() == String::from_utf16_lossy(replace_all),
                );
            }

            // split
            let same_split = |expected: &Json, got: Vec<Option<std::borrow::Cow<'_, [u16]>>>| -> bool {
                let e = expected.as_array().unwrap();
                e.len() == got.len()
                    && e.iter().zip(&got).all(|(e, g)| match (e, g) {
                        (Json::Null, None) => true,
                        (Json::String(s), Some(g)) => **s == **g,
                        _ => false,
                    })
            };
            check("split", same_split(a.get("split").unwrap(), re.split_utf16(text).unwrap()));
            check("split3", same_split(a.get("split3").unwrap(), re.splitn_utf16(text, 3).unwrap()));
        }
    }
}

// exec/test262.jsonl

/// The file of the exec calls of test262, in a directory of its own.
const EXEC: &str = "exec/test262.jsonl";

/// A file's name as the runner gives it: its own, or `exec/<name>` for one in the exec directory, whose name would
/// otherwise be that of a file of records (`test262.jsonl`).
fn data_name(path: &std::path::Path) -> String {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    if path.parent().and_then(|p| p.file_name()).is_some_and(|d| d == "exec") { format!("exec/{name}") } else { name }
}

/// A call's lastIndex. Past the end of the text exec gives null; test262 sets it as far as 2^53 - 1 and
/// Number.MAX_VALUE, so anything past the end reads as the text's length plus one.
fn last_index_of(t: &Json, len: usize) -> usize {
    let v = t.get("lastIndex").and_then(Json::as_f64).unwrap();
    if v > len as f64 { len + 1 } else { v as usize }
}

/// Whether a position is between the halves of a surrogate pair, where UTF-8 has no position.
fn inside_pair(s: &[u16], at: usize) -> bool {
    at > 0 && at < s.len() && (0xDC00..=0xDFFF).contains(&s[at]) && (0xD800..=0xDBFF).contains(&s[at - 1])
}

/// An engine's match for a call: the bounds of the match and of each group, and the named groups by name.
struct Got {
    groups: Vec<Option<(usize, usize)>>,
    names: Vec<(String, Option<(usize, usize)>)>,
}

/// Whether an engine's match is the call's answer, through `map` from the answer's UTF-16 positions to the engine's:
/// both null, or the same groups, the same named groups (present exactly when the pattern has names) and, with g or y,
/// a lastIndex after at the match's end.
fn same_exec(answer: &Json, got: Option<&Got>, carries: bool, map: &dyn Fn(usize) -> usize) -> bool {
    let Some(g) = got else { return answer.is_null() };
    if answer.is_null() || !same_slots(&exec_of(answer.get("captures").unwrap()), Some(&g.groups), map) {
        return false;
    }
    let names_ok = match answer.get("groups") {
        None => g.names.is_empty(),
        Some(Json::Object(members)) => {
            members.len() == g.names.len()
                && members.iter().all(|(name, v)| {
                    g.names.iter().find(|(n, _)| n == name).is_some_and(|(_, span)| match (v.as_array(), span) {
                        (None, None) => true,
                        (Some(b), Some((s, t))) => {
                            map(b[0].as_f64().unwrap() as usize) == *s && map(b[1].as_f64().unwrap() as usize) == *t
                        }
                        _ => false,
                    })
                })
        }
        Some(_) => false,
    };
    names_ok
        && (!carries
            || answer.get("lastIndex").and_then(Json::as_f64).map(|v| map(v as usize)) == g.groups[0].map(|(_, e)| e))
}

/// Counts one answer of a call.
fn count_exec(counts: &Counts, wrong: &mut Vec<String>, where_: &str, encoding: &str, outcome: Result<bool, String>) {
    match outcome {
        Ok(true) => {
            counts.right.fetch_add(1, Ordering::Relaxed);
        }
        Ok(false) => {
            counts.wrong.fetch_add(1, Ordering::Relaxed);
            wrong.push(format!("WRONG\t{EXEC}\t{where_}\t{encoding}"));
        }
        Err(e) => {
            counts.limited.fetch_add(1, Ordering::Relaxed);
            wrong.push(format!("LIMIT\t{EXEC}\t{where_}\t{encoding}\t{e}"));
        }
    }
}

/// Where a call is, for the wrong cases: the pattern with its flags, the text and the lastIndex as the data has it.
fn exec_where(shown: &str, flags: &str, text: &[u16], t: &Json) -> String {
    format!("/{shown}/{flags}\t{}\t{}", escape(text), t.get("lastIndex").and_then(Json::as_f64).unwrap())
}

/// Judges this crate on a record of `exec/test262.jsonl`: its own g, y and d flags, from each call's lastIndex.
fn judge_exec(r: &Json, pattern: &str, options: &Options, counts: &Counts, wrong: &mut Vec<String>) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    let Ok(re) = Regex::new(pattern, &flags) else {
        counts.refused.fetch_add(1, Ordering::Relaxed);
        wrong.push(format!("REFUSED\t{EXEC}\t/{shown}/{flags}"));
        return;
    };
    let carries = flags.contains('g') || flags.contains('y');
    let names: Vec<String> = re.group_names().map(str::to_string).collect();
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return };
    for t in tests {
        if t.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            continue;
        }
        let text = t.get("text").and_then(Json::as_units).unwrap();
        let start = if carries { last_index_of(t, text.len()) } else { 0 };
        let answer = t.get("answer").unwrap();
        let where_ = exec_where(&shown, &flags, text, t);
        if options.utf16 {
            let found = if start > text.len() { Ok(None) } else { re.captures_at_within_utf16(text, start, LIMIT) };
            let outcome = found.map_err(limit_name).map(|c| {
                let got = c.map(|c| Got {
                    groups: groups_of(&c),
                    names: names.iter().map(|n| (n.clone(), c.name(n).map(|m| (m.start(), m.end())))).collect(),
                });
                same_exec(answer, got.as_ref(), carries, &|k| k)
            });
            count_exec(counts, wrong, &where_, "utf16", outcome);
        }
        if options.utf8 && !has_lone_surrogate(text) && !inside_pair(text, start) {
            let utf8 = String::from_utf16(text).unwrap();
            let byte_at = byte_offsets(text);
            let found = if start > text.len() { Ok(None) } else { re.captures_at_within(&utf8, byte_at[start], LIMIT) };
            let outcome = found.map_err(limit_name).map(|c| {
                let got = c.map(|c| Got {
                    groups: groups_of(&c),
                    names: names.iter().map(|n| (n.clone(), c.name(n).map(|m| (m.start(), m.end())))).collect(),
                });
                same_exec(answer, got.as_ref(), carries, &|k| byte_at[k])
            });
            count_exec(counts, wrong, &where_, "utf8", outcome);
        }
    }
}

/// Judges regress on a record of `exec/test262.jsonl`. regress has no g, y or d flag: the pattern is compiled
/// without them, the search starts at lastIndex with g or y, and with y a match counts only where it starts there.
fn judge_exec_regress(r: &Json, pattern: &str, options: &Options, counts: &Counts, wrong: &mut Vec<String>) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    let compile: String = flags.chars().filter(|c| !"gyd".contains(*c)).collect();
    let unicode = flags.contains('u') || flags.contains('v');
    let Ok(Ok(re)) = std::panic::catch_unwind(|| regress::Regex::with_flags(pattern, compile.as_str())) else {
        counts.refused.fetch_add(1, Ordering::Relaxed);
        wrong.push(format!("REFUSED\t{EXEC}\t/{shown}/{flags}"));
        return;
    };
    let (carries, sticky) = (flags.contains('g') || flags.contains('y'), flags.contains('y'));
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return };
    for t in tests {
        if t.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            continue;
        }
        let text = t.get("text").and_then(Json::as_units).unwrap();
        let start = if carries { last_index_of(t, text.len()) } else { 0 };
        let answer = t.get("answer").unwrap();
        let where_ = exec_where(&shown, &flags, text, t);
        if options.utf16 {
            self::answer(counts, wrong, |counts, wrong| {
                let outcome = std::panic::catch_unwind(|| {
                    let got = regress_exec(&re, unicode, text, start)
                        .filter(|(g, _)| !sticky || g[0].is_some_and(|(s, _)| s == start))
                        .map(|(groups, names)| Got { groups, names });
                    same_exec(answer, got.as_ref(), carries, &|k| k)
                })
                .map_err(|_| "panic".to_string());
                count_exec(counts, wrong, &where_, "utf16", outcome);
            });
        }
        if options.utf8 && !has_lone_surrogate(text) && !inside_pair(text, start) {
            let utf8 = String::from_utf16(text).unwrap();
            let byte_at = byte_offsets(text);
            self::answer(counts, wrong, |counts, wrong| {
                let outcome = std::panic::catch_unwind(|| {
                    let from = if start > text.len() { None } else { Some(byte_at[start]) };
                    let got = from
                        .and_then(|from| re.find_from(&utf8, from).next().map(|m| (from, m)))
                        .filter(|(from, m)| !sticky || m.range.start == *from)
                        .map(|(_, m)| Got {
                            groups: (0..m.captures.len() + 1).map(|i| m.group(i).map(|r| (r.start, r.end))).collect(),
                            names: m
                                .named_groups()
                                .map(|(n, r)| (n.to_string(), r.map(|r| (r.start, r.end))))
                                .collect(),
                        });
                    same_exec(answer, got.as_ref(), carries, &|k| byte_at[k])
                })
                .map_err(|_| "panic".to_string());
                count_exec(counts, wrong, &where_, "utf8", outcome);
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The child processes that run regress.

/// Runs the records of a file from answer `first_answer` of record `first` on (an earlier answer of that record was
/// given by the child before; with `spent`, the record has had LIMITS_PER_RECORD answers at the limit), writing
/// `R <index>` before each record, `S` for one left out, its answers and counts as `answer` writes them, `E` after it,
/// and `DONE` at the end. The watchdog ends the process at an answer that passes LIMIT.
fn worker(file: &str, first: usize, first_answer: usize, spent: bool, encodings: &str) {
    let options = Options {
        utf16: matches!(encodings, "both" | "utf16"),
        utf8: matches!(encodings, "both" | "utf8"),
        engine: EngineKind::Regress,
    };
    WORKER.store(true, Ordering::Relaxed);
    no_core_dump();
    use_thread_clock();
    watchdog();
    let name = data_name(std::path::Path::new(file));
    let text = std::fs::read_to_string(file).unwrap();
    for (k, line) in text.lines().enumerate().skip(first) {
        {
            let mut out = std::io::stdout().lock();
            writeln!(out, "R {k}").unwrap();
            out.flush().unwrap();
        }
        let r = json::parse(line).unwrap();
        if r.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            println!("S");
            continue;
        }
        let counts = Counts::default();
        let mut wrong = Vec::new();
        *WRITTEN.lock().unwrap() = ([0; 6], 0);
        NEXT_ANSWER.store(0, Ordering::Relaxed);
        FIRST_ANSWER.store(if k == first { first_answer } else { 0 }, Ordering::Relaxed);
        SPENT.store(k == first && spent, Ordering::Relaxed);
        judge_record(&name, &r, &options, &counts, &mut wrong);
        write_counts(&counts, &wrong, false);
        println!("E");
    }
    println!("DONE");
}

/// Runs a file through child processes (`worker`), four at a time over its quarters. An answer that a child's watchdog
/// stops at LIMIT, or that a child dies in, is unanswered, and a new child goes on from the next answer; after
/// LIMITS_PER_RECORD of a record, the child counts the record's other answers unanswered without asking them.
fn supervise(
    file: &str,
    encodings: &str,
    counts: &Counts,
    wrong_cases: &Mutex<Vec<String>>,
    records: &AtomicI64,
    skipped: &AtomicI64,
) {
    let text = std::fs::read_to_string(file).unwrap();
    let lines = text.lines().count();
    let name = data_name(std::path::Path::new(file));
    let quarter = lines.div_ceil(4);
    std::thread::scope(|scope| {
        for part in 0..4 {
            let (from, to) = (part * quarter, ((part + 1) * quarter).min(lines));
            let name = &name;
            scope.spawn(move || {
                let (mut record, mut first_answer) = (from, 0);
                // The answers of `record` at the limit so far.
                let mut at_limit = 0;
                while record < to {
                    let exe = std::env::current_exe().unwrap();
                    let (record_text, answer_text) = (record.to_string(), first_answer.to_string());
                    let spent = if at_limit >= LIMITS_PER_RECORD { "1" } else { "0" };
                    let worker_args = ["--worker", file, &record_text, &answer_text, spent, encodings];
                    // regress has no limit on a match, and some patterns take memory until the process is stopped:
                    // the child's address space is capped (2 GB), and an answer whose child dies is unanswered.
                    let mut command = if cfg!(unix) {
                        let mut c = Command::new("sh");
                        c.arg("-c").arg("ulimit -v 2097152 && exec \"$0\" \"$@\"").arg(&exe).args(worker_args);
                        c
                    } else {
                        let mut c = Command::new(&exe);
                        c.args(worker_args);
                        c
                    };
                    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
                    let stdout = child.stdout.take().unwrap();
                    let (tx, rx) = std::sync::mpsc::channel::<String>();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stdout).lines() {
                            let Ok(line) = line else { break };
                            if tx.send(line).is_err() {
                                break;
                            }
                        }
                    });
                    // The record being judged and the number of its answers given so far.
                    let (mut current, mut given) = (record, first_answer);
                    let mut pending: Vec<String> = Vec::new();
                    let why = loop {
                        match rx.recv_timeout(SILENCE) {
                            Ok(line) => {
                                if let Some(k) = line.strip_prefix("R ") {
                                    current = k.parse().unwrap();
                                    given = if current == record { first_answer } else { 0 };
                                    if current >= to {
                                        break None;
                                    }
                                } else if line == "S" {
                                    skipped.fetch_add(1, Ordering::Relaxed);
                                } else if line == "+" {
                                    counts.right.fetch_add(1, Ordering::Relaxed);
                                    given += 1;
                                } else if line == "." {
                                    given += 1;
                                } else if let Some(d) = line.strip_prefix("D ") {
                                    let v: Vec<i64> = d.split(' ').map(|x| x.parse().unwrap()).collect();
                                    for (c, x) in [
                                        &counts.right,
                                        &counts.wrong,
                                        &counts.refused,
                                        &counts.accepted,
                                        &counts.limited,
                                        &counts.errors,
                                    ]
                                    .into_iter()
                                    .zip(v)
                                    {
                                        c.fetch_add(x, Ordering::Relaxed);
                                    }
                                } else if let Some(w) = line.strip_prefix("W ") {
                                    pending.push(w.to_string());
                                } else if line == "E" {
                                    records.fetch_add(1, Ordering::Relaxed);
                                } else if line == "L" {
                                    break Some(format!("no answer in {} s", LIMIT.as_secs()));
                                } else if line == "DONE" {
                                    break None;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                break Some(format!("nothing written in {} s", SILENCE.as_secs()));
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                // The child died (a stack overflow, or an abort at the cap on its memory).
                                break Some("the process died".to_string());
                            }
                        }
                    };
                    let _ = child.kill();
                    let _ = child.wait();
                    wrong_cases.lock().unwrap().append(&mut pending);
                    let Some(why) = why else { break };
                    // The answer being given is unanswered (the judging rule of the test data); the next child goes
                    // on from the one after it.
                    counts.limited.fetch_add(1, Ordering::Relaxed);
                    wrong_cases.lock().unwrap().push(format!(
                        "LIMIT\t{name}\trecord {} answer {}\t{why}",
                        current + 1,
                        given + 1
                    ));
                    if current != record {
                        at_limit = 0;
                    }
                    at_limit += 1;
                    if at_limit == LIMITS_PER_RECORD {
                        wrong_cases.lock().unwrap().push(format!(
                            "LIMIT\t{name}\trecord {}\tits answers after answer {} not asked: {LIMITS_PER_RECORD} at the limit",
                            current + 1,
                            given + 1
                        ));
                    }
                    (record, first_answer) = (current, given + 1);
                }
            });
        }
    });
}

/// One match of regress over UTF-16, as the groups' bounds (`None` for a group that did not take part).
type Groups16 = Vec<Option<(usize, usize)>>;

/// The named groups of one match of regress, by name.
type Names16 = Vec<(String, Option<(usize, usize)>)>;

/// regress over UTF-16: the first match at or after `from` (its UTF-16 search with the u or v flag, its UCS-2 search
/// without).
fn regress_exec(re: &regress::Regex, unicode: bool, text: &[u16], from: usize) -> Option<(Groups16, Names16)> {
    if from > text.len() {
        return None;
    }
    let m = if unicode { re.find_from_utf16(text, from).next() } else { re.find_from_ucs2(text, from).next() }?;
    let groups: Groups16 = (0..m.captures.len() + 1).map(|i| m.group(i).map(|r| (r.start, r.end))).collect();
    let names = m.named_groups().map(|(n, r)| (n.to_string(), r.map(|r| (r.start, r.end)))).collect();
    Some((groups, names))
}

/// After an empty match at `pos`, the next position: a code point with the u or v flag, a code unit without.
fn advance16(text: &[u16], pos: usize, unicode: bool) -> usize {
    if unicode
        && pos + 1 < text.len()
        && (0xD800..=0xDBFF).contains(&text[pos])
        && (0xDC00..=0xDFFF).contains(&text[pos + 1])
    {
        pos + 2
    } else {
        pos + 1
    }
}

/// GetSubstitution over UTF-16.
fn substitute16(out: &mut Vec<u16>, text: &[u16], groups: &Groups16, names: &Names16, template: &[u16]) {
    let (from, to) = groups[0].unwrap();
    let m = groups.len() - 1;
    let push = |out: &mut Vec<u16>, g: Option<(usize, usize)>| {
        if let Some((a, b)) = g {
            out.extend_from_slice(&text[a..b]);
        }
    };
    let digit = |k: usize| template.get(k).filter(|&&u| (0x30..=0x39).contains(&u)).map(|&u| (u - 0x30) as usize);
    let mut k = 0;
    while k < template.len() {
        if template[k] != u16::from(b'$') || k + 1 >= template.len() {
            out.push(template[k]);
            k += 1;
            continue;
        }
        match u8::try_from(template[k + 1]).unwrap_or(0) {
            b'$' => {
                out.push(template[k]);
                k += 2;
            }
            b'&' => {
                out.extend_from_slice(&text[from..to]);
                k += 2;
            }
            b'`' => {
                out.extend_from_slice(&text[..from]);
                k += 2;
            }
            b'\'' => {
                out.extend_from_slice(&text[to..]);
                k += 2;
            }
            b'0'..=b'9' => {
                let d1 = digit(k + 1).unwrap();
                if let Some(d2) = digit(k + 2)
                    && (1..=m).contains(&(d1 * 10 + d2))
                {
                    push(out, groups[d1 * 10 + d2]);
                    k += 3;
                } else if (1..=m).contains(&d1) {
                    push(out, groups[d1]);
                    k += 2;
                } else {
                    out.push(template[k]);
                    k += 1;
                }
            }
            b'<' => match template[k + 2..].iter().position(|&u| u == u16::from(b'>')) {
                Some(n) if !names.is_empty() => {
                    let name = String::from_utf16_lossy(&template[k + 2..k + 2 + n]);
                    if let Some((_, g)) = names.iter().find(|(nm, g)| *nm == name && g.is_some()) {
                        push(out, *g);
                    }
                    k += n + 3;
                }
                _ => {
                    out.push(template[k]);
                    k += 1;
                }
            },
            _ => {
                out.push(template[k]);
                k += 1;
            }
        }
    }
}

/// Judges regress on `captures.jsonl`: exec, the global search, the sticky flag (a match at 1, which regress has no
/// flag for, is its leftmost match from 1 when that starts at 1), replace, replaceAll, split and split3 over UTF-16,
/// and the same but the splits over UTF-8, each one answer. regress has none of them but exec, so they are written
/// over its exec as a JavaScript engine's String methods are (`regress_answers`). A panic is an error for the answer
/// it happens in.
fn judge_captures_regress(r: &Json, pattern: &str, options: &Options, counts: &Counts, wrong: &mut Vec<String>) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let valid = r.get("valid").unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return };
    for t in tests {
        if t.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            continue;
        }
        let text = t.get("text").and_then(Json::as_units).unwrap();
        let utf8 = if has_lone_surrogate(text) { None } else { Some(String::from_utf16(text).unwrap()) };
        let byte_at = byte_offsets(text);
        // The UTF-16 position of each byte offset that is one (a character's first byte, or the third of four, which
        // is the position between the halves of a pair).
        let mut unit_at = HashMap::new();
        for (k, &b) in byte_at.iter().enumerate() {
            unit_at.insert(b, k);
        }
        for mode in ["unicode", "legacy"] {
            if valid.get(mode).and_then(Json::as_bool) != Some(true) {
                continue;
            }
            let Some(a) = t.get(mode) else { continue };
            let unicode = mode == "unicode";
            let mode_flags = if unicode { format!("{flags}u") } else { flags.clone() };
            let where_ = format!("/{shown}/{mode_flags}\t{}", escape(text));
            let Ok(Ok(re)) = std::panic::catch_unwind(|| regress::Regex::with_flags(pattern, mode_flags.as_str()))
            else {
                counts.refused.fetch_add(1, Ordering::Relaxed);
                wrong.push(format!("REFUSED\tcaptures.jsonl\t{where_}"));
                continue;
            };
            // Each answer, its failure (a panic) an error.
            let mut ask = |encoding: &str, what: &str, judged: &dyn Fn() -> bool| {
                answer(counts, wrong, |counts, wrong| {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(judged)) {
                        Ok(true) => {
                            counts.right.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(false) => {
                            counts.wrong.fetch_add(1, Ordering::Relaxed);
                            wrong.push(format!("WRONG\tcaptures.jsonl\t{where_}\t{what}{encoding}"));
                        }
                        Err(_) => {
                            counts.errors.fetch_add(1, Ordering::Relaxed);
                            wrong.push(format!("ERROR\tcaptures.jsonl\t{where_}\t{what}{encoding}\tpanic"));
                        }
                    }
                });
            };
            if options.utf16 {
                let exec16 = |pos: usize| regress_exec(&re, unicode, text, pos);
                for (what, judged) in regress_answers(a, text, unicode, true, true, &exec16) {
                    ask("", what, &*judged);
                }
            }
            if let (true, Some(utf8)) = (options.utf8, &utf8) {
                // regress over a str from the byte offset of a UTF-16 position, its bounds taken back to UTF-16
                // positions; a bound at a byte that is no position makes the answer wrong, and ends the operation.
                let bad = std::cell::Cell::new(false);
                let exec8 = |pos: usize| -> Option<(Groups16, Names16)> {
                    if pos > text.len() || bad.get() {
                        return None;
                    }
                    let m = re.find_from(utf8, byte_at[pos]).next()?;
                    let unit = |r: Option<std::ops::Range<usize>>| -> Option<Option<(usize, usize)>> {
                        match r {
                            None => Some(None),
                            Some(r) => Some(Some((*unit_at.get(&r.start)?, *unit_at.get(&r.end)?))),
                        }
                    };
                    let groups: Option<Groups16> = (0..m.captures.len() + 1).map(|i| unit(m.group(i))).collect();
                    let names: Option<Names16> =
                        m.named_groups().map(|(n, r)| unit(r).map(|g| (n.to_string(), g))).collect();
                    match (groups, names) {
                        (Some(g), Some(n)) => Some((g, n)),
                        _ => {
                            bad.set(true);
                            None
                        }
                    }
                };
                for (what, judged) in regress_answers(a, text, unicode, false, !inside_pair(text, 1), &exec8) {
                    ask("-utf8", what, &|| {
                        bad.set(false);
                        judged() && !bad.get()
                    });
                }
            }
        }
    }
}

/// An answer of `captures.jsonl` written over regress's exec: its name and the judging of it.
type Asked<'a> = (&'static str, Box<dyn Fn() -> bool + 'a>);

/// The answers of one test of `captures.jsonl` under one grammar, from `exec` (the first match at or after a UTF-16
/// position): exec, the global search, the sticky search (when `sticky` and the text is not empty), replace and
/// replaceAll, and split and split3 when `splits`. Each is written over exec as ECMA-262 writes the String methods.
fn regress_answers<'a>(
    a: &'a Json,
    text: &'a [u16],
    unicode: bool,
    splits: bool,
    sticky: bool,
    exec: &'a dyn Fn(usize) -> Option<(Groups16, Names16)>,
) -> Vec<Asked<'a>> {
    let same = |e: &Exec, g: &Option<Groups16>| -> bool {
        match (e, g) {
            (None, None) => true,
            (Some(e), Some(g)) => e == g,
            _ => false,
        }
    };
    let mut out: Vec<Asked<'a>> = Vec::new();
    out.push(("exec", Box::new(move || same(&exec_of(a.get("exec").unwrap()), &exec(0).map(|x| x.0)))));
    // The global search.
    out.push((
        "all",
        Box::new(move || {
            let all: Vec<Exec> = a.get("all").and_then(Json::as_array).unwrap().iter().map(exec_of).collect();
            let mut found: Vec<Groups16> = Vec::new();
            let mut pos = 0;
            while found.len() < 20 {
                let Some((g, _)) = exec(pos) else { break };
                let (s, e) = g[0].unwrap();
                pos = if s == e { advance16(text, e, unicode) } else { e };
                found.push(g);
            }
            (if all.len() < 20 { found.len() == all.len() } else { found.len() >= 20 })
                && all.iter().zip(&found).all(|(e, g)| e.as_ref() == Some(g))
        }),
    ));
    if sticky && !text.is_empty() {
        out.push((
            "sticky",
            Box::new(move || {
                let got = exec(1).filter(|(g, _)| g[0].unwrap().0 == 1).map(|x| x.0);
                same(&exec_of(a.get("sticky").unwrap()), &got)
            }),
        ));
    }
    // replace and replaceAll.
    for (what, global) in [("replace", false), ("replaceAll", true)] {
        out.push((
            what,
            Box::new(move || {
                let template: Vec<u16> = REPLACEMENT.encode_utf16().collect();
                let mut out = Vec::new();
                let (mut last, mut pos) = (0, 0);
                while let Some((g, names)) = exec(pos) {
                    let (s, e) = g[0].unwrap();
                    out.extend_from_slice(&text[last..s]);
                    substitute16(&mut out, text, &g, &names, &template);
                    last = e;
                    if !global {
                        break;
                    }
                    pos = if s == e { advance16(text, e, unicode) } else { e };
                }
                out.extend_from_slice(&text[last..]);
                a.get(what).and_then(Json::as_units).unwrap() == out
            }),
        ));
    }
    if !splits {
        return out;
    }
    // split and split3.
    for (what, limit) in [("split", u32::MAX as usize), ("split3", 3)] {
        out.push((
            what,
            Box::new(move || {
                let mut out: Vec<Option<Vec<u16>>> = Vec::new();
                let size = text.len();
                if size == 0 {
                    if exec(0).is_none() {
                        out.push(Some(Vec::new()));
                    }
                } else {
                    let (mut p, mut q) = (0, 0);
                    'split: while q < size {
                        let Some((g, _)) = exec(q) else { break };
                        let (s, e) = g[0].unwrap();
                        if s >= size {
                            break;
                        }
                        if e.min(size) == p {
                            q = advance16(text, s, unicode);
                            continue;
                        }
                        out.push(Some(text[p..s].to_vec()));
                        if out.len() == limit {
                            break 'split;
                        }
                        p = e.min(size);
                        for c in &g[1..] {
                            out.push(c.map(|(x, y)| text[x..y].to_vec()));
                            if out.len() == limit {
                                break 'split;
                            }
                        }
                        q = p;
                    }
                    if out.len() < limit {
                        out.push(Some(text[p..].to_vec()));
                    }
                }
                let expected = a.get(what).and_then(Json::as_array).unwrap();
                expected.len() == out.len()
                    && expected.iter().zip(&out).all(|(e, g)| match (e, g) {
                        (Json::Null, None) => true,
                        (Json::String(x), Some(y)) => x == y,
                        _ => false,
                    })
            }),
        ));
    }
    out
}
