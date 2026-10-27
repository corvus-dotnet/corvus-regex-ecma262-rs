//! The Unicode Character Database properties the engine reads, for each version of Unicode the crate carries (the
//! `ucd` package of the Go module, one instance for each version).
//!
//! Nothing in the engine takes Unicode properties from the Rust standard library or from the `regex` crate, because
//! those follow the toolchain and the version of the dependency. With its own tables the crate gives the same
//! answers whatever builds it, and the answers change only when the tables are generated again. A pattern is read
//! with the tables of the [`UnicodeVersion`](crate::UnicodeVersion) it was compiled for, [`Ucd`] here.
//!
//! The tables of a version are in `tables<major>.rs`, which `tools/unicode/gen-ucd-tables.ps1` writes from the
//! tables of the regress crate, and its properties of strings in `strings<major>.rs`. Each table is a sorted list of
//! inclusive ranges, the code points up to U+FFFF as 16-bit bounds and the rest as 32-bit bounds. A value that
//! follows from other values (a General_Category group such as L, the Assigned property, the Unknown script) has no
//! table of its own and is built when it is first asked for.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use super::charset::{CharSet, StringProperty};
use super::fold::FoldTable;
use super::{Rune, UnicodeVersion, strings13, strings17, strings18, tables13, tables17, tables18};

/// The last Unicode code point.
pub(crate) const MAX_RUNE: Rune = 0x10FFFF;

/// An immutable set of code points. `r16` holds the ranges that end at or below U+FFFF and `r32` the ranges that
/// start above it, each as inclusive pairs in ascending order. No two ranges of one list overlap or touch.
pub(crate) struct Table {
    pub(super) r16: &'static [u16],
    pub(super) r32: &'static [u32],
}

impl Table {
    /// Whether the table holds the code point.
    pub(crate) fn contains(&self, r: Rune) -> bool {
        if (r as u32) <= 0xFFFF {
            let (ranges, c) = (self.r16, r as u16);
            let (mut lo, mut hi) = (0, ranges.len() / 2);
            while lo < hi {
                let mid = (lo + hi) >> 1;
                if c > ranges[2 * mid + 1] {
                    lo = mid + 1;
                } else if c < ranges[2 * mid] {
                    hi = mid;
                } else {
                    return true;
                }
            }
            return false;
        }
        let (ranges, c) = (self.r32, r as u32);
        let (mut lo, mut hi) = (0, ranges.len() / 2);
        while lo < hi {
            let mid = (lo + hi) >> 1;
            if c > ranges[2 * mid + 1] {
                lo = mid + 1;
            } else if c < ranges[2 * mid] {
                hi = mid;
            } else {
                return true;
            }
        }
        false
    }

    /// Appends the ranges of the table to `dst` as inclusive pairs in ascending order. The pairs neither overlap
    /// nor touch.
    pub(crate) fn append_ranges(&self, dst: &mut Vec<Rune>) {
        dst.reserve(self.r16.len() + self.r32.len());
        dst.extend(self.r16.iter().map(|&c| c as Rune));
        let mut high = self.r32;
        // A range that crosses U+FFFF is stored in two parts.
        if !self.r16.is_empty() && !high.is_empty() && self.r16[self.r16.len() - 1] == 0xFFFF && high[0] == 0x10000 {
            let last = dst.len() - 1;
            dst[last] = high[1] as Rune;
            high = &high[2..];
        }
        dst.extend(high.iter().map(|&c| c as Rune));
    }

    /// The ranges of the table as inclusive pairs.
    pub(crate) fn ranges(&self) -> Vec<Rune> {
        let mut pairs = Vec::new();
        self.append_ranges(&mut pairs);
        pairs
    }
}

/// A table that is built, not generated: the ranges of a value that follows from other values.
pub(crate) struct Built(Vec<Rune>);

/// The ranges of a property: of a generated table, or of one that was built.
pub(crate) enum Ranges {
    Table(&'static Table),
    Built(&'static Built),
}

impl Ranges {
    /// The ranges as inclusive pairs in ascending order.
    pub(crate) fn pairs(&self) -> Vec<Rune> {
        match self {
            Ranges::Table(t) => t.ranges(),
            Ranges::Built(b) => b.0.clone(),
        }
    }
}

/// The table of the code points that any of the tables holds.
fn union(tables: &[&Table]) -> Built {
    let mut all = Vec::new();
    for t in tables {
        t.append_ranges(&mut all);
    }
    let mut order: Vec<usize> = (0..all.len() / 2).collect();
    order.sort_by_key(|&i| all[2 * i]);
    let mut merged: Vec<Rune> = Vec::with_capacity(all.len());
    for i in order {
        let (lo, hi) = (all[2 * i], all[2 * i + 1]);
        if let Some(last) = merged.last_mut()
            && lo <= *last + 1
        {
            if hi > *last {
                *last = hi;
            }
            continue;
        }
        merged.push(lo);
        merged.push(hi);
    }
    Built(merged)
}

/// The table of the code points that the table does not hold.
fn complement(t: &Table) -> Built {
    let pairs = t.ranges();
    let mut inverse = Vec::with_capacity(pairs.len() + 2);
    let mut next: Rune = 0;
    for &[lo, hi] in pairs.as_chunks::<2>().0 {
        if lo > next {
            inverse.push(next);
            inverse.push(lo - 1);
        }
        next = hi + 1;
    }
    if next <= MAX_RUNE {
        inverse.push(next);
        inverse.push(MAX_RUNE);
    }
    Built(inverse)
}

/// The table of the Script value Katakana_Or_Hiragana (Hrkt). PropertyValueAliases.txt lists the value, and no code
/// point has it, in Script or in Script_Extensions. Scripts.txt has no line for it, so the generated tables do not
/// have it.
static KATAKANA_OR_HIRAGANA: Table = Table { r16: &[], r32: &[] };

/// The generated data of one version of Unicode: its lookups by name and its case mappings.
pub(crate) struct Data {
    pub(crate) version: UnicodeVersion,
    category_table: fn(&str) -> Option<&'static Table>,
    binary_table: fn(&str) -> Option<&'static Table>,
    script_table: fn(&str) -> Option<&'static Table>,
    script_extensions_table: fn(&str) -> Option<&'static Table>,
    #[cfg_attr(not(test), allow(dead_code))]
    script_names: &'static [&'static [&'static str]],
    #[cfg_attr(not(test), allow(dead_code))]
    binary_names: &'static [&'static str],
    case_folds: &'static [CaseRange],
    upper_cases: &'static [CaseRange],
    /// The properties of strings of the v flag (see `strings<major>.rs`).
    pub(crate) strings: &'static [(&'static str, &'static str, &'static str); 6],
}

/// The Unicode of one version: the generated data, and what is built from it on first use and then kept.
pub(crate) struct Ucd {
    pub(crate) data: Data,
    // The General_Category values that are groups of other values, the Assigned property and the Unknown script. The
    // generator of the tables checks each definition against their source.
    letter: OnceLock<Built>,
    cased_letter: OnceLock<Built>,
    mark: OnceLock<Built>,
    number: OnceLock<Built>,
    punctuation: OnceLock<Built>,
    symbol: OnceLock<Built>,
    separator: OnceLock<Built>,
    other: OnceLock<Built>,
    /// The code points that have a General_Category other than Cn.
    assigned: OnceLock<Built>,
    /// The code points that belong to no script. They are the unassigned, private use and surrogate code points.
    unknown_script: OnceLock<Built>,
    pub(crate) id_start: OnceLock<&'static Table>,
    pub(crate) id_continue: OnceLock<&'static Table>,
    /// The equivalence classes of Canonicalize under the u flag grammar and under the grammar of no flag (`fold.rs`).
    pub(crate) unicode_fold: OnceLock<FoldTable>,
    pub(crate) legacy_fold: OnceLock<FoldTable>,
    /// The properties of strings, read from `data.strings` (`charset.rs`).
    pub(crate) string_properties: OnceLock<Vec<StringProperty>>,
    /// The sets already built for property expressions (`charset.rs`).
    pub(crate) property_cache: LazyLock<Mutex<HashMap<String, Arc<CharSet>>>>,
}

macro_rules! ucd {
    ($version:expr, $tables:ident, $strings:ident) => {
        Ucd {
            data: Data {
                version: $version,
                category_table: $tables::category_table,
                binary_table: $tables::binary_table,
                script_table: $tables::script_table,
                script_extensions_table: $tables::script_extensions_table,
                script_names: $tables::SCRIPT_NAMES,
                binary_names: $tables::BINARY_NAMES,
                case_folds: $tables::CASE_FOLDS,
                upper_cases: $tables::UPPER_CASES,
                strings: &$strings::PROPERTIES,
            },
            letter: OnceLock::new(),
            cased_letter: OnceLock::new(),
            mark: OnceLock::new(),
            number: OnceLock::new(),
            punctuation: OnceLock::new(),
            symbol: OnceLock::new(),
            separator: OnceLock::new(),
            other: OnceLock::new(),
            assigned: OnceLock::new(),
            unknown_script: OnceLock::new(),
            id_start: OnceLock::new(),
            id_continue: OnceLock::new(),
            unicode_fold: OnceLock::new(),
            legacy_fold: OnceLock::new(),
            string_properties: OnceLock::new(),
            property_cache: LazyLock::new(|| Mutex::new(HashMap::new())),
        }
    };
}

static UCD13: Ucd = ucd!(UnicodeVersion::V13, tables13, strings13);
static UCD17: Ucd = ucd!(UnicodeVersion::V17, tables17, strings17);
static UCD18: Ucd = ucd!(UnicodeVersion::V18, tables18, strings18);

/// The Unicode of a version, resolved (see [`UnicodeVersion::resolve`]).
pub(crate) fn ucd(version: UnicodeVersion) -> &'static Ucd {
    match version.resolve() {
        UnicodeVersion::V13 => &UCD13,
        UnicodeVersion::V18 => &UCD18,
        _ => &UCD17,
    }
}

/// The Unicode of a version by its major number, as a backreference instruction carries it. Any number but 13 and 18
/// is 17, which is also what an instruction of no version (0) means.
#[inline]
pub(crate) fn by_major(major: i32) -> &'static Ucd {
    match major {
        13 => &UCD13,
        18 => &UCD18,
        _ => &UCD17,
    }
}

impl Ucd {
    fn gc(&'static self, name: &str) -> &'static Table {
        (self.data.category_table)(name).expect("the tables have every General_Category value")
    }

    fn built(&'static self, cell: &'static OnceLock<Built>, names: &[&str]) -> Ranges {
        Ranges::Built(cell.get_or_init(|| union(&names.iter().map(|n| self.gc(n)).collect::<Vec<_>>())))
    }

    /// The table of a General_Category value by its short name, such as "Lu" or "L". `None` for any other name.
    pub(crate) fn category(&'static self, name: &str) -> Option<Ranges> {
        Some(match name {
            "L" => self.built(&self.letter, &["Lu", "Ll", "Lt", "Lm", "Lo"]),
            "LC" => self.built(&self.cased_letter, &["Lu", "Ll", "Lt"]),
            "M" => self.built(&self.mark, &["Mn", "Mc", "Me"]),
            "N" => self.built(&self.number, &["Nd", "Nl", "No"]),
            "P" => self.built(&self.punctuation, &["Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po"]),
            "S" => self.built(&self.symbol, &["Sm", "Sc", "Sk", "So"]),
            "Z" => self.built(&self.separator, &["Zs", "Zl", "Zp"]),
            "C" => self.built(&self.other, &["Cc", "Cf", "Cs", "Co", "Cn"]),
            _ => return (self.data.category_table)(name).map(Ranges::Table),
        })
    }

    /// The table of a binary property by its canonical name, such as "Alphabetic" or "White_Space". `None` for any
    /// other name. ASCII and Any, which ECMA-262 adds to the properties of Unicode, are not here.
    pub(crate) fn binary(&'static self, name: &str) -> Option<Ranges> {
        if name == "Assigned" {
            return Some(Ranges::Built(self.assigned.get_or_init(|| complement(self.gc("Cn")))));
        }
        (self.data.binary_table)(name).map(Ranges::Table)
    }

    /// The generated table of a binary property, for the callers that test one code point.
    pub(crate) fn binary_table(&'static self, name: &str) -> Option<&'static Table> {
        (self.data.binary_table)(name)
    }

    fn unknown_script(&'static self) -> Ranges {
        self.built(&self.unknown_script, &["Cn", "Co", "Cs"])
    }

    /// The table of a Script value by its long name or an alias, such as "Greek" or "Grek". `None` for any other
    /// name.
    pub(crate) fn script(&'static self, name: &str) -> Option<Ranges> {
        match name {
            "Unknown" | "Zzzz" => Some(self.unknown_script()),
            "Katakana_Or_Hiragana" | "Hrkt" => Some(Ranges::Table(&KATAKANA_OR_HIRAGANA)),
            _ => (self.data.script_table)(name).map(Ranges::Table),
        }
    }

    /// The table of a Script_Extensions value by the long name of the script or an alias. `None` for any other name.
    pub(crate) fn script_extensions(&'static self, name: &str) -> Option<Ranges> {
        match name {
            "Unknown" | "Zzzz" => Some(self.unknown_script()),
            "Katakana_Or_Hiragana" | "Hrkt" => Some(Ranges::Table(&KATAKANA_OR_HIRAGANA)),
            _ => (self.data.script_extensions_table)(name).map(Ranges::Table),
        }
    }

    /// The names of every script. Each element is the long name of one script followed by its aliases.
    #[cfg(test)]
    pub(crate) fn script_names(&self) -> &'static [&'static [&'static str]] {
        self.data.script_names
    }

    /// The canonical name of every binary property that has a table, which is every name [`Ucd::binary`] knows but
    /// Assigned.
    #[cfg(test)]
    pub(crate) fn binary_names(&self) -> &'static [&'static str] {
        self.data.binary_names
    }

    /// The simple case folding of a code point (the C and S rows of CaseFolding.txt), which is the code point itself
    /// when it has none. Two code points are equal ignoring case when `fold` gives the same result for both.
    pub(crate) fn fold(&self, r: Rune) -> Rune {
        if r < 0x80 {
            if (b'A' as Rune..=b'Z' as Rune).contains(&r) {
                return r + (b'a' - b'A') as Rune;
            }
            return r;
        }
        map_case(self.data.case_folds, r)
    }

    /// The simple uppercase mapping of a code point, which is the code point itself when it has none.
    pub(crate) fn to_upper(&self, r: Rune) -> Rune {
        if r < 0x80 {
            if (b'a' as Rune..=b'z' as Rune).contains(&r) {
                return r - (b'a' - b'A') as Rune;
            }
            return r;
        }
        map_case(self.data.upper_cases, r)
    }

    /// Every code point that [`Ucd::fold`] changes, in ascending order.
    pub(crate) fn folding(&self) -> Vec<Rune> {
        let mut dst = Vec::new();
        for row in self.data.case_folds {
            for offset in 0..row.length as Rune {
                if offset & row.mask as Rune == 0 && row.delta != 0 {
                    dst.push(row.first + offset);
                }
            }
        }
        dst
    }
}

/// Maps some of the code points `first`, `first + 1`, ... `first + length - 1` to themselves plus `delta`. The ones
/// it maps are those whose offset from `first` has none of the bits of `mask`, so a mask of 1 takes every other code
/// point.
pub(crate) struct CaseRange {
    pub(super) first: Rune,
    pub(super) delta: i32,
    pub(super) length: u16,
    pub(super) mask: u16,
}

fn map_case(table: &[CaseRange], r: Rune) -> Rune {
    let (mut lo, mut hi) = (0, table.len());
    while lo < hi {
        let mid = (lo + hi) >> 1;
        let row = &table[mid];
        if r < row.first {
            hi = mid;
        } else {
            let offset = r - row.first;
            if offset >= row.length as Rune {
                lo = mid + 1;
            } else if offset & row.mask as Rune == 0 {
                return r + row.delta;
            } else {
                return r;
            }
        }
    }
    r
}
