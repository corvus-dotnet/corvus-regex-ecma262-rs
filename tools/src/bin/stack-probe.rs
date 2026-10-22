//! Asks the engine about patterns built to overflow the stack: deep (groups, lookarounds, modifier groups, quantified
//! groups and the classes of the v flag, each nested to the limit, one past it and far past it), wide (many groups,
//! classes, alternatives, backreferences, set operations and string alternatives side by side) and long (the longest
//! pattern allowed, and one past it), in every grammar, through every member over UTF-8 and UTF-16.
//!
//!   stack-probe [--stacks 8M,1M,256K] [--jobs 4] [--filter text]
//!
//! Each probe runs in a process of its own on a thread with each of the stacks, since a stack overflow ends the
//! process and no caller can catch it. A probe is answered (compiled and matched), refused (not valid, or too large,
//! which a caller can catch), or a fault: the process died (a stack overflow is a SIGSEGV) or made no answer within
//! two minutes. Any fault makes the exit status 1.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use corvus_regex_ecma262::{Error, Regex};

/// Builds a probe's pattern and the text it is matched against.
type Build = Box<dyn Fn() -> (String, String) + Send>;

/// Builds a wide pattern of a number of one thing.
type Wide = fn(usize) -> String;

/// A probe, whose pattern and text are built only by the process that asks about it.
struct Probe {
    name: String,
    flags: &'static str,
    build: Build,
}

fn nest(open: &str, body: &str, close: &str, depth: usize) -> String {
    open.repeat(depth) + body + &close.repeat(depth)
}

fn probes() -> Vec<Probe> {
    let mut all = Vec::new();
    let mut add = |name: String, flags: &'static str, build: Build| {
        all.push(Probe { name, flags, build });
    };
    fn text(n: usize) -> String {
        "a".repeat(n.clamp(64, 1 << 20)) + "b"
    }
    let depths = [32, 33, 200, 1000, 1001, 5000, 200_000];
    let grammars = ["", "u", "v", "i", "iv"];

    // Deep: each kind of group nested.
    let groups: [(&str, &str, &str); 12] = [
        ("group", "(", ")"),
        ("non-capturing", "(?:", ")"),
        ("lookahead", "(?=", ")"),
        ("negative lookahead", "(?!", ")"),
        ("lookbehind", "(?<=", ")"),
        ("negative lookbehind", "(?<!", ")"),
        ("modifier i", "(?i:", ")"),
        ("modifier -i", "(?-i:", ")"),
        ("star", "(?:", ")*"),
        ("counted", "(?:", "){2}"),
        ("lazy plus", "(?:", ")+?"),
        ("alternative", "(?:b|", ")"),
    ];
    for (kind, open, close) in groups {
        for depth in depths {
            for flags in grammars {
                add(
                    format!("deep {kind} x{depth} /{flags}"),
                    flags,
                    Box::new(move || (nest(open, "a", close, depth), text(depth))),
                );
            }
        }
    }
    for depth in depths {
        for flags in grammars {
            add(
                format!("deep named x{depth} /{flags}"),
                flags,
                Box::new(move || {
                    let named: String = (0..depth).map(|i| format!("(?<g{i}>")).collect();
                    (named + "a" + &")".repeat(depth), text(depth))
                }),
            );
            add(
                format!("deep backreference x{depth} /{flags}"),
                flags,
                Box::new(move || (nest("(", "a", ")\\1", depth), text(depth))),
            );
        }
    }

    // Deep: the classes of the v flag nested, asked of every grammar (the others refuse them as not valid).
    let classes: [(&str, &str, &str, &str); 6] = [
        ("class", "[", "a", "]"),
        ("negated class", "[^", "a", "]"),
        ("difference", "[a--", "[b]", "]"),
        ("intersection", "[a&&", "[a]", "]"),
        ("string alternatives", "[\\q{ab|c}", "a", "]"),
        ("class in a group", "(?:[", "a", "])"),
    ];
    for (kind, open, body, close) in classes {
        for depth in depths {
            for flags in grammars {
                add(
                    format!("deep {kind} x{depth} /{flags}"),
                    flags,
                    Box::new(move || (nest(open, body, close, depth), text(depth))),
                );
            }
        }
    }
    for depth in depths {
        add(format!("deep [ unclosed x{depth} /v"), "v", Box::new(move || ("[".repeat(depth), text(64))));
        add(format!("deep ( unclosed x{depth} /v"), "v", Box::new(move || ("(".repeat(depth), text(64))));
    }

    // Wide: many of one thing side by side.
    let wide: [(&str, Wide); 16] = [
        ("alternatives", |w| "a|".repeat(w) + "b"),
        ("empty groups", |w| "()".repeat(w)),
        ("groups", |w| "(a)".repeat(w)),
        ("named groups", |w| (0..w).map(|i| format!("(?<g{i}>a)")).collect()),
        ("classes", |w| "[a]".repeat(w)),
        ("lookaheads", |w| "(?=a)".repeat(w) + "a"),
        ("backreferences", |w| "(a)".to_string() + &"\\1".repeat(w)),
        ("optional", |w| "a?".repeat(w)),
        ("optional groups", |w| "(?:a?)".repeat(w)),
        ("ranges in a class", |w| "[".to_string() + &"a-b".repeat(w) + "]"),
        ("properties", |w| "\\p{L}".repeat(w)),
        ("stars", |w| "a*".repeat(w)),
        ("alternatives in a star", |w| "(?:".to_string() + &"a|".repeat(w) + "b)*"),
        ("differences", |w| "[a".to_string() + &"--b".repeat(w) + "]"),
        ("intersections", |w| "[a".to_string() + &"&&a".repeat(w) + "]"),
        ("string alternatives", |w| "[\\q{".to_string() + &"a|".repeat(w) + "b}]"),
    ];
    for width in [1000, 100_000, 300_000] {
        for (kind, pattern) in wide {
            for flags in grammars {
                add(format!("wide {kind} x{width} /{flags}"), flags, Box::new(move || (pattern(width), text(width))));
            }
        }
    }

    // Long: the longest pattern allowed, and one past it.
    for length in [corvus_regex_ecma262::MAX_PATTERN_LENGTH, corvus_regex_ecma262::MAX_PATTERN_LENGTH + 1] {
        for flags in grammars {
            add(
                format!("long literal x{length} /{flags}"),
                flags,
                Box::new(move || ("a".repeat(length), text(length))),
            );
        }
    }
    all
}

/// Asks the engine everything about one probe; the answer is a line for the parent.
fn ask(probe: &Probe) -> String {
    let (pattern, text) = (probe.build)();
    if !Regex::is_valid(&pattern, probe.flags) {
        return match Regex::new(&pattern, probe.flags) {
            Err(Error::TooLarge(_)) => "refused too large".into(),
            Err(_) => "refused not valid".into(),
            Ok(_) => "FAULT is_valid refused a pattern new accepted".into(),
        };
    }
    let re = match Regex::new(&pattern, probe.flags) {
        Ok(re) => re,
        Err(Error::TooLarge(_)) => return "refused too large".into(),
        Err(e) => return format!("FAULT is_valid accepted a pattern new refused: {e}"),
    };
    let text = text.as_str();
    let text16: Vec<u16> = text.encode_utf16().collect();
    let limit = Duration::from_secs(5);
    let mut answers = Vec::new();
    answers.push(format!("{:?}", re.is_match_within(text, limit)));
    answers.push(format!("{:?}", re.is_match_within_utf16(&text16, limit)));
    answers.push(format!("{:?}", re.find_at_within(text, 0, limit).map(|m| m.map(|m| m.range()))));
    answers.push(format!("{:?}", re.captures_at_within(text, 0, limit).map(|c| c.map(|c| c.len()))));
    answers.push(format!("{:?}", re.captures_at_within_utf16(&text16, 0, limit).map(|c| c.map(|c| c.len()))));
    answers.push(re.find_iter(text).map(Result::unwrap).take(1000).count().to_string());
    answers.push(re.find_iter_utf16(&text16).map(Result::unwrap).take(1000).count().to_string());
    answers.push(re.captures_iter(text).map(Result::unwrap).take(100).count().to_string());
    let mut locations = re.capture_locations();
    answers.push(format!("{:?}", re.captures_read_at(&mut locations, text, 0).unwrap().map(|m| m.range())));
    answers.push(re.replace_all(text, "$1x").unwrap().len().to_string());
    answers.push(re.replace_all_utf16(&text16, &[b'x' as u16]).unwrap().len().to_string());
    answers.push(re.split(text).unwrap().len().to_string());
    answers.push(re.split_utf16(&text16).unwrap().len().to_string());
    format!("answered {}", answers.join(" "))
}

fn child(index: usize, stack: usize) {
    let probe = probes().swap_remove(index);
    let answer = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || ask(&probe))
        .expect("a thread for the probe")
        .join()
        .unwrap_or_else(|_| "FAULT panicked".into());
    println!("{answer}");
}

fn size(text: &str) -> usize {
    let (number, unit) = text.split_at(text.len() - 1);
    let number: usize = number.parse().expect("a stack size such as 256K or 8M");
    match unit {
        "K" => number << 10,
        "M" => number << 20,
        _ => panic!("a stack size such as 256K or 8M"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--child" {
        child(args[2].parse().unwrap(), args[3].parse().unwrap());
        return;
    }
    let (mut stacks, mut jobs, mut filter) = ("8M,1M,256K".to_string(), 4, String::new());
    let mut k = 1;
    while k + 1 < args.len() {
        match args[k].as_str() {
            "--stacks" => stacks = args[k + 1].clone(),
            "--jobs" => jobs = args[k + 1].parse().expect("a number of jobs"),
            "--filter" => filter = args[k + 1].clone(),
            other => panic!("unknown argument {other}"),
        }
        k += 2;
    }
    let stacks: Vec<(String, usize)> = stacks.split(',').map(|s| (s.to_string(), size(s))).collect();
    let all = probes();
    let work: Vec<(usize, usize)> = (0..all.len())
        .filter(|&i| all[i].name.contains(&filter))
        .flat_map(|i| (0..stacks.len()).map(move |s| (i, s)))
        .collect();
    let exe = std::env::current_exe().unwrap();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(i, s)) = work.get(n) else { break };
                    let mut process = Command::new(&exe)
                        .args(["--child", &i.to_string(), &stacks[s].1.to_string()])
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("a child process");
                    let deadline = Instant::now() + Duration::from_secs(120);
                    let outcome = loop {
                        if let Some(status) = process.try_wait().unwrap() {
                            let mut out = String::new();
                            std::io::Read::read_to_string(process.stdout.as_mut().unwrap(), &mut out).unwrap();
                            break if status.success() {
                                out.trim().to_string()
                            } else {
                                format!("FAULT died: {status}")
                            };
                        }
                        if Instant::now() > deadline {
                            let _ = process.kill();
                            let _ = process.wait();
                            break "FAULT no answer in two minutes".to_string();
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    };
                    results.lock().unwrap().push((i, s, outcome));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort();
    let (mut answered, mut refused, mut faults) = (0, 0, 0);
    let mut out = std::io::stdout().lock();
    for (i, s, outcome) in &results {
        if outcome.starts_with("answered") {
            answered += 1;
        } else if outcome.starts_with("refused") {
            refused += 1;
        } else {
            faults += 1;
            writeln!(out, "{} on {}: {outcome}", all[*i].name, stacks[*s].0).unwrap();
        }
    }
    writeln!(
        out,
        "{} probes on {} stacks: {answered} answered, {refused} refused, {faults} faults, in {:.0} s",
        work.len() / stacks.len(),
        stacks.len(),
        started.elapsed().as_secs_f64()
    )
    .unwrap();
    if faults > 0 {
        std::process::exit(1);
    }
}
