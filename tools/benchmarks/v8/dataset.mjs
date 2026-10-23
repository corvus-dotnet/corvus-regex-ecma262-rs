// The ECMA-262 test data's test answers judged against the V8 of this Node, as tools/src/bin/dataset.rs judges an
// engine: each record under each grammar V8 found it valid in when the data was written (u, none, and v in the file
// that has it), every text as a JavaScript string (UTF-16), records and tests marked v8_deviation left out.
// captures.jsonl is judged on exec, the global search, the sticky flag, replace, replaceAll, split and split3, each
// one answer, as the Rust runner judges an engine over UTF-16. With the Node the data was written with (24.22.0),
// every answer is V8's own, so this is the check that the data and the V8 on the page agree; with another, a
// difference is a place where its V8 differs.
//
// Each file is judged in a process of its own, as the data's verify.js checks it: V8 answers two hand-written
// patterns with a word boundary in a case-insensitive modifier group, (?i:\Bs) and (?i:\bk\b), otherwise once a
// process has compiled a great many patterns, and in one process for every file it gets them wrong.
//
//   node dataset.mjs <data directory>
//
// The last line has the counts in the shape of the Rust runner's: records, right, wrong, refused-valid,
// accepted-invalid, limit, errors.
import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = process.argv[2];
// node dataset.mjs <data directory> --file <name>: one file, its counts written as JSON (see the top).
const only = process.argv[3] === "--file" ? process.argv[4] : null;
const REPLACEMENT = "<$&|$1|$2|$10|$<a>|$$|$`|$'>";

function captures(m) {
  if (m === null) return null;
  const out = [[m.index, m.index + m[0].length]];
  if (m.indices) {
    for (let g = 1; g < m.length; g++) out.push(m.indices[g] === undefined ? null : [m.indices[g][0], m.indices[g][1]]);
  }
  return out;
}

// The answers of captures.jsonl V8 gives, as generate-captures.js of the data writes them.
function answers(pattern, flags, text) {
  const re = new RegExp(pattern, flags + "d");
  const g = new RegExp(pattern, flags + "dg");
  const y = new RegExp(pattern, flags + "dy");
  const a = { exec: captures(re.exec(text)), all: [] };
  for (const m of text.matchAll(g)) {
    a.all.push(captures(m));
    if (a.all.length >= 20) break;
  }
  if (text.length >= 1) {
    y.lastIndex = 1;
    a.sticky = captures(y.exec(text));
  }
  a.replace = text.replace(new RegExp(pattern, flags), REPLACEMENT);
  a.replaceAll = text.replace(new RegExp(pattern, flags + "g"), REPLACEMENT);
  a.split = text.split(new RegExp(pattern, flags)).map((s) => (s === undefined ? null : s));
  a.split3 = text.split(new RegExp(pattern, flags), 3).map((s) => (s === undefined ? null : s));
  return a;
}
// For a run with a limit of time on each record (tools/benchmarks/tregex-compliance.py, for an engine that does not
// answer some records): DATASET_FROM ("<file>\t<index>") starts at that record, DATASET_LIMITED (lines of
// "<file>\t<index>") names records not asked, whose answers are counted at the limit, and DATASET_PROGRESS=1 writes
// each record's counts to stderr as it is judged. The index of a record is that of its line among the file's lines
// that are not empty. V8's run sets none of them.
const from = process.env.DATASET_FROM ? process.env.DATASET_FROM.split("\t") : null;
const notAsked = new Set((process.env.DATASET_LIMITED ?? "").split("\n").filter(Boolean));
const progress = process.env.DATASET_PROGRESS === "1";
// The answers a record has, which a record not asked never gives: each test's answer where its grammar finds the
// pattern valid (seven of them in captures.jsonl, six for an empty text). A verdict is counted only where it is wrong,
// so a record not asked counts none.
function answersOf(name, r, modes) {
  let n = 0;
  for (const [, answer, valid] of modes) {
    if (!valid) continue;
    for (const t of r.tests ?? []) {
      if (t.v8_deviation || !(answer in t)) continue;
      n += name === "captures.jsonl" ? (t.text.length === 0 ? 6 : 7) : 1;
    }
  }
  return n;
}
// One file's counts.
function judgeFile(name) {
  let records = 0, skipped = 0, fr = 0, fw = 0, ff = 0, fa = 0, fl = 0;
  let index = -1;
  for (const line of readFileSync(join(dir, name), "utf8").split("\n")) {
    if (!line) continue;
    index++;
    if (from && name === from[0] && index < Number(from[1])) continue;
    const r = JSON.parse(line);
    if (r.v8_deviation) {
      skipped++;
      if (progress) process.stderr.write(`P\t${name}\t${index}\t0\t0\t0\t0\t0\t0\t1\n`);
      continue;
    }
    records++;
    const modes = [["u", "unicode", r.valid.unicode], ["", "legacy", r.valid.legacy]];
    if ("unicode_sets" in r.valid) modes.push(["v", "unicode_sets", r.valid.unicode_sets]);
    const before = [fr, fw, ff, fa, fl];
    if (notAsked.has(`${name}\t${index}`)) {
      fl += answersOf(name, r, modes);
      if (progress) process.stderr.write(`P\t${name}\t${index}\t0\t0\t0\t0\t${fl - before[4]}\t1\t0\n`);
      continue;
    }
    for (const [suffix, answer, valid] of modes) {
      let re = null;
      try { re = new RegExp(r.pattern, r.flags + suffix); } catch { re = null; }
      if (re === null) { if (valid) ff++; continue; }
      if (!valid) { fa++; continue; }
      for (const t of r.tests ?? []) {
        if (t.v8_deviation || !(answer in t)) continue;
        if (name === "captures.jsonl") {
          const got = answers(r.pattern, r.flags + suffix, t.text);
          for (const what of ["exec", "all", "sticky", "replace", "replaceAll", "split", "split3"]) {
            if (what === "sticky" && t.text.length === 0) continue;
            if (JSON.stringify(got[what]) === JSON.stringify(t[answer][what])) fr++; else fw++;
          }
          continue;
        }
        re.lastIndex = 0;
        if (re.test(t.text) === t[answer]) fr++; else fw++;
      }
    }
    if (progress) {
      const d = [fr, fw, ff, fa, fl].map((v, k) => v - before[k]);
      process.stderr.write(`P\t${name}\t${index}\t${d.join("\t")}\t1\t0\n`);
    }
  }
  return { records, skipped, right: fr, wrong: fw, refused: ff, accepted: fa, limited: fl };
}

if (only) {
  console.log(JSON.stringify(judgeFile(only)));
} else {
  let records = 0, skipped = 0, right = 0, wrong = 0, refused = 0, accepted = 0, limited = 0;
  const start = Date.now();
  for (const name of readdirSync(dir).filter(f => f.endsWith(".jsonl")).sort()) {
    if (from && name < from[0]) continue;
    // Progress goes to this process's stderr as it is written; the counts come back on stdout.
    const child = spawnSync(process.execPath, [...process.execArgv, fileURLToPath(import.meta.url), dir, "--file", name],
      { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 1 << 26 });
    if (child.status !== 0) throw new Error(`${name}: the process that judged it exited with ${child.status}`);
    const c = JSON.parse(child.stdout.trim().split("\n").pop());
    records += c.records; skipped += c.skipped;
    right += c.right; wrong += c.wrong; refused += c.refused; accepted += c.accepted; limited += c.limited;
    console.log(`${name.padEnd(24)} right ${String(c.right).padStart(10)}  wrong ${String(c.wrong).padStart(6)}  refused-valid ${String(c.refused).padStart(4)}  accepted-invalid ${String(c.accepted).padStart(4)}${c.limited ? `  limit ${c.limited}` : ""}`);
  }
  console.log("");
  console.log(`records ${records} (skipped ${skipped} deviations)  right ${right}  wrong ${wrong}  refused-valid ${refused}  accepted-invalid ${accepted}  limit ${limited}  errors 0  in ${((Date.now() - start) / 1000).toFixed(1)}s  (${process.version}, ${typeof Graal !== "undefined" ? "GraalJS " + Graal.versionGraalVM : "V8 " + process.versions.v8})`);
}
