# CommonMark Conformance Roadmap

Tracking `marki-parse` toward full CommonMark conformance without sacrificing
performance. Living document — update the status table and changelog as each
phase lands.

## Baseline (2026-06-24)

- **Conformance: 485/652 (74.4%)** via `cargo test --test commonmark commonmark_conformance -- --nocapture`
- The `commonmark_conformance` test **reports but does not gate**; the hard gate
  is `no_panic_on_any_fixture`. Phase 6 flips conformance into a floor assertion.

### Per-section status (baseline)

| Section                       | Pass    | Target phase |
|-------------------------------|---------|--------------|
| Emphasis and strong emphasis  | 68/132  | 1            |
| Links                         | 54/90   | 2, 3         |
| Images                        | 14/22   | 2, 3         |
| Code spans                    | 14/22   | 4            |
| Entity/numeric references     | 12/17   | 3            |
| Lists                         | 20/26   | 5            |
| List items                    | 45/48   | 5            |
| Block quotes                  | 20/25   | 5            |
| Tabs                          | 3/11    | 5            |
| Fenced code blocks            | 25/29   | 5            |
| Backslash escapes             | 9/13    | 3            |
| Thematic breaks               | 16/19   | 1, 6         |
| Hard line breaks              | 13/15   | 4            |
| Raw HTML                      | 18/20   | 1, 6         |
| Autolinks                     | 18/19   | 3            |
| ATX/Setext headings           | 17/18, 26/27 | 5, 6    |
| HTML blocks                   | 42/44   | 6            |
| Link reference definitions    | 23/27   | 3            |

## Performance baseline (regression gate)

Every phase must keep conformance green **and** not regress these benchmarks
dramatically. Rule of thumb: **investigate any median regression > 10%**, and
do not merge a phase with a > 25% regression on the corpus benches without an
explicit, documented justification.

`cargo bench --bench parse` medians (2026-06-24, release+LTO):

| Bench                          | Median   |
|--------------------------------|----------|
| heading                        | 78.0 ns  |
| paragraph                      | 87.2 ns  |
| inline_rich                    | 309.7 ns |
| code_block                     | 66.7 ns  |
| unordered_list                 | 394.2 ns |
| ordered_list                   | 382.9 ns |
| blockquote                     | 270.7 ns |
| horizontal_rule                | 53.3 ns  |
| mixed_document_bench           | 1.292 µs |
| fixture/awesome                | 87.8 µs  |
| fixture/commonmark_spec        | 205.8 µs |
| fixture/rust_readme            | 6.82 µs  |
| scaling/100                    | 86.3 µs  |
| spec_vectors/parse_each        | 230 µs   |
| spec_vectors/parse_concatenated| 126.6 µs |
| spec_vectors/parse_and_render  | 359 µs   |

The corpus rows (`fixture/*`, `spec_vectors/*`) are the load-bearing gates;
the tiny single-construct benches are noisy and used only as directional signals.

## Phase plan

Each phase: implement → `cargo test` (conformance breakdown) → `cargo bench`
(compare to table above) → update this doc → commit.

### Phase 1 — Emphasis delimiter stack  ✅ DONE (74.4% → 85.4%)
Replaced the naive first-match nesting in `src/inline.rs` with the standard
CommonMark two-pass **delimiter-stack + flanking** algorithm. Implementation:
- `EmphArena`: index-based doubly-linked node list + parallel delimiter stack,
  adapted to marki's flat post-order pool (`emit_list` flushes in post-order).
- `process_emphasis` mirrors commonmark.js (`openers_bottom` indexed by
  char/can_open/origdelims%3; odd_match rule-of-3; `use_delims` = 2 iff both ≥2).
- `scan_delims` for flanking; `contiguous_merge` recombines split text nodes.
- The no-`*`/`_` fast path (`InlineBuf`) is untouched; only emphasis-bearing
  contexts take the arena path.
- Arenas are pooled in a thread-local free-list (`with_arena`) to amortize the
  `Vec` allocations — without it, inline-heavy benches regressed ~2×.

Results: **Emphasis 68→131/132**, overall **485→557/652**. All 134 lib tests
pass, clippy clean. Perf gate held: corpus benches within ±7% of baseline
(awesome −8%, commonmark_spec +6.8%, rust_readme −2%; spec_vectors flat).

### Phase 2 — Links/images: destination/title parsing + normalization
**2a + 2b DONE (85.4% → 89.4%). 2c remaining.**

- **2a ✅ destination/title syntax** (85.4% → 87.1%): replaced the naive
  "grab between parens + guess title backwards" with a real `CommonMark` §6.3
  link-tail parser in `src/inline.rs` (`scan_link_tail` / `scan_link_destination`
  / `scan_link_title` / `skip_link_ws`). Handles the angle `<…>` form, balanced
  bare destinations, space/newline/control rejection, and the three title quote
  forms. Pure source-slicing — no type or render change. Deleted `split_url_title`.
- **2b ✅ URL/title normalization** (87.1% → 89.4%): render-time encoders in
  `src/html.rs`. `escape_href` now resolves backslash escapes + numeric entities,
  preserves existing `%XX`, and percent-encodes unsafe bytes via the `HREF_SAFE`
  table (`encodeURI` semantics). `escape_href_autolink` does the same minus
  backslash resolution (autolinks are literal). `escape_link_title` resolves
  escapes + numeric entities then HTML-escapes. Autolinks now **19/19**.
- **2c TODO — inline content in link-text/image-alt + link nesting**: parse alt
  as inlines (573–589), forbid links-in-links and handle outer-link suppression
  (`[foo [bar](/uri)](/uri)` → 518/519/532/533). Needs brackets on the
  delimiter stack — the hardest piece. ~12 examples.

Note: the remaining Entities/title failures (25, 32–34, 41, 506, 503) need the
full HTML5 **named-entity** table — moved to Phase 3.

### Phase 3 — Named-entity table  (target ~91%)
**DONE (91.0% → 92.2%).** Vendored the full ~2125-entry HTML5 named character
reference table (`src/entities_table.rs`, from `pulldown-cmark` 0.12.2 under
MIT, sorted for binary search). `entity.rs` gained a unified `decode_entity`
returning an `Entity` (one scalar for numeric, a one/two-scalar `&'static str`
for named) with a non-allocating `chars()` iterator; the three `html.rs`
encoders (`escape_text`, `escape_href`, `escape_link_title`) and a new
`escape_info_string` for code-fence languages now resolve named refs.

Two follow-on fixes were needed: a backslash-escaped `&` must stay literal
(`\&ouml;` → `&amp;ouml;`), so the fast-path splits the escaped char into its
own text node and `emit_list` suppresses the contiguous-text merge when the last
node is an escaped `&`. Result: Entities **17/17**, Backslash escapes **13/13**,
Fenced code blocks stay 29/29 (info-string `foo\+bar`/`f&ouml;` now decode).

### Phase 4 — Code-span & line-break normalization  (target ~91%)
**DONE (89.4% → 90.5%).** Code-span §6.1 normalization moved to render time
(`escape_code_span` in `src/html.rs`): collapse interior line endings to spaces,
then strip one leading/trailing space unless the content is all spaces. The
parser (`try_parse_inline_code`) now returns the raw inter-backtick slice.
Code spans 16→20/22, Hard line breaks 13→15/15 (the `` `code  \nspan` `` cases
fall out of the same pass). Remaining 2 (342, 347) are backtick-vs-link/fence
*precedence*, deferred to the long tail.

### Phase 5 — Block-level: tabs + containers  (target ~96%)
Tab expansion to 4-col stops in `src/block.rs`; ~~fenced-code indent stripping~~
(**done**, see below); loose/tight list detection; lazy continuation and nested
blockquote/list edges.

**Fenced-code indent strip DONE (90.5% → 91.0%, Fenced code blocks 26→29/29).**
`RawSection::CodeBlock` now carries the opening fence's `indent` (0–3). In pass 2
the common `indent == 0` case stays a zero-copy contiguous `CodeBlock`; when
`indent > 0`, each content line is dedented by up to `indent` leading spaces
into the line pool and emitted as the existing line-backed `CodeLines` variant
(removing interior bytes breaks slice contiguity, so it can't stay one slice).
Perf gate held (dedent path only runs for the rare indented fence).

### Phase 6 — Long tail + gate
Remaining thematic-break/setext/raw-HTML/HTML-block edge cases. Then flip the
conformance test from reporting to asserting a floor (e.g. `assert!(pct >= 95.0)`)
to lock in progress and prevent regressions.

## Changelog

- 2026-06-24: Baseline recorded (74.4%, perf table above). Roadmap created.
- 2026-06-24: Phase 1 complete — emphasis delimiter-stack rewrite. 74.4% → 85.4%
  (Emphasis 68→131/132). Perf gate held (corpus benches within ±7%). Next: Phase 2.
- 2026-06-24: Phase 2a+2b complete — real link destination/title parser +
  render-time URL/title normalization. 85.4% → 89.4% (Links 59→76, Images 14→15,
  Autolinks 18→19/19, Backslash 9→12, Link-ref-defs 23→26). Perf gate held
  (render-only + scanner change; commonmark_spec +7%). Next: Phase 2c (inline
  alt + link nesting), then Phase 3 (named entities).
- 2026-06-24: Phase 4 complete — code-span §6.1 normalization at render time
  (`escape_code_span`). 89.4% → 90.5% (Code spans 16→20, Hard line breaks
  13→15/15). Render-only, perf unaffected. Next: Phase 2c or Phase 3.
- 2026-06-24: Phase 5 (partial) — fenced-code indent strip. 90.5% → 91.0%
  (Fenced code blocks 26→29/29). `RawSection::CodeBlock` carries the fence
  indent; indent>0 dedents into the line pool as `CodeLines`. Perf gate held.
  Next: Phase 3 (named entities).
- 2026-06-24: Phase 3 complete — vendored HTML5 named-entity table + unified
  `decode_entity`. 91.0% → 92.2% (Entities 17/17, Backslash 13/13). Three
  sections now perfect that weren't. Perf gate held (verified against a
  same-session stashed baseline; render-path change, parse benches flat).
  Next: Phase 2c (link nesting + inline alt) or the block-level long tail
- 2026-06-24: Phase 2c (part 1) — image alt as parsed inlines. 92.2% → 93.3%
  (Images 15→22/22, all perfect). `Inline::Image.alt` is now an `InlineSpan`
  parsed like link text; the renderer flattens it to plain text
  (`render_alt_text`), dropping emphasis/link/code markup but keeping textual
  content and recursing into nested images (`![foo *bar*]` → alt `foo bar`,
  `![foo ![bar](/u)](/v)` → alt `foo bar`). Fixed examples 573/574/575/576/577/
  585/589. Perf gate held (same-session stashed baseline; nested parse only on
  the image path). Next: Phase 2c part 2 — brackets on the delimiter stack for
  link/image *nesting precedence* (518/519/520/532/533), then the block-level
  tail (tabs, lists, blockquotes).
- 2026-06-24: Quick wins batch. 93.3% → 95.1% over four commits:
  - **ATX heading edges** (93.3% → 93.7%, ATX 18/18): the opening hash run may
    be followed by a space, a tab, **or** end-of-line — bare `#` is an empty
    heading and `#\tFoo` is valid (`src/block.rs`).
  - **Raw HTML comments** (93.7% → 93.9%, Raw HTML 20/20): CommonMark 0.30
    rules — `<!-->`/`<!--->` are complete comments, otherwise the text after
    `<!--` may not begin with `>` or `->` (`src/raw_html.rs`).
  - **Inline-code/autolink/raw-HTML precedence over link brackets** (93.9% →
    94.9%, Links 78→84): `find_matching_close` now skips over any code span,
    autolink, or raw HTML beginning inside the brackets so a `]` within one
    can't close the link (`src/inline.rs`, examples 342/524/525/526/536/537/538).
  - **Unmatched backtick run is literal** (94.9% → 95.1%, Code spans 22/22):
    an opening run of N backticks with no closing run of exactly N is literal;
    skip the whole run so a shorter sub-run can't re-open (example 347).
  Perf gate held throughout (verified against a same-session baseline:
  `find_matching_close` change within ~3%, others render/block-only).
  Remaining 32: link *nesting* (518/519/520/532/533), block containers
  (tabs 8, blockquotes 5, lists 9, HTML-in-container 2), misc (540 case-fold).
  (tabs, lists, blockquotes).
