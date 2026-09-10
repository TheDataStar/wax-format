# docs/

Authoritative DeltOS design documents, kept **in the repo** so no implementation
pass has to work from pasted fragments. See track-a-refinement §17 for why this
matters.

| File | What it is |
|------|------------|
| `track-a-refinement.docx` | The Track A Refinement doc (v2, reviewed) — original binary, byte-for-byte as supplied. Source of truth. |
| `track-a-refinement.md` | Plain-text extraction of the same document: greppable, diffable, no binary reader needed. Section numbering (§1–§17) and all three tables preserved. |

`track-a-refinement.md` is **generated** from the `.docx` by
`docs/extract-docx.py`. If the `.docx` is updated, re-run:

```bash
python docs/extract-docx.py docs/track-a-refinement.docx docs/track-a-refinement.md
```

and commit both. Never hand-edit the `.md` — corrections belong in the source
document, which is what review sign-off applies to.

## Relationship to SPEC.md

`SPEC.md` (repo root) is the byte-exact **implementation contract** for WAX
format v0.9. The Refinement doc is the **design rationale** behind it. Where the
Refinement doc is silent or self-contradictory, its own §15 and §16 record how
the ambiguity was resolved, and SPEC.md carries the resolution.
