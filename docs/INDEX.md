# DeltOS Design Documents

Authoritative design documents for the DeltOS / WAX project. **Markdown is canonical**
— these files are the source of truth an implementer reads. Word/PDF renderings exist
for human reading only and are generated from the same source; if they ever disagree
with these files, these files win.

Read `cross-track-contract.md` before any other document.

## Reading order for an implementation task

1. **`cross-track-contract.md`** — Normative values for every vocabulary more than one
   track consumes: hardware tiers and their floors, deployment profiles and their
   mapping, role strings, the progress-event schema, service/install state enums,
   origin and hostname patterns, filesystem paths, language/version/license formats.
   **Where this and a track document disagree, this document wins.** A track document
   restating one of these values is descriptive, not binding.
2. The track document you are implementing (below).
3. `implementability-sweep.md` — the known-defect register. Check whether your track
   has open findings before you start. The Contract’s §13 is **the enumerated list of blocking decisions with their answers**; §13.2 is the short list of what is still genuinely open.

## The documents

| File | Owns |
|---|---|
| `cross-track-contract.md` | Every shared vocabulary. Read first. |
| `track-and-phase-plan-v2.md` | The component inventory, deployment profiles, phase sequencing. **The inventory is stated by enumeration, never by a fixed total** — an earlier entry here read "A1–H10", which the catalogue has since outgrown. |
| `track-a-refinement.md` | The .wax container format, storage engine, signing. §15–§18 are implementation resolutions from real build passes. |
| `track-b-refinement.md` | Content & interop: ZIM conversion, crawling, manifests, catalog. §20 holds the zim2wax blockers. |
| `track-c-refinement.md` | The shell: launcher, privilege split, search, profiles, sandboxing, accessibility. |
| `track-d-refinement.md` | Search & intelligence: FTS, vectors, local LLM, RAG. |
| `track-e-refinement.md` | Hardware, OS, networking, updates, radio. |
| `track-f-refinement.md` | Admin, identity, roles, install pipeline, fleet. |
| `track-g-refinement.md` | Orchestration, reverse proxy, platform services. |
| `track-h-refinement.md` | The application catalog and its third-party services. |
| `design-language.md` | Visual and interaction system for the shell. |
| `implementability-sweep.md` | The known-defect register from the audit — a record of what was found, **not a to-do list**. Several of its Critical findings are already fixed in the Contract; check there before citing one as open. |
| `baseline.md` | Measured figures every later session compares against, and the prerequisites. Transcribed into the repo so nothing depends on a document outside it. |
| *a handoff PDF, if filed here* | **Reference only. Below every document above in precedence**, including this index. It records history; it decides nothing. |

## Ground rules for implementing from these

- **Never invent a value that belongs in the Contract.** If you need a tier floor, a
  role name, an enum member, a path or an identifier format and it is not in
  `cross-track-contract.md`, that is a defect report against the Contract — not a
  decision to make quietly. Stop and say so.
- **Stopping is correct.** Three earlier build passes were halted by missing
  specifications, and every one of those halts was the right call. A build that
  proceeds on an invented assumption costs more than one that stops.
- The Contract is new and has not been through a full implementation pass. Defects in
  it are expected; report them against it rather than working around them.

## Provenance

These files come from **upstream** — they are installed into this repo, not
authored in it. Do not edit them here: a correction belongs in the upstream
source, which is what review sign-off applies to, and the next install
overwrites local changes. The retired `.docx`-extraction pipeline that once
produced `track-a-refinement.md` is gone; markdown is now the canonical form
end to end.

## Relationship to `SPEC.md`

`SPEC.md` (repo root) is the byte-exact **implementation contract** for WAX
format v0.9. `track-a-refinement.md` is the **design rationale** behind it.
Where the Refinement doc is silent or self-contradictory, its own §15–§18
record how each ambiguity was resolved, and `SPEC.md` carries the resolution.
Where `SPEC.md` and `cross-track-contract.md` disagree, the Contract wins and
`SPEC.md` is the defect.
