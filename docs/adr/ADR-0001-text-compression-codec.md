# ADR-0001 — The WAX text-compression codec

**Status:** Accepted
**Date decided:** 2026-10-13 (Directive 04) · **Implemented:** 2026-10-14 (SPEC v0.10, Directive 05)
**Owner:** Track A (the `.wax` container format)
**Supersedes:** the per-entry-only framing of SPEC v0.9 §3

---

## Context

WAX exists to beat ZIM. On size it did not.

Directive 01's x86 leg measured built packs against their source archives and
found WAX **larger** than the ZIM it replaced — and worst exactly where the
content was most textual:

| Archive | text share | WAX ÷ ZIM |
|---|---:|---:|
| `wikipedia_en_chemistry_mini` | 96.4% | **1.47×** |
| `wikipedia_en_history_maxi` | 44.8% | **1.12×** |

That correlation is a fingerprint, not noise. SPEC v0.9 §3 stores each entry as
its own zstd frame; ZIM compresses *clusters* of articles together, amortising
one dictionary across thousands of small, highly similar documents. On 58,257
chemistry articles averaging ~3.5 KB, grouping wins decisively.

So one decision inside the format — per-entry framing — was wrong for text. The
question was what to replace it with **without losing what per-entry framing
buys**, which Directive 01 had just measured as excellent:

- pack open **0.85 MB at 382,499 entries**, flat across a 41× range in entry
  count,
- **~1.29** index page reads per lookup, 2.1 µs mean,
- a full verify under a hard **64 MB** cap,
- and deltas that ship only the entries that changed.

## Decision

**Adopt arm 3: per-entry zstd against a corpus-trained shared dictionary,
stored once per pack.**

One zstd frame per entry, exactly as before — the dictionary is supplied to the
encoder and decoder, and nothing else about the blob layer changes. Landed as
SPEC **v0.10**, additively: one new `compression` value (`zstd-dict`) and one
new segment-0 table (`dictionary`).

### Options measured

Four arms, all three archives, real content on bare-metal x86:

1. **per-entry** — v0.9 today. The baseline.
2. **grouped** — ZIM-style blocks. The size reference.
3. **per-entry + shared dictionary** — the candidate.
4. **grouped + shared dictionary** — the absolute floor, to price what
   per-entry random access actually costs.

## Evidence

### Size — blob region, versus source ZIM

| Archive | text | 1 per-entry | 2 grouped | **3 pe+dict** | 4 grp+dict |
|---|---:|---:|---:|---:|---:|
| `chemistry_mini` | 96.4% | **1.246×** ❌ | 0.410× | **0.530×** | 0.384× |
| `en_100` | 22.5% | 0.159× | 0.157× | **0.157×** | 0.157× |
| `history_maxi` | 44.8% | **1.099×** ❌ | 0.952× | **0.971×** | 0.941× |

Arm 3 beats ZIM on all three. It sits **3.2% above** the absolute floor on the
large archive — that 3.2% is the entire byte price of keeping per-entry random
access. Whole-pack, counting the SQLite index, arm 3 is ~0.75× ZIM on
`chemistry_mini` and ~parity on `history_maxi`.

The dictionary is worth **2.35×** on `chemistry_mini` and ~nothing on `en_100`,
because its benefit scales inversely with entry size: small similar documents
have redundancy *between* them, large ones already have enough *within* them.

### Delta — and where the stated hypothesis was wrong

Directive 04 predicted grouping would bloat updates. Against two real editions
(`chemistry_mini` 2026-04 → 2026-07) it did the **opposite** — grouped deltas
came out at 0.40× of per-entry. That looked like evidence for grouping.

The churn sweep explained it. Holding content fixed and varying only how much
moved:

| churn | 1 per-entry | 2 grouped | **3 pe+dict** | grouped ÷ per-entry |
|---:|---:|---:|---:|---:|
| 0.5% | 163,653 | 9,672,376 | **73,526** | **59×** |
| 1% | 319,889 | 10,093,774 | **137,287** | **32×** |
| 2% | 616,287 | 10,155,239 | 252,029 | 16× |
| 10% | 3,163,846 | 10,155,239 | 1,318,951 | 3.2× |
| 40% | 10,199,807 | 10,155,239 | 4,265,854 | 1.0× |

The real-edition pair had **37.6% churn**, which dirtied **98.9% of blocks** —
at that rate you are rebuilding, and a rebuilt block legitimately beats its
entries framed singly. Below ~40% churn, which is where every realistic update
lives, grouping ships **16–59× more**: even 0.5% churn dirties 95.6% of 1 MiB
blocks, because one changed article in 200 touches nearly every block.

**Arm 3 is the smallest delta at every churn level measured.**

### Read path — the constraint

| | per-entry (v0.9) | **per-entry + dict** | whole-block |
|---|---:|---:|---:|
| Single-article decode | 3.1 µs | **2.2 µs** | 26.3 µs |

Arm 3 decodes **faster** than v0.9 — the frame is smaller, so there is less to
inflate. Grouping costs **8.4×** to reach one article.

## Consequences

**Good.** WAX now beats ZIM on size for the first time, without surrendering any
of the read-path or delta advantages Directive 01 measured. The index, the
lookup path, the append protocol and the signing model are untouched.

**The load-bearing obligation.** The dictionary **must be trained once and
carried forward across editions.** Retraining reframes every entry, so an
update would ship the whole archive rather than the changed entries — which
destroys the very property that made arm 3 win. SPEC §5.9 makes carry-forward
normative; `wax-core` enforces it with a test and a negative control.

**A determinism condition, found by testing rather than assuming.** zstd's
trainer is deterministic for a given sample *sequence* but is **not
order-independent** — the same samples reversed produce a different dictionary.
Training samples must therefore be selected and ordered by a content-derived
rule (entry path order), never filesystem or insertion order. Unchecked, this
would have been a silent reproducibility failure visible only across machines.

**A security placement.** The dictionary lives in the index segment, not the
blob region, because the signing digest is `SHA-256(header ‖ index-chain)`.
In the blob region it would sit outside the signature — and since every
dict-framed entry depends on it, an attacker could alter the decoded content of
the whole archive with the signature still verifying.

**Costs.** Readers must hold ~110 KB per open pack. A v0.9 reader can open and
list a v0.10 pack but cannot read its dict-framed entries — the correct failure,
reported as a distinct error rather than a plain-decode attempt that would yield
silent garbage.

**Not chosen, and why.** Grouping (arms 2 and 4) is smaller at rest but costs
8.4× on single-article decode and 16–59× on realistic deltas. Those are the two
properties WAX is *for*. Buying 3.2% of size with them would be a bad trade.

## Rejected alternative: reopen the format

Considered explicitly — the format had, after all, just lost to ZIM on its
headline metric. Rejected on evidence: the read path was measured excellent, the
deficiency was traced to one decision inside the blob layer, and that decision
turned out to be replaceable additively. Arm 3 reaching parity is precisely the
result that ruled a rewrite out; had **no** arm beaten ZIM on text, container
overhead rather than framing would have been implicated, and reopening the
format would have been justified.

## References

- Directive 04 measurement run — commit `f300306`, and its git note.
- Harnesses: `crates/wax-core/examples/codecbench.rs`, `deltabench.rs`,
  `dictcheck.rs`.
- SPEC v0.10 §3 (codec values) and §5.9 (the `dictionary` table, carry-forward
  and determinism rules) — commit `0e1e2fb`.
