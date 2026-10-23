//! Runs an engine over the ECMA-262 regular expression test data (github.com/corvus-dotnet/ecma262-regex-test-data).
//!
//!   dataset <data directory> <wrong-cases output file> [file filter] [utf16|utf8|both] [--engine corvus|regress]
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
//! On `captures.jsonl` it is judged over UTF-16 alone (exec, the global search, the sticky flag, replace, replaceAll,
//! split and split3), since it has no UTF-8 interface for them. regress has no limit on a match, so each file is run
//! in child processes that report each record as they go, each with its address space capped at 2 GB; a record that
//! makes no progress for ten seconds, or whose child dies, is counted at the limit once for every answer it would have
//! had, and the child is started again after it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use corvus_regex_ecma262::{Error, MatchError, Regex};
use corvus_regex_ecma262_tools::json::{self, Json, escape};

const LIMIT: Duration = Duration::from_secs(2);
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
        // dataset --worker <file> <first record> <utf16|utf8|both>
        worker(&args[1], args[2].parse().unwrap(), &args[3]);
        return;
    }
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
    let total = Counts::default();
    let wrong_cases = Mutex::new(Vec::<String>::new());
    let (records, skipped) = (AtomicI64::new(0), AtomicI64::new(0));
    let clock = Instant::now();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
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
    println!();
    println!(
        "records {} (skipped {} deviations)  right {right}  wrong {wrong}  refused-valid {refused}  accepted-invalid {accepted}  limit {limited}  errors {errors}  in {:.1}s",
        records.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed),
        clock.elapsed().as_secs_f64()
    );
    std::process::exit(if wrong + refused + accepted + errors + limited == 0 { 0 } else { 1 });
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

fn judge_record(name: &str, r: &Json, options: &Options, counts: &Counts, wrong: &mut Vec<String>) {
    let pattern = r.get("pattern").and_then(Json::as_string).unwrap();
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
        if options.utf16 {
            match options.engine {
                EngineKind::Corvus => judge_captures(r, &pattern, counts, wrong),
                EngineKind::Regress => judge_captures_regress(r, &pattern, counts, wrong),
            }
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
            let mut check = |encoding: &str, result: Result<bool, String>| match result {
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
                    wrong.push(format!("ERROR\t{name}\t/{shown}/{mode_flags}\t{}\t{encoding}\tpanic", escape(text)));
                }
                Err(why) => {
                    counts.limited.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("LIMIT\t{name}\t/{shown}/{mode_flags}\t{}\t{encoding}\t{why}", escape(text)));
                }
            };
            if options.utf16 {
                check("utf16", E::test_utf16(&re, text));
            }
            if options.utf8 && !has_lone_surrogate(text) {
                let s = String::from_utf16(text).unwrap();
                check("utf8", E::test_utf8(&re, &s));
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
            match std::str::from_utf8(&bytes) {
                Err(_) => {
                    counts.right.fetch_add(1, Ordering::Relaxed);
                }
                Ok(s) => {
                    // Not reached with the data, whose bytes are never UTF-8; were they, the engine would answer.
                    let got = E::test_utf8(&re, s);
                    counts.wrong.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("WRONG\t{name}\t/{shown}/{mode_flags}\t{hex}\tutf8\tread as UTF-8: {got:?}"));
                }
            }
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

// ---------------------------------------------------------------------------------------------------------------------
// The child processes that run regress.

/// Runs the records of a file from `first` on, and writes `R <index>` before each and its counts and wrong cases
/// after it.
fn worker(file: &str, first: usize, encodings: &str) {
    let options = Options {
        utf16: matches!(encodings, "both" | "utf16"),
        utf8: matches!(encodings, "both" | "utf8"),
        engine: EngineKind::Regress,
    };
    let name = std::path::Path::new(file).file_name().unwrap().to_string_lossy().to_string();
    let text = std::fs::read_to_string(file).unwrap();
    let mut out = std::io::stdout().lock();
    for (k, line) in text.lines().enumerate().skip(first) {
        writeln!(out, "R {k}").unwrap();
        out.flush().unwrap();
        let r = json::parse(line).unwrap();
        if r.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            writeln!(out, "S").unwrap();
            continue;
        }
        let counts = Counts::default();
        let mut wrong = Vec::new();
        judge_record(&name, &r, &options, &counts, &mut wrong);
        let c = counts.get();
        writeln!(out, "C {} {} {} {} {} {}", c[0], c[1], c[2], c[3], c[4], c[5]).unwrap();
        for w in wrong {
            writeln!(out, "W {w}").unwrap();
        }
    }
    writeln!(out, "DONE").unwrap();
}

/// Runs a file through child processes, counting a record that makes no progress for ten seconds at the limit.
fn supervise(
    file: &str,
    encodings: &str,
    counts: &Counts,
    wrong_cases: &Mutex<Vec<String>>,
    records: &AtomicI64,
    skipped: &AtomicI64,
) {
    let text = std::fs::read_to_string(file).unwrap();
    let all_lines: Vec<&str> = text.lines().collect();
    let lines = all_lines.len();
    let utf8 = matches!(encodings, "both" | "utf8");
    let name = std::path::Path::new(file).file_name().unwrap().to_string_lossy().to_string();
    // Four children, each over a quarter of the file.
    let quarter = lines.div_ceil(4);
    std::thread::scope(|scope| {
        for part in 0..4 {
            let (from, to) = (part * quarter, ((part + 1) * quarter).min(lines));
            let name = &name;
            let all_lines = &all_lines;
            scope.spawn(move || {
                let mut next = from;
                while next < to {
                    let exe = std::env::current_exe().unwrap();
                    let next_text = next.to_string();
                    let worker_args = ["--worker", file, &next_text, encodings];
                    // regress has no limit on a match, and some patterns take memory until the process is stopped:
                    // the child's address space is capped (2 GB), and a child that dies counts its record at the
                    // limit.
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
                    let mut current = next;
                    let mut finished = false;
                    loop {
                        match rx.recv_timeout(Duration::from_secs(10)) {
                            Ok(line) => {
                                if let Some(k) = line.strip_prefix("R ") {
                                    current = k.parse().unwrap();
                                    if current >= to {
                                        finished = true;
                                        break;
                                    }
                                } else if line == "S" {
                                    skipped.fetch_add(1, Ordering::Relaxed);
                                } else if let Some(c) = line.strip_prefix("C ") {
                                    records.fetch_add(1, Ordering::Relaxed);
                                    let v: Vec<i64> = c.split(' ').map(|x| x.parse().unwrap()).collect();
                                    counts.right.fetch_add(v[0], Ordering::Relaxed);
                                    counts.wrong.fetch_add(v[1], Ordering::Relaxed);
                                    counts.refused.fetch_add(v[2], Ordering::Relaxed);
                                    counts.accepted.fetch_add(v[3], Ordering::Relaxed);
                                    counts.limited.fetch_add(v[4], Ordering::Relaxed);
                                    counts.errors.fetch_add(v[5], Ordering::Relaxed);
                                } else if let Some(w) = line.strip_prefix("W ") {
                                    wrong_cases.lock().unwrap().push(w.to_string());
                                } else if line == "DONE" {
                                    finished = true;
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                // The child died (a stack overflow, or an abort): the record it was on is an error.
                                break;
                            }
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    if finished {
                        break;
                    }
                    // The record `current` made no progress or killed the child: every answer it would have had is
                    // unanswered (the judging rule of the test data).
                    records.fetch_add(1, Ordering::Relaxed);
                    let answers = json::parse(all_lines[current]).map_or(1, |r| answers_of(name, &r, utf8).max(1));
                    counts.limited.fetch_add(answers, Ordering::Relaxed);
                    wrong_cases
                        .lock()
                        .unwrap()
                        .push(format!("LIMIT\t{name}\trecord {}\tno answer in 10 s, or the process died", current + 1));
                    next = current + 1;
                }
            });
        }
    });
}

/// The answers a record would have had from an engine that answers over UTF-16 and, with `utf8`, over UTF-8 (on
/// `captures.jsonl`, seven over UTF-16 for each test and grammar, as regress, which has no UTF-8 interface, gives them).
fn answers_of(name: &str, r: &Json, utf8: bool) -> i64 {
    if r.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
        return 0;
    }
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return 0 };
    let valid = r.get("valid").unwrap();
    if name == "ill-formed-utf8.jsonl" {
        let grammars =
            modes(r).iter().filter(|(_, mode, _)| valid.get(mode).and_then(Json::as_bool) == Some(true)).count();
        return if utf8 { (grammars * tests.len()) as i64 } else { 0 };
    }
    let mut n = 0;
    for t in tests {
        if t.get("v8_deviation").and_then(Json::as_bool) == Some(true) {
            continue;
        }
        let text = t.get("text").and_then(Json::as_units).unwrap_or(&[]);
        for (_, mode, _) in modes(r) {
            if valid.get(mode).and_then(Json::as_bool) != Some(true) || t.get(mode).is_none() {
                continue;
            }
            n += if name == "captures.jsonl" { 7 } else { 1 + i64::from(utf8 && !has_lone_surrogate(text)) };
        }
    }
    n
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

/// Judges regress on `captures.jsonl` over UTF-16 (it has no UTF-8 interface): exec, the global search, the sticky
/// flag (a match at 1, which regress has no flag for, is its leftmost match from 1 when that starts at 1), replace,
/// replaceAll, split and split3, each one answer. A panic is an error.
fn judge_captures_regress(r: &Json, pattern: &str, counts: &Counts, wrong: &mut Vec<String>) {
    let flags = r.get("flags").and_then(Json::as_string).unwrap();
    let valid = r.get("valid").unwrap();
    let shown = escape(&pattern.encode_utf16().collect::<Vec<u16>>());
    let Some(tests) = r.get("tests").and_then(Json::as_array) else { return };
    let template: Vec<u16> = REPLACEMENT.encode_utf16().collect();
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
            let unicode = mode == "unicode";
            let mode_flags = if unicode { format!("{flags}u") } else { flags.clone() };
            let where_ = format!("/{shown}/{mode_flags}\t{}", escape(text));
            let Ok(Ok(re)) = std::panic::catch_unwind(|| regress::Regex::with_flags(pattern, mode_flags.as_str()))
            else {
                counts.refused.fetch_add(1, Ordering::Relaxed);
                wrong.push(format!("REFUSED\tcaptures.jsonl\t{where_}"));
                continue;
            };
            let judged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut results: Vec<(&str, bool)> = Vec::new();
                let same = |e: &Exec, g: &Option<Groups16>| -> bool {
                    match (e, g) {
                        (None, None) => true,
                        (Some(e), Some(g)) => e == g,
                        _ => false,
                    }
                };
                let exec = regress_exec(&re, unicode, text, 0);
                results.push(("exec", same(&exec_of(a.get("exec").unwrap()), &exec.as_ref().map(|x| x.0.clone()))));
                // The global search.
                let all: Vec<Exec> = a.get("all").and_then(Json::as_array).unwrap().iter().map(exec_of).collect();
                let mut found: Vec<Groups16> = Vec::new();
                let mut pos = 0;
                while found.len() < 20 {
                    let Some((g, _)) = regress_exec(&re, unicode, text, pos) else { break };
                    let (s, e) = g[0].unwrap();
                    pos = if s == e { advance16(text, e, unicode) } else { e };
                    found.push(g);
                }
                let ok_all = if all.len() < 20 { found.len() == all.len() } else { found.len() >= 20 }
                    && all.iter().zip(&found).all(|(e, g)| e.as_ref() == Some(g));
                results.push(("all", ok_all));
                if !text.is_empty() {
                    let sticky = regress_exec(&re, unicode, text, 1).filter(|(g, _)| g[0].unwrap().0 == 1).map(|x| x.0);
                    results.push(("sticky", same(&exec_of(a.get("sticky").unwrap()), &sticky)));
                }
                // replace and replaceAll.
                for (what, global) in [("replace", false), ("replaceAll", true)] {
                    let mut out = Vec::new();
                    let (mut last, mut pos) = (0, 0);
                    while let Some((g, names)) = regress_exec(&re, unicode, text, pos) {
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
                    results.push((what, a.get(what).and_then(Json::as_units).unwrap() == out));
                }
                // split and split3.
                for (what, limit) in [("split", u32::MAX as usize), ("split3", 3)] {
                    let mut out: Vec<Option<Vec<u16>>> = Vec::new();
                    let size = text.len();
                    if size == 0 {
                        if regress_exec(&re, unicode, text, 0).is_none() {
                            out.push(Some(Vec::new()));
                        }
                    } else {
                        let (mut p, mut q) = (0, 0);
                        'split: while q < size {
                            let Some((g, _)) = regress_exec(&re, unicode, text, q) else { break };
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
                    let ok = expected.len() == out.len()
                        && expected.iter().zip(&out).all(|(e, g)| match (e, g) {
                            (Json::Null, None) => true,
                            (Json::String(x), Some(y)) => x == y,
                            _ => false,
                        });
                    results.push((what, ok));
                }
                results
            }));
            match judged {
                Ok(results) => {
                    for (what, ok) in results {
                        if ok {
                            counts.right.fetch_add(1, Ordering::Relaxed);
                        } else {
                            counts.wrong.fetch_add(1, Ordering::Relaxed);
                            wrong.push(format!("WRONG\tcaptures.jsonl\t{where_}\t{what}"));
                        }
                    }
                }
                Err(_) => {
                    counts.errors.fetch_add(1, Ordering::Relaxed);
                    wrong.push(format!("ERROR\tcaptures.jsonl\t{where_}\tpanic"));
                }
            }
        }
    }
}
