# Port checklist: the search layer of the .NET reference engine

Every optimisation of sections 4.3 and 4.4 of the design of the .NET reference engine
([Corvus.Regex.Ecma262](https://github.com/corvus-dotnet/Corvus.Regex.Ecma262), `docs/design/design.md`), and the
working-memory parts of section 4.2 they rest on, each marked **done**, **not applicable** (and why) or **to do**.
Section 5 (the source generator) is not applicable to this port as a whole: Rust has no build-time generator step
here, and the plan leaves it out. Where this port does a thing another way, the entry says how.

| Count | |
|---|---|
| Done | 50 |
| Not applicable | 5 |
| To do | 0 |

Items 1 to 38 are the reference's; items 39 to 50 are what bringing the engine level with the `regex` crate added;
item 51 is what brought the C library level with the crate; items 52 and later are what PCRE2 and V8 do that this
engine did not, where they were faster.

## Section 4.2: the program and the working memory

| # | Optimisation | State | In this port |
|---|---|---|---|
| 1 | Flat program, per-thread working memory grown to what a match needs and kept up to a cap | done | From the Go and Rust engine this crate came from (`vm.rs`, `State`): the stack, the groups, the loop counters and the memo are a thread's, so a warm match allocates nothing (`tests/allocations.rs`). |
| 2 | The memo of failed places, so a pattern without backreferences is linear | done | From the engine this crate came from; taken into use once a match has made sixty-four steps a position. It stays on when a match records every group, since what a group holds changes nothing without a backreference. |
| 3 | The clock read every 4,096 steps, and a match that ends before the first check never reads it | done | `CHECK_EVERY`. |
| 4 | The quick check of the next character at every choice, as V8 makes it | done | `set_quick`. |
| 5 | A counted loop gives up where the rest cannot fit | done | `set_needs`, now worked out in bytes of UTF-8 and in UTF-16 code units, one need for each encoding. |
| 6 | A repetition whose body cannot consume runs once or not at all | done | `simplify`. |
| 7 | Small counted repetitions written out: an exact count of one character up to 32, a body with no group up to four | done | `simplify` and `unroll`, as the reference writes them. |

## Section 4.3: finding where to start

| # | Optimisation | State | In this port |
|---|---|---|---|
| 8 | An anchored pattern starts only at 0 | done | From the engine this crate came from. |
| 9 | A literal every match starts with, found by an anchored search | done | `memchr::memmem`, built once with the program (`search.rs`, `Finder::One`). |
| 10 | The needle by the literal's rarest positions, compared over a vector of starts, the literal in full only where they agree | done | `memmem`'s prefilter is that search: it picks the literal's two rarest bytes by its own table of byte frequencies and compares them over a vector. |
| 11 | The lazy third anchor | not applicable | `memmem` compares two bytes and then the literal; the third anchor belongs to the reference's own needle. |
| 12 | A unit rare enough alone searched for by the one-value span search | done | `memmem` of a literal of one byte is `memchr`. |
| 13 | Over UTF-16, the span search, or the fingerprint search where both ends are common letters | done | Another way: the same searchers over the bytes of the code units; an occurrence at an odd byte offset is passed over. |
| 14 | A literal at a fixed distance from the start (`[a-q][^u-z]{13}x`), the start found by walking back over the characters | done | The windows of item 20, with one distance; the walk back is by characters, so a class that admits a character of four bytes before the literal needs no fixed offset. |
| 15 | A literal at a bounded range of distances (`\s[a-zA-Z]{0,12}ing\s`) | done | The windows of item 20. |
| 16 | A literal after a run of disjoint unbounded loops, with a leading `\b`, the runs walked back to the earliest start, the only start tried for the occurrence | done | `Plan::AfterLoops`. |
| 17 | Several literals at once (the multi-literal needle), by a cost model of one or two positions | done | Another way: two literals whose first bytes are rare are found by `aho-corasick`'s automaton, whose prefilter is a `memchr` of those bytes; any others by the fingerprint search. The cost model is that rule. |
| 18 | The pairs of first characters of a sequence of classes as a multi-needle | done | Another way: a class of up to four characters is one character of an anchor with several forms, written out into the literals of the fingerprint search. |
| 19 | Fingerprints (Teddy): buckets, nibble tables, up to three positions | done | `aho-corasick`'s packed searcher, Teddy, for three literals or more and for any case-insensitive anchor. |
| 20 | The literal every alternative holds, at a fixed or bounded distance, the occurrences opening windows of starts tried in order and never twice; the matcher at the occurrence alone where every alternative starts with its literal | done | `Plan::Windows` (`search_windows`), through nested alternations (`.{0,2}(Tom\|Sawyer\|Huckleberry\|Finn)`). |
| 21 | The case-insensitive search: a letter's forms in one search, masks joining the forms that differ in a bit | done | Another way: the forms of each character are written out into literals (three characters of each anchor, more where a character has one form), which Teddy's nibble tables then match. |
| 22 | The long s and the Kelvin sign, whose UTF-8 is longer than s and k, looked for apart | done | Another way: they are forms like any other in the literals of the one search, which takes literals of different lengths, so nothing is looked for apart and no window is needed for them. |
| 23 | The fingerprint search's positions chosen by rarity, beyond ASCII the bytes that tell the literals apart | not applicable | `aho-corasick` chooses the bytes Teddy looks at (the first bytes of each literal); this port uses the crate as it is. |
| 24 | The first set found by a vectorised search for its units | done | Over UTF-8: `memchr` for up to three bytes, Teddy over one-byte literals for up to sixty-four (the first set's ASCII members and the lead bytes of its other members), after a probe of sixteen bytes one at a time where they are common. |
| 25 | The same over UTF-16 | done | `simd16.rs`, AVX2 chosen at run time with SSE2 under it: the first set's units as up to eight ranges (a member beyond the BMP as its lead surrogate, more ranges merged across the narrowest gaps, the ASCII and Latin ones last), a block of ASCII units tested against the ASCII ranges alone; single units by equality, sixty-four units a step on vector boundaries as `memchr` reads. A candidate is then tested against the first set, and the trail half of a pair is passed over. |
| 26 | A failed start skips the rest of its leading loop's run | done | `Search::skip_run`. |
| 27 | A start inside a word skipped where the pattern starts with `\b` and every first character is a word character | done | `Search::leading_word_boundary`. |
| 28 | A minimum length rules out short rests | done | From the engine this crate came from (`need`). |
| 29 | Simple shapes answered without the matcher: a literal alone | done | `Search::literal`. |
| 30 | Simple shapes: one class alone, a sequence of classes, one unbounded loop, an alternation of literals, a case-insensitive ASCII literal | done | `Shape`: one class (`Set`), one greedy unbounded loop of a class (`Loop`, its run's end found by `memchr` where the class leaves out up to three ASCII characters or is the dot), an alternation of literals (`Literals`, the search's own leftmost-first match) and of literals with forms (`Forms`, the alternatives compared in order at each occurrence). A sequence of classes is left to the anchored automaton (item 40), which answers it as fast. |
| 31 | What an enumeration carries: the range known to hold no rare line terminator, so a run of `.` is one search for LF and CR | done | Another way: a run of `.` is one `memchr3` for LF, CR and the lead byte of U+2028 and U+2029, looked at further where it is found (`RunEnd::Line`); nothing is carried from one match to the next. `.*` over the Sherlock text is a fifth of regex's time. |
| 32 | What an enumeration carries: the range known to hold no form the search leaves out | not applicable | No form is left out of the search (item 22). |
| 33 | The give-back of a single-character loop by a vectorised scan for the units what follows can start with | done | `set_back`: where what follows a greedy loop can start with one to three ASCII bytes and nothing else, the give-back is a `memrchr` for them. |
| 34 | The search's kernels as static methods with constants as arguments, no call inside | not applicable | A .NET JIT concern; the searches here are the crates', and the matcher is monomorphised for each encoding. |

## Section 4.4: the automaton

| # | Optimisation | State | In this port |
|---|---|---|---|
| 35 | An automaton over ASCII for a pattern that needs no backtracking, built on first use, its steps bounded | done | From the engine this crate came from (`dfa.rs`). |
| 36 | The automaton takes `\b` and `\B` (a state carries the word bit and the start bit; a boundary is passed when the next unit is known) | done | `dfa.rs`. Not a boundary of a case-insensitive pattern with the u flag, which folds, as in the reference. |
| 37 | One table read per unit for both encodings | done | The automaton reads UTF-8 bytes and UTF-16 units alike. |
| 38 | An automaton over full Unicode | not applicable | The reference has none either; the benchmarks have not asked for one. |

## Beyond the reference: level with the `regex` crate

The work that brought this engine over UTF-8 level with `regex` on the Industry workloads, each change measured A/B
against the build before it with `regex` beside them (cores 0 to 11, nothing else running), the data set at 0 wrong
after each.

| # | Optimisation | State | In this port |
|---|---|---|---|
| 39 | Simple shapes and the dot's run by `memchr` | done | Items 30 and 31. |
| 40 | An anchored, leftmost-first automaton over ASCII for where a match from a start ends | done | `adfa.rs`: threads in priority order, a match ending the lower ones, the word and start bits of item 36, the states premultiplied by the stride so a step is one load and an add; it gives up at the first byte beyond ASCII and the matcher runs instead. Used for a pattern with no group and no backreference, at every start the search tries. |
| 41 | The terms before a window's anchors read backwards from each occurrence, so that only the starts they allow are tried | done | The reverse search of the `regex` crate: `adfa::reverse`, run by `search_reverse` with the starts it allows held as a bit mask and tried in order, an occurrence within reach of the earliest taken in first so leftmost order holds. |
| 42 | Case-insensitive anchors cut to one length by `regex`'s literal rule | done | `counts` (`Seq::optimize_by_preference`): full if ten literals or fewer, else five characters if that makes ten or fewer, else four if sixty-four or fewer, else three, two, one. Where an anchor is cut, its UTF-8 literals are cut to the bytes of the shortest (`utf8_cuts`), as `regex` cuts by bytes, so the long s and the Kelvin sign make no extra literals. |
| 43 | Two literals by the automaton only when they start with the same rare byte | done | `Finder::new`; any other pair goes to Teddy, which looks at three bytes. |
| 44 | A failed start of a leading loop's run passes over the occurrences in the rest of the run | done | `search_after_loops` with one loop: every occurrence in the run walks back to the start just tried. |
| 45 | Over UTF-16, literals found by searches of the code units themselves | done | One literal: `Needle16`, the vector search for its two rarest units (ranked for UTF-16) and a compare. Up to four literals with one rare first unit: the vector search for it. Several: the low bytes of the units packed into bytes by a vector loop and searched (`Literals::find_packed`), the case forms of a case-insensitive set folded to lower case on both sides so a few literals are searched where there were many (the .NET reference's packed case-insensitive search), and the literals with the long s or the Kelvin sign looked for apart at those units, in a chunk that holds a unit beyond ASCII. The chunk packed last is kept between the searches of one global search (`find_iter`), which borrows the text, so it is not packed again. A run of a loop ends where the vector search finds a unit the loop's class leaves out. |
| 46 | The anchor and what follows it checked at each occurrence before any start is walked back to | done | The anchored automaton of the anchors' rests (an alternation of each anchor and the terms after it, to the end of the pattern), run at each occurrence of a window or of a literal after loops, stopping at the first place a match could end (`Anchored::matches_at`). Left out where the anchor ends its alternative. `\b\w+n\b` is the row: an `n` inside a word is passed over without walking back to the word's start. |
| 47 | The reverse automaton's table premultiplied, and the look for a nearer occurrence made without a search call on a few bytes | done | `Reverse::starts`, `Literals::find_within`. |
| 48 | Up to four literals with one rare first byte found by `memchr` for it and a compare of each | done | `Finder::Shared`, as `regex` finds `Sherlock\|Street` (its prefilter is a `memchr` of `S`). |
| 49 | Literals with forms written out in full answered by the search alone | done | `Shape::Literals` takes them too (`the` under the i flag is eight literals, and the search's leftmost-first match is the pattern's); an alternation of literals with forms cut short is compared at each occurrence by the anchored automaton where the text is ASCII. |
| 50 | A pattern with no literal read once by a forward automaton, not tried at every start its first set allows | done | `adfa::build_forward`, built on the first search that wants it: the anchored automaton with a start joined at every unit behind the threads already alive, the threads of the current start kept apart so that a state with no thread of an earlier start is known. From a candidate the first set finds, it reads on to where a match first ends (the starts from the candidate to there are then tried, the first that matches being the leftmost) or to a state with no earlier thread (no match starts before it, and the first-set search goes on from there). A unit beyond ASCII hands the starts so far to the matcher. IPv4 is the row: every digit was a start. |

### The rows still above `regex`

Measured with the interleaved harness at 97d9cea (7 rounds, this engine, `regex`, regress, V8 and the ea6a7d1 build
in turn; see `docs/benchmarks/README.md`). A row is above or below `regex` only where the rounds say so: a one-sided
Mann-Whitney test at p < 0.01, at least three rounds in four the same way, and a difference of medians beyond both
engines' spread. By that test this engine over UTF-8 is below `regex` on 12 rows, level on 44 (median ratio 0.97,
the dot-all row left out), and above on one:

| Row | This engine | `regex` | Ratio | Why |
|---|--:|--:|--:|---|
| `Holmes.{0,25}Watson\|Watson.{0,25}Holmes` | 66.6 us | 59.3 us | 1.12 | Both find the 542 occurrences with the same Teddy searcher and check each with an automaton; Teddy takes 37.8 us here against 34.4 us in `regex`, the check 19.9 against 18.1 us. Reading on through the window after a failed occurrence, as `regex`'s lazy automaton does, was tried two ways and both were slower: looking at the window's units for a literal's first byte before calling the search (62.9 to 68.9 us over UTF-8, 88 to 138 us over UTF-16) and running the forward automaton (item 50) from each occurrence (1.12 times the time). Not an ECMA-262 rule. |

IPv4, above `regex` before the forward automaton, is level with it. With the pair search (item 52)
`Holmes.{0,25}Watson|Watson.{0,25}Holmes` finds its occurrences by `H` or `W` and `m` or `s` three bytes on, and is
below `regex` (42.8 us against 63.2 in the final run of this work, 0.68), as IPv4 is with the window of item 54 (660
against 2,039 us). On the central page's runner `\s[a-zA-Z]{0,12}ing\s` (1.14) and `Tom.{10,25}river|river.{10,25}Tom`
(1.06) were above `regex`; on the reference machine both are level with it by the test above (0.99 and 1.03 to 1.09
with p above 0.01), so what they spend there could not be looked at here.

### UTF-16 against UTF-8

In the same run the median row over UTF-16 takes 1.49 times its UTF-8 time; the Russian and Chinese rows, where
UTF-16 is the shorter encoding, take 0.68 to 1.05 of it. The rows over 2 are those whose UTF-16 text leaves a level of
cache its UTF-8 text fits in, measured here as one `memchr` pass over buffers of the texts' sizes:

| Text | UTF-8 | UTF-16 | One read of the UTF-8 size | One read of the UTF-16 size | Rows over 2 (UTF-16/UTF-8) |
|---|--:|--:|--:|--:|---|
| Leipzig | 16 MB, in the 24 MB L3 cache | 32 MB, beyond it | 389 us | 1,948 us (5.0 times) | the eleven Leipzig rows, 2.1 to 3.1 |
| mariomka | 6.8 MB | 13.6 MB | 99 us | 277 us (2.8 times) | `[\w\.+-]+@…` 2.13 |
| Sherlock | 0.6 MB, in the 1.25 MB L2 cache of a core | 1.2 MB, at its edge | 4.8 us | 11.0 us (2.3 times) | `.*` 2.04 |

Every Leipzig row over UTF-16 is within 1.0 to 1.6 times the 1,948 us it takes to read the text once. Before this work
the case-insensitive rows over UTF-16 were 5 to 9 times their UTF-8 time (`Sher[a-z]+\|Hol[a-z]+` /i 626 against
76 us) and `\p{Sm}` 5.3 times; they are now 1.1 to 1.8 times, and `\p{Sm}` is within 1.6 times one read of the text.

`.*` with the s flag is answered without reading the text (8.8 ns over UTF-8, 10.3 ns over UTF-16): the pattern is one
greedy loop, of least count 0 or 1 and no upper bound, of a class that holds every character (`Shape::Loop` with
`RunEnd::All`), the program records no group, the search is not sticky and the pattern is not anchored; the match is
then from where the search starts (moved back to a pair's start with the u or v flag) to the end.
`tests/api.rs` (`a_run_of_the_dot_with_the_s_flag_is_the_rest_of_the_text`) checks it against the matcher over UTF-16
with lone halves anywhere and last, from every start (between the halves of a pair included), with and without the u
flag and with the sticky flag. Rows under 100 ns are kept out of the medians above.

Tried and taken out, each slower on the rows it was for: a search that folds case in place of the written-out forms;
the long s and the Kelvin sign looked for apart (item 22's reference way); an unanchored forward automaton with a
backward one to find the start (no row gained); budgets of 64 literals shared round the anchors, and anchors of four
characters; Teddy for `Sherlock|Street` (3.4 times slower than `memchr` for `S`); the byte cut of a literal inside a
character (the Cyrillic letters with a three-byte form, 1.2 times slower); the anchored automaton for the forms of one
literal (`the` /i, 1.1 times slower than comparing them); over UTF-16, the case forms of a set packed as their own bytes
(Teddy with 32 literals in eight buckets took 24 ms on the Leipzig text against 3.8 ms in sixteen, and folding the
packed text to lower case beat both) and a separate kernel for one unit before the loads were aligned.

## Beyond the reference: the C library level with the crate

| # | Optimisation | State | In this port |
|---|---|---|---|
| 51 | The matches of a global search a batch at a time, in one loop, with the results of the search and the matcher in registers | done | `Matches::next_batch` and `CaptureMatches::next_batch` (the C library's `cre_matches_next_batch` and `cre_matches_next_captures_batch`) fill a slice the caller owns: a pattern of a simple shape by the search inlined into one loop over the batch (one class and one loop of a class each by a loop of its own, an empty match before an ASCII unit going on one unit later without a call), any other by the matcher with the thread's memory taken once for the batch, and the groups written from the matcher's memory with no allocation. `match_with` returns two words (a start of `NO_MATCH`, `STOPPED_TIME` or `STOPPED_MEMORY` for the rest): a `Result` or an `Option` of a pair merged from several arms came back through memory written as two words and read back as one, a failed store forward on every match. Over the Sherlock text the C library took twice the crate's time on the rows with most matches (`.*`, `[^\n]*`, `\p{L}`, `\p{Ll}`: 473, 324, 5,827 and 5,499 us against the crate's 239, 172, 2,686 and 2,752 us), which a call a match does not explain: the crate's loop had the search inlined, the library's called through three functions a match with the stalls between them. With the batch they take 259, 174, 2,387 and 2,457 us, 0.89 to 1.08 of the crate's time; the captures of `(\w+)` take 36 ns a match against 60 ns with `cre_matches_next_captures` and 42 ns with the crate's `captures_read_at`. |

## Beyond the reference: what PCRE2 and V8 do

The rows where PCRE2 (10.49, with its JIT) or V8 was clearly faster than this engine over UTF-8, on the central
benchmark page's runner and on the reference machine, each looked at for what the faster engine does, and that
brought over where it is not an ECMA-262 difference. Each change was measured A/B against the build before it with
`regex` beside them, in the interleaved harness (Mann-Whitney p < 0.01, beyond both builds' spread), the data set at 0
wrong after each.

| # | Optimisation | State | In this port |
|---|---|---|---|
| 52 | A few literals found by two of their byte positions, each holding one or two values, compared over a vector of starts (PCRE2's `fast_forward_char_pair_simd`) | done | `pair8.rs`, `Finder::Pair`: the positions are the rarest pair (by `memchr`'s table of byte frequencies) whose values are apart, a position whose two values differ in one bit (the cases of an ASCII letter, the lead bytes of a Cyrillic letter's cases) compared once with that bit set; the kernel gives the block of the first candidates and they are compared out of its loop, which keeps it in registers, and a word's cases written out are known a literal once the bytes pass the sets of each position. Where the two positions hold common bytes the fingerprint search is kept (`Tom\|river` took 1.24 times its time, `[A-Za-z]awyer\|[A-Za-z]inn` 4.3 times). `Sherlock\|Holmes` 47 to 25 us (PCRE2 29), `Sher[a-z]+\|Hol[a-z]+` 46 to 34 (36), `Holmes.{0,25}Watson\|...` 67 to 39 (35), `Holmes` /i 51 to 27 (43), `the` /i 287 to 199 (247), `Twain` /i 1,247 to 925 (936), `Шерлок Холмс` /i 170 to 88 (82). |
| 53 | One literal with a byte rare in text found by `memchr` for that byte, as V8 searches a literal pattern for its first character | done | `Finder::Rare`, for a literal holding one of `ZXQKVJzqjx` (one in a thousand bytes of English text or fewer), the rarest of them: `memchr` reads a block at twice the speed of `memmem`'s search for the two rarest bytes, which is kept for every other literal (a capital letter, one in five hundred, is found too often over the Leipzig text, read from memory: `Holmes` 777 us against 651). `zqj` 14.6 to 5.5 us (V8 5.3), `aqj` 14.6 to 6.8 (V8 204). |
| 54 | A window of positions read one unit at a time before the first set is searched for (V8's Boyer-Moore lookahead, `BoyerMooreLookahead` in its regexp compiler) | done | `adfa::Skip`, for a pattern the forward automaton reads (item 50): the units each position can be before a match can end, from the pattern's automaton without its determinisation (ASCII sets and whether a character beyond ASCII can be there, the positions after such a character not known), and the window whose units are rare enough by a rough table of text frequencies that one unit read passes over three starts or more. A unit at the window's end that none of its positions can be rules out the starts of the window. IPv4 is the row: its first set, the digits, is one byte in sixty of the mariomka text, and its window is eleven positions of digits and dots, so one byte in eleven is read where no digit or dot is near: 1,856 to 682 us over UTF-8 (V8 739), 2,020 to 832 over UTF-16. The window is boxed, and the loop is a call of its own: kept inline, the program's other fields moved and the shapes' loops took 1.05 to 1.15 times their time. |
| 55 | Where every match starts with one literal, the automaton goes on after an occurrence rather than reading the literal again (as PCRE2's compiled match goes on after the units its search compared) | done | `adfa::Anchored::after` gives the state after the literal from each start state, kept with the program (`after_literal`), and `find_after` reads on from the occurrence's end: `the\s+\w+` 364 to 323 us over UTF-8 (PCRE2 246, `regex` 367). The rest of the gap to PCRE2 is its match compiled to machine code, run inline from its search, against an automaton's table read a unit at a time and a call into the matcher for each occurrence; not an ECMA-262 rule. |

## The search layer, step by step

Each step measured over the Industry workloads over UTF-8 (the harness at 0.3 seconds and three passes, an
unpinned look on the reference machine during the work, so a few percent either way is noise), in microseconds of one
count; the data set answered right in full after each. "scan" is the engine as it came out of the JSON Schema
evaluator, trying every position its first set allows. Each column adds one search: the needle for one literal at the
start (memmem), then two literals anywhere (the automaton), then three or more (Teddy), then the windows at distances,
then the case-insensitive forms, then the walk back over leading loops with the skip over a failed loop's run and the
leading `\b`. The steps after these (the first set's bytes, the writing out of small repetitions, the anchors of three
characters, the literal alone) were measured in the full runs reported with the work. regex is the same
look at the `regex` crate.

| Workload | scan | + needle | + 2 literals | + fingerprint | + windows | + forms | + loops | regex |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| `mariomka: [\w\.+-]+@[\w\.-]+\.[\w\.-]+` | 87,581.5 | 89,417.8 | 90,316.9 | 85,692.5 | 89,826.1 | 86,032.9 | 130.6 | 354.7 |
| `mariomka: [\w]+://[^/\s?#]+[^\s?#]+(?:\?[^\s#]*...` | 80,501.1 | 78,136.3 | 80,107.9 | 81,129.0 | 78,716.2 | 78,983.0 | 641.1 | 709.6 |
| `mariomka: (?:(?:25[0-5]\|2[0-4][0-9]\|[01]?[0-9][...` | 13,203.5 | 13,292.2 | 13,438.0 | 14,491.0 | 14,017.3 | 13,417.7 | 13,415.0 | 2,118.8 |
| `sherlock: Sherlock` | 720.7 | 16.7 | 17.1 | 16.3 | 15.8 | 16.1 | 16.1 | 15.6 |
| `sherlock: Holmes` | 793.8 | 23.8 | 25.1 | 22.9 | 23.6 | 23.4 | 22.4 | 18.8 |
| `sherlock: Sherlock Holmes` | 720.6 | 16.1 | 16.1 | 16.4 | 16.0 | 16.2 | 15.7 | 15.7 |
| `sherlock: Sherlock /i` | 1,121.0 | 1,105.4 | 1,155.0 | 1,178.7 | 1,100.1 | 65.0 | 68.4 | 42.5 |
| `sherlock: Holmes /i` | 1,102.0 | 1,120.8 | 1,185.8 | 1,148.0 | 1,262.5 | 60.2 | 57.3 | 53.7 |
| `sherlock: Sherlock Holmes /i` | 1,093.2 | 1,130.5 | 1,151.3 | 1,125.4 | 1,158.3 | 67.2 | 68.8 | 44.0 |
| `sherlock: Sherlock\s+Holmes` | 695.2 | 16.9 | 16.8 | 17.3 | 17.2 | 17.2 | 16.4 | 16.8 |
| `sherlock: Sherlock\|Street` | 748.5 | 713.4 | 13.9 | 14.2 | 14.2 | 14.0 | 13.2 | 11.4 |
| `sherlock: Sherlock\|Holmes` | 770.4 | 795.6 | 56.1 | 53.5 | 54.7 | 52.5 | 53.7 | 51.6 |
| `sherlock: Sherlock\|Holmes\|Watson\|Irene\|Adler\|Jo...` | 963.5 | 955.3 | 1,002.4 | 68.3 | 66.6 | 65.2 | 63.9 | 54.7 |
| `sherlock: Sherlock\|Holmes\|Watson\|Irene\|Adler\|Jo... /i` | 4,663.5 | 4,489.0 | 4,577.9 | 4,694.7 | 4,662.5 | 1,980.0 | 2,041.0 | 187.9 |
| `sherlock: Sher[a-z]+\|Hol[a-z]+` | 897.4 | 763.5 | 51.9 | 55.4 | 51.2 | 54.1 | 52.2 | 47.5 |
| `sherlock: Sher[a-z]+\|Hol[a-z]+ /i` | 1,856.6 | 1,745.7 | 1,888.4 | 1,809.3 | 1,793.8 | 127.8 | 126.5 | 78.1 |
| `sherlock: Sherlock\|Holmes\|Watson` | 795.8 | 760.8 | 770.3 | 61.3 | 59.1 | 56.1 | 61.1 | 51.3 |
| `sherlock: Sherlock\|Holmes\|Watson /i` | 2,153.7 | 2,268.9 | 2,273.7 | 2,213.4 | 2,164.9 | 1,343.8 | 1,413.4 | 93.8 |
| `sherlock: zqj` | 733.5 | 14.6 | 15.1 | 14.9 | 14.6 | 14.8 | 14.5 | 14.5 |
| `sherlock: aqj` | 1,184.5 | 14.7 | 15.0 | 15.0 | 14.6 | 16.2 | 14.4 | 14.5 |
| `sherlock: aei` | 1,231.4 | 14.7 | 14.5 | 14.5 | 14.3 | 14.3 | 14.7 | 15.0 |
| `sherlock: the` | 1,318.1 | 246.0 | 259.8 | 262.3 | 253.8 | 247.5 | 252.1 | 176.1 |
| `sherlock: The` | 744.0 | 29.0 | 30.5 | 29.4 | 30.0 | 29.0 | 30.2 | 22.8 |
| `sherlock: the /i` | 1,481.1 | 1,473.2 | 1,406.0 | 1,548.7 | 1,457.3 | 396.7 | 365.7 | 312.5 |
| `sherlock: the\s+\w+` | 1,361.7 | 358.2 | 364.2 | 343.4 | 340.2 | 340.3 | 337.4 | 363.1 |
| `sherlock: .*` | 903.6 | 863.3 | 916.8 | 874.6 | 870.5 | 909.5 | 891.7 | 1,336.2 |
| `sherlock: [^\n]*` | 758.3 | 770.9 | 770.2 | 759.7 | 785.3 | 763.1 | 777.0 | 1,293.2 |
| `sherlock: .* /s` | 355.2 | 360.7 | 351.1 | 359.7 | 367.7 | 356.9 | 345.6 | 908.2 |
| `sherlock: \p{L}` | 5,775.7 | 6,147.4 | 6,019.9 | 6,285.1 | 6,356.1 | 5,851.3 | 5,774.8 | 5,715.4 |
| `sherlock: \p{Lu}` | 949.2 | 945.0 | 991.5 | 1,013.4 | 966.6 | 990.5 | 972.5 | 885.2 |
| `sherlock: \p{Ll}` | 5,906.3 | 5,714.9 | 6,246.9 | 5,894.7 | 5,704.6 | 5,754.2 | 5,915.3 | 5,626.2 |
| `sherlock: \w+` | 2,888.7 | 2,761.7 | 3,142.7 | 2,943.3 | 2,931.3 | 2,884.1 | 2,934.1 | 2,987.5 |
| `sherlock: \w+\s+Holmes` | 9,968.1 | 9,631.5 | 9,590.8 | 11,209.6 | 9,611.6 | 9,639.9 | 26.5 | 32.4 |
| `sherlock: \w+\s+Holmes\s+\w+` | 9,935.3 | 9,817.5 | 9,587.0 | 9,952.6 | 10,114.4 | 10,067.3 | 29.0 | 27.1 |
| `sherlock: Holmes.{0,25}Watson\|Watson.{0,25}Holmes` | 806.5 | 799.3 | 92.7 | 95.9 | 92.7 | 86.8 | 86.5 | 59.1 |
| `sherlock: ^Sherlock Holmes\|Sherlock Holmes$ /m` | 779.5 | 16.1 | 725.6 | 16.3 | 17.1 | 16.3 | 17.9 | 16.5 |
| `sherlock: \b\w+n\b` | 5,925.4 | 4,803.7 | 4,722.9 | 4,914.7 | 4,711.9 | 4,413.9 | 1,550.2 | 877.5 |
| `sherlock: [a-q][^u-z]{13}x` | 6,580.0 | 6,691.9 | 24.4 | 7,112.7 | 23.6 | 24.5 | 24.2 | 26.1 |
| `sherlock: [a-zA-Z]+ing` | 8,966.3 | 8,620.7 | 8,979.5 | 9,426.4 | 8,884.6 | 9,324.9 | 171.8 | 712.2 |
| `sherlock: \s[a-zA-Z]{0,12}ing\s` | 5,039.5 | 4,868.6 | 369.3 | 5,075.7 | 390.6 | 366.1 | 375.2 | 164.7 |
| `leipzig: Twain` | 19,967.4 | 762.7 | 801.4 | 1,209.9 | 780.8 | 678.8 | 744.1 | 706.3 |
| `leipzig: Twain /i` | 36,899.9 | 35,471.7 | 36,525.3 | 37,534.3 | 35,503.6 | 1,238.2 | 1,621.8 | 1,236.4 |
| `leipzig: [a-z]shing` | 130,175.3 | 125,657.5 | 814.6 | 125,757.8 | 755.4 | 887.2 | 1,038.8 | 846.7 |
| `leipzig: Huck[a-zA-Z]+\|Saw[a-zA-Z]+` | 21,552.0 | 21,509.8 | 1,331.9 | 1,454.9 | 1,183.4 | 1,317.6 | 1,318.4 | 950.1 |
| `leipzig: Tom\|Sawyer\|Huckleberry\|Finn` | 23,945.5 | 22,576.5 | 22,845.5 | 1,325.0 | 1,204.1 | 1,031.5 | 1,415.2 | 1,038.9 |
| `leipzig: Tom\|Sawyer\|Huckleberry\|Finn /i` | 88,671.9 | 85,751.6 | 91,130.6 | 95,010.3 | 87,165.3 | 46,642.5 | 48,190.2 | 2,055.8 |
| `leipzig: .{0,2}(Tom\|Sawyer\|Huckleberry\|Finn)` | 226,251.2 | 219,612.1 | 221,305.2 | 221,935.9 | 1,072.1 | 1,172.7 | 1,406.5 | 18,061.2 |
| `leipzig: .{2,4}(Tom\|Sawyer\|Huckleberry\|Finn)` | 246,332.0 | 230,916.6 | 228,376.3 | 232,114.7 | 1,043.6 | 1,177.4 | 1,457.4 | 18,768.1 |
| `leipzig: Tom.{10,25}river\|river.{10,25}Tom` | 31,662.4 | 33,317.4 | 7,469.7 | 7,314.0 | 7,778.8 | 7,132.1 | 7,464.7 | 1,433.9 |
| `leipzig: ([A-Za-z]awyer\|[A-Za-z]inn)\s` | 222,467.4 | 231,641.0 | 30,029.0 | 238,454.6 | 31,607.0 | 30,289.4 | 31,950.6 | 1,032.1 |
| `leipzig: \p{Sm}` | 20,030.5 | 19,486.0 | 20,547.8 | 20,413.3 | 20,919.4 | 19,883.6 | 20,333.7 | 18,451.8 |
| `rebar-ru: Шерлок Холмс` | 3,682.1 | 57.6 | 55.7 | 59.3 | 57.7 | 57.1 | 62.3 | 46.2 |
| `rebar-ru: Шерлок Холмс /i` | 4,745.9 | 4,824.4 | 4,717.7 | 4,611.5 | 4,721.1 | 282.9 | 280.3 | 187.5 |
| `rebar-ru: Шерлок Холмс\|Джон Уотсон\|Ирен Адлер\|и...` | 9,248.1 | 10,083.2 | 9,144.2 | 252.4 | 243.8 | 246.7 | 255.2 | 220.9 |
| `rebar-ru: Шерлок Холмс\|Джон Уотсон\|Ирен Адлер\|и... /i` | 12,065.0 | 12,106.1 | 13,387.7 | 12,643.9 | 11,998.9 | 6,055.4 | 5,836.4 | 980.0 |
| `rebar-zh: 夏洛克·福尔摩斯` | 2,117.4 | 20.8 | 20.0 | 20.3 | 20.6 | 20.7 | 20.5 | 21.1 |
| `rebar-zh: 夏洛克·福尔摩斯\|约翰华生\|阿德勒\|雷斯垂德\|莫里亚蒂教授` | 2,777.4 | 2,794.8 | 2,772.3 | 66.4 | 61.2 | 61.2 | 62.9 | 55.2 |
