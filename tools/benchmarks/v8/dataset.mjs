// The ECMA-262 test data's test answers judged against the V8 of this Node, as tools/src/bin/dataset.rs judges an
// engine: each record under each grammar V8 found it valid in when the data was written (u, none, and v in the file
// that has it), every text as a JavaScript string (UTF-16), records marked v8_deviation left out. captures.jsonl is
// not judged here. With the Node the data was written with (24.11.0), every answer is V8's own, so this is the check
// that the data and the V8 on the page agree; with another, a difference is a place where its V8 differs.
//
//   node dataset.mjs <data directory>
//
// The last line has the counts in the shape of the Rust runner's: records, right, wrong, refused-valid,
// accepted-invalid, limit, errors.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const dir = process.argv[2];
let records = 0, skipped = 0, right = 0, wrong = 0, refused = 0, accepted = 0;
const start = Date.now();
for (const name of readdirSync(dir).filter(f => f.endsWith(".jsonl") && f !== "captures.jsonl").sort()) {
  let fr = 0, fw = 0, ff = 0, fa = 0;
  for (const line of readFileSync(join(dir, name), "utf8").split("\n")) {
    if (!line) continue;
    const r = JSON.parse(line);
    if (r.v8_deviation) { skipped++; continue; }
    records++;
    const modes = [["u", "unicode", r.valid.unicode], ["", "legacy", r.valid.legacy]];
    if ("unicode_sets" in r.valid) modes.push(["v", "unicode_sets", r.valid.unicode_sets]);
    for (const [suffix, answer, valid] of modes) {
      let re = null;
      try { re = new RegExp(r.pattern, r.flags + suffix); } catch { re = null; }
      if (re === null) { if (valid) ff++; continue; }
      if (!valid) { fa++; continue; }
      for (const t of r.tests ?? []) {
        if (!(answer in t)) continue;
        re.lastIndex = 0;
        if (re.test(t.text) === t[answer]) fr++; else fw++;
      }
    }
  }
  right += fr; wrong += fw; refused += ff; accepted += fa;
  console.log(`${name.padEnd(24)} right ${String(fr).padStart(10)}  wrong ${String(fw).padStart(6)}  refused-valid ${String(ff).padStart(4)}  accepted-invalid ${String(fa).padStart(4)}`);
}
console.log("");
console.log(`records ${records} (skipped ${skipped} deviations)  right ${right}  wrong ${wrong}  refused-valid ${refused}  accepted-invalid ${accepted}  limit 0  errors 0  in ${((Date.now() - start) / 1000).toFixed(1)}s  (${process.version}, V8 ${process.versions.v8})`);
