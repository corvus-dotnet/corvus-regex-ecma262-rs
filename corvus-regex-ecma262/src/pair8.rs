//! A vectorised search for a few literals by two of their byte positions, each of which holds one or two values
//! across the literals: PCRE2's search before a match (`fast_forward_char_pair_simd` in its JIT compiler), which finds
//! `Sher[a-z]+|Hol[a-z]+` by an `S` or an `H` with an `e` or an `l` two bytes on, and a case-insensitive
//! `Holmes` by both cases of two of its letters. A position whose two values differ in one bit (the cases of an ASCII
//! letter, the lead bytes of the cases of a Cyrillic letter) is compared once with that bit set; any other pair of
//! values is compared twice. Each candidate is compared with the literals inside the kernel, as the pair search of
//! `memchr::memmem` does for one literal.

/// How common each byte is in text, from 0 (rare) to 255: the table `memchr::memmem` ranks a needle's bytes by
/// (`memchr` 2.8, `arch/all/packedpair/default_rank.rs`; MIT or Unlicense).
const RANK: [u8; 256] = [
    55, 52, 51, 50, 49, 48, 47, 46, 45, 103, 242, 66, 67, 229, 44, 43, 42, 41, 40, 39, 38, 37, 36, 35, 34, 33, 56, 32,
    31, 30, 29, 28, 255, 148, 164, 149, 136, 160, 155, 173, 221, 222, 134, 122, 232, 202, 215, 224, 208, 220, 204, 187,
    183, 179, 177, 168, 178, 200, 226, 195, 154, 184, 174, 126, 120, 191, 157, 194, 170, 189, 162, 161, 150, 193, 142,
    137, 171, 176, 185, 167, 186, 112, 175, 192, 188, 156, 140, 143, 123, 133, 128, 147, 138, 146, 114, 223, 151, 249,
    216, 238, 236, 253, 227, 218, 230, 247, 135, 180, 241, 233, 246, 244, 231, 139, 245, 243, 251, 235, 201, 196, 240,
    214, 152, 182, 205, 181, 127, 27, 212, 211, 210, 213, 228, 197, 169, 159, 131, 172, 105, 80, 98, 96, 97, 81, 207,
    145, 116, 115, 144, 130, 153, 121, 107, 132, 109, 110, 124, 111, 82, 108, 118, 141, 113, 129, 119, 125, 165, 117,
    92, 106, 83, 72, 99, 93, 65, 79, 166, 237, 163, 199, 190, 225, 209, 203, 198, 217, 219, 206, 234, 248, 158, 239,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
];

/// The most bytes of the literals looked at before they are compared one by one.
const CHECKED: usize = 16;

/// The search for a few literals by two byte positions. Only x86-64 has its kernels; elsewhere the fingerprint search,
/// which has vector kernels there, is kept.
#[derive(Clone, Debug)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) struct PairSearch {
    literals: Vec<Vec<u8>>,
    /// The fewest bytes of a literal.
    shortest: usize,
    /// The two positions and the test of each: a byte `x` passes where `x | mask == one` or `x == other`.
    at: [usize; 2],
    mask: [u8; 2],
    one: [u8; 2],
    other: [u8; 2],
    /// For each of the first bytes of the literals, up to `CHECKED`, the values they hold there.
    sets: Vec<[bool; 256]>,
    /// Whether the literals are every sequence of the values `sets` holds, all of one length (a word's cases written
    /// out): a candidate whose bytes pass `sets` is then a literal, with nothing more to compare.
    every: bool,
}

impl PairSearch {
    /// The search for the literals (two or more, of at least two bytes each, in the pattern's order), or `None` where
    /// no two of their positions hold at most two values each.
    pub(crate) fn new(literals: &[Vec<u8>]) -> Option<PairSearch> {
        if literals.len() < 2 || cfg!(not(target_arch = "x86_64")) {
            return None;
        }
        let shortest = literals.iter().map(Vec::len).min()?;
        if shortest < 2 {
            return None;
        }
        let values = |k: usize| -> Vec<u8> {
            let mut v: Vec<u8> = literals.iter().map(|l| l[k]).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        // Each position of at most two values, with how common its commoner value is.
        let candidates: Vec<(usize, Vec<u8>, u32)> = (0..shortest)
            .map(|k| (k, values(k)))
            .filter(|(_, v)| v.len() <= 2)
            .map(|(k, v)| {
                let rank = v.iter().map(|&b| u32::from(RANK[b as usize])).max().unwrap_or(0);
                (k, v, rank)
            })
            .collect();
        // The pair of the rarest positions, their values apart where any pair has them apart (a run of one byte
        // would pass both otherwise), as PCRE2 chooses.
        let mut best: Option<(u32, usize, usize)> = None;
        for apart in [true, false] {
            for (x, (i, vi, ri)) in candidates.iter().enumerate() {
                for (j, vj, rj) in &candidates[x + 1..] {
                    if apart && vi.iter().any(|b| vj.contains(b)) {
                        continue;
                    }
                    let cost = ri + rj;
                    if best.is_none_or(|(c, _, _)| cost < c) {
                        best = Some((cost, *i, *j));
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        let (cost, i, j) = best?;
        // Where the two positions hold common bytes the candidates are too many, and the fingerprint search, which
        // looks at three bytes, is faster: measured over the Industry workloads, `Tom|river` (`T` or `r`, then `m` or
        // `v`, a cost of 478) took 1.24 times its time and `[A-Za-z]awyer|[A-Za-z]inn` (`a` or `i`, then `w` or `n`,
        // 495) 4.3 times, where `Sherlock|Holmes` (436) took 0.52. Many literals slow the fingerprint search and not
        // this one, so a word's cases written out (`the` under the i flag, eight literals, 481) take it up to more.
        let bound = if literals.len() >= 8 { 490 } else { 470 };
        if cost > bound {
            return None;
        }
        let test = |k: usize| -> (u8, u8, u8) {
            let v = values(k);
            let (a, b) = (v[0], *v.last().expect("a value"));
            let bit = a ^ b;
            if bit.is_power_of_two() { (bit, a | bit, a | bit) } else { (0, a, b) }
        };
        let (ti, tj) = (test(i), test(j));
        let sets = (0..shortest.min(CHECKED))
            .map(|k| {
                let mut set = [false; 256];
                for l in literals {
                    set[l[k] as usize] = true;
                }
                set
            })
            .collect();
        let every = shortest <= CHECKED
            && literals.iter().all(|l| l.len() == shortest)
            && (0..shortest).try_fold(1usize, |n, k| n.checked_mul(values(k).len())) == Some(literals.len());
        Some(PairSearch {
            every,
            literals: literals.to_vec(),
            shortest,
            at: [i, j],
            mask: [ti.0, tj.0],
            one: [ti.1, tj.1],
            other: [ti.2, tj.2],
            sets,
        })
    }

    /// Where the literal that starts at `i` ends (the first in the pattern's order), if one does.
    #[inline(always)]
    fn literal_at(&self, haystack: &[u8], i: usize) -> Option<usize> {
        let rest = &haystack[i..];
        if !self.sets.iter().zip(rest).all(|(set, &b)| set[b as usize]) {
            return None;
        }
        if self.every {
            return Some(i + self.shortest);
        }
        // Compared a byte at a time: the literals are short, and a call to compare them costs more.
        self.literals
            .iter()
            .find(|l| rest.len() >= l.len() && l.iter().zip(rest).all(|(a, b)| a == b))
            .map(|l| i + l.len())
    }

    /// The leftmost occurrence at or after `from` (the first literal, in their order, of those that start there).
    #[inline(never)]
    pub(crate) fn find(&self, haystack: &[u8], from: usize) -> Option<(usize, usize)> {
        let last = haystack.len().checked_sub(self.shortest)?;
        if from > last {
            return None;
        }
        let found = |i: usize| self.literal_at(haystack, i).map(|end| (i, end));
        #[cfg(target_arch = "x86_64")]
        {
            let avx2 = std::is_x86_feature_detected!("avx2");
            let mut at = from;
            loop {
                // The kernel gives the block of the first candidates, which are compared here, out of its loop, so
                // that its loop keeps everything in registers.
                let block = if avx2 {
                    // SAFETY: the processor has AVX2.
                    unsafe { x86::pair_avx2(self, haystack, at, last) }
                } else {
                    // SAFETY: SSE2 is part of x86-64.
                    unsafe { x86::pair_sse2(self, haystack, at, last) }
                };
                match block {
                    Ok((block, mut m, lanes)) => {
                        while m != 0 {
                            let i = block + m.trailing_zeros() as usize;
                            if let Some(f) = found(i) {
                                return Some(f);
                            }
                            m &= m - 1;
                        }
                        at = block + lanes;
                    }
                    Err(next) => return (next..=last).find_map(found),
                }
            }
        }
        #[allow(unreachable_code)]
        (from..=last).find_map(found)
    }
}

/// The vector kernels: `Ok` with the first block that holds a candidate, the candidates as a bit mask and the size of
/// a block, or `Err` with where the blocks ended, for the caller to go on one byte at a time.
#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::PairSearch;
    use std::arch::x86_64::*;

    macro_rules! pair_kernel {
        ($name:ident, $feature:literal, $vec:ty, $lanes:expr, $set1:ident, $loadu:ident, $cmpeq:ident, $or:ident,
         $and:ident, $movemask:ident) => {
            #[target_feature(enable = $feature)]
            pub(super) unsafe fn $name(
                s: &PairSearch,
                haystack: &[u8],
                from: usize,
                last: usize,
            ) -> Result<(usize, u32, usize), usize> {
                let [i, j] = s.at;
                let (mi, oi, xi) = ($set1(s.mask[0] as i8), $set1(s.one[0] as i8), $set1(s.other[0] as i8));
                let (mj, oj, xj) = ($set1(s.mask[1] as i8), $set1(s.one[1] as i8), $set1(s.other[1] as i8));
                let test = |x: $vec, m: $vec, o: $vec, y: $vec| $or($cmpeq($or(x, m), o), $cmpeq(x, y));
                let p = haystack.as_ptr();
                let mut at = from;
                while at + $lanes <= last + 1 {
                    // SAFETY: at + lanes - 1 <= last, and last + max(i, j) < last + shortest <= haystack.len().
                    let (x, y) =
                        unsafe { ($loadu(p.add(at + i).cast::<$vec>()), $loadu(p.add(at + j).cast::<$vec>())) };
                    let m = $movemask($and(test(x, mi, oi, xi), test(y, mj, oj, xj))) as u32;
                    if m != 0 {
                        return Ok((at, m, $lanes));
                    }
                    at += $lanes;
                }
                Err(at)
            }
        };
    }

    pair_kernel!(
        pair_avx2,
        "avx2",
        __m256i,
        32,
        _mm256_set1_epi8,
        _mm256_loadu_si256,
        _mm256_cmpeq_epi8,
        _mm256_or_si256,
        _mm256_and_si256,
        _mm256_movemask_epi8
    );
    pair_kernel!(
        pair_sse2,
        "sse2",
        __m128i,
        16,
        _mm_set1_epi8,
        _mm_loadu_si128,
        _mm_cmpeq_epi8,
        _mm_or_si128,
        _mm_and_si128,
        _mm_movemask_epi8
    );
}
