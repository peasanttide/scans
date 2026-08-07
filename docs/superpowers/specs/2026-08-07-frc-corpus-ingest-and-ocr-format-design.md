# The Newberry French Revolution corpus — ingest, and a TOML format for OCR

Status: implemented
Date: 2026-08-07
Revised: 2026-08-07, after the design met the corpus. The corrections are marked
**Corrected** and the original claim is stated alongside, because most of them were
wrong in a way worth remembering.

## Purpose

Bring the Newberry Library's French Revolution Collection into this archive: 38,377
Internet Archive items of French pamphlets, mostly 1789–1799, listed by
[NewberryDIS/frc-data](https://github.com/NewberryDIS/frc-data). For each item, hold the
scanned PDF and the word-level OCR, addressed the same way as everything else here.

That is a factor of roughly seven thousand more records than the archive currently holds.
The design is shaped almost entirely by that number.

## What frc-data actually is

**Corrected.** The first draft of this said frc-data held no XML OCR, and that the
coordinate-bearing OCR would have to be fetched from archive.org. That was wrong, and it
was wrong because GitHub's tree API silently truncates: it returned 72,499 entries with
`"truncated": true`, and the counts happened to add up so neatly against two directories
that the third was never missed. `git ls-tree` on a blobless clone gives the real list.

There are four directories, and each of the three that matter holds exactly 38,377 files:

| directory | contents |
| --- | --- |
| `Metadata/*_meta.xml` | 38,377 Internet Archive metadata records |
| `XML_for_OCR/*_djvu.xml` | 38,377 DjVu XML files — **word-level OCR with coordinates** |
| `OCR_text/*_djvu.txt` | 38,377 plain-text OCR files |
| `IA_pamphlets-1560-1660-marc_records/` | one 3.5 MB MARC file for a different collection |

So frc-data supplies the item list, the catalogue metadata **and the OCR**. Only the PDFs
have to be fetched.

That changes the crawl from about 115,000 requests to about 77,000, and saves archive.org
11 GB of egress for bytes that are already sitting in a git repository. Given that "be
respectful of rate limits" was the constraint, this is not a small correction.

## Scale, measured

Sampled across three items and extrapolated by OCR-text size, which tracks page count
closely:

| | |
| --- | --- |
| items | 38,377, all with OCR |
| PDFs | ~63 GB, still an estimate — the fetch has not finished |
| `XML_for_OCR` in frc-data | 11 GB **measured** |
| the same OCR as markdown | ~1.2 GB — text only, no coordinates |
| HTTP requests | ~77,000, all of them for PDFs |
| wall time at one request per second | ~21 hours |

The wall time is the governing constraint. This is a pipeline that runs for days and is
killed and resumed many times, not a script that is run once.

## Decisions

### One IA item becomes one `source` record

`layer = "source"`, with `copy` and `document` collapsed in, exactly as `pellet-1873` and
`turgot-1739` already do. A pamphlet is a single work, digitised once, and paying three
files of ceremony for it would produce 115,000 files to describe 38,377 pamphlets.

`type` comes from `mediatype` and `physical_description`: `pamphlet` under ~50 pages,
`book` above, `periodical` where the metadata says so.

### Ids are the bare Internet Archive identifier

`abondance00unse`, not `frc-abondance00unse`. The identifier is already globally unique
and stable, and it is what every other system cites. Prefixing would add nine characters
to 38,377 addresses to solve a collision problem that does not exist — the existing
hand-curated ids are all of the form `name-year`, which no IA identifier matches.

### Directories are sharded two deep by identifier prefix

```
sources/frc/ab/abondance00unse/
                 abondance00unse.toml
                 abondance00unse.pdf
                 abondance00unse.ocr.toml
```

~700 shard directories of ~55 items each. The README's promise that "directories are
cosmetic" is what makes this free: no address changes if the sharding is ever redone.

A single flat directory of 38,377 entries was rejected — not because git minds, but
because `ls`, tab completion, and the editor's file tree all become unusable.

### The OCR sidecar attaches through the existing `[[text]]` table

No new top-level table. `[[text]]` already means "OCR or transcription, never inline",
which is exactly what this is:

```toml
[[text]]
file = "abondance00unse.ocr.toml"
kind = "ocr"
by   = "ABBYY FineReader 11.0"
lang = "fr"
```

## The OCR format

**Superseded twice.** It was TOML carrying packed word-level arrays when this was written; it
is now markdown with YAML frontmatter carrying no coordinates at all. Both changes were at the
repository owner's direction, and the second is the larger one.

### Where it landed

One file per scanned page, `<id>.pN.ocr.md`:

```markdown
---
of: 1789iemilseptcen00unse
page: 6
engine: ABBYY FineReader 11.0
lang: fr
w: 2597
h: 4418
dpi: 500
---

<the page's text>
```

Frontmatter says which page of what and how big the image is; the body is the page. That is
the whole format.

### What was dropped, and what that costs

Gone: word boxes, per-word confidence, baselines, region background colours, and the `brk`
encoding that reconstructed DjVu's four levels of nesting in one byte per word. With them goes
the losslessness claim and `scans ocr export`, which existed to make that claim executable.

The reason is proportion. The coordinates were about seven eighths of the bytes — 8.3 GB
against roughly 1.2 GB for the text — and nothing was reading them.

**Nothing is destroyed.** The coordinates are in `XML_for_OCR/` in frc-data, which is one
clone away and needs no request to archive.org. Reinstating them is a change to
`tools/src/djvu.rs` and a re-run of `frc ingest`, not a re-fetch.

### The per-page split

Before the coordinates were dropped, one file per *item* put a 3,378-page bound run of nine
volumes into a single 49 MB text file. Splitting per page fixed that, and kept two properties
worth naming even now that files are small:

- `<id>.pN` matches the address grammar the archive already uses for pages.
- The body needs no `## Page N` headings, so nothing in the OCR can be mistaken for a
  delimiter. The per-item version had to carve the body back up by markers the OCR could in
  principle contain.

### Bare scalars, and the YAML version that matters

Values are written bare — `lang: fr` — because the frontmatter is meant to read as YAML.

The hazard is that `no` is the ISO 639-1 code for Norwegian and, in **YAML 1.1**, the boolean
false. `serde_norway` implements **1.2**, where it is already a string, so a round-trip check
against this crate's own parser passes it. That check is not the right bar: PyYAML, libyaml,
Psych and gopkg.in/yaml.v2 are all 1.1, and this data is published for their tools rather than
ours. So there is an explicit 1.1 deny-list *and* a round-trip check, and a test asserts that
every reserved word survives as the string it was.

## New record fields

**Corrected.** The draft proposed a fourth, an `[ocr]` table pointing at the sidecar. It
was not needed: `[[text]]` already means "OCR or transcription, never inline", which is
exactly what a sidecar is. Three remain, each justified independently of this ingest:

| field | why |
| --- | --- |
| `subject` — array of strings | 38,377 pamphlets are unusable without subject access. LCSH terms, from IA's `<subject>`. |
| `extent` — string | The bibliographic collation, `"7 p. ; 18 cm."`, verbatim. Distinct from `scan.count`, which counts scanned images including covers. |
| `scan.ppi` — integer | The scan's own resolution. `render` currently infers native resolution from the largest embedded image; this states it. |

`subject` and `extent` go on all three record variants and do not inherit. `ppi`
inherits, like the rest of `[scan]` except `count` and `note`.

## Metadata mapping

| IA field | record |
| --- | --- |
| `title` | `title` |
| `date`, `dfate` (a real IA typo, present in some records) | `date`, as EDTF |
| `creator`* | `[[resp]]`; a trailing `, printer` inside the name becomes `role` |
| `publisher` | `place` (before the first ` : `) and a `[[resp]]` with `role = "publisher"` |
| `language` | `language`, ISO-639-2 → BCP-47 (`fre` → `fr`) |
| `contributor`, `call_number`, `collection` | `[holding]` repository, shelfmark, collection |
| `identifier`, `identifier-ark`, `openlibrary_edition`, `openlibrary_work` | `[identifier]` |
| `link_to_catalog`, `identifier-access` | `[[link]]` with `rel` `catalogue`, `viewer` |
| `imagecount`, `ppi`, `scanner`, `camera`, `scandate`, `scanningcenter` | `[scan]` |
| `subject`* | `subject` |
| `physical_description` | `extent` |
| `description`*, `notes`, `citation` | `note` |
| `sponsor` | `rights.attribution` |

Rights are constant across the corpus: the works are of the 1790s and long out of
copyright; the digitisation is the Newberry's, sponsored and public.

## The pipeline

A new `scans fetch` subcommand, behind a `fetch` feature so the default build stays free
of an HTTP stack.

**Resumability** is the first requirement, not an afterthought. **Corrected:** the draft
specified a JSONL ledger. There is none. An item is done when its file is on disk, which is
the whole resume condition — a ledger would be a second source of truth that can fall out
of step with the tree it describes, bought at the price of a file to keep consistent.
Downloads are written to `.pdf.part` and renamed, so a run killed mid-download cannot leave
a half PDF that the next run mistakes for a finished one.

**Politeness** is the second:

- one request in flight at a time, minimum interval configurable, default 1s
- a `User-Agent` naming the project and its repository, so the operator can be contacted
- `Retry-After` honoured on 429 and 503
- exponential backoff on 5xx, capped, then the item is parked rather than blocking the queue
- `--limit` and `--since` so a run can be bounded

**Verification**: every download is checked for size and MD5 against the values in the
item's IA metadata before it is accepted. A truncated PDF must not be committed and then
believed.

**Disposal**: `_djvu.xml` is converted to `.ocr.toml` and deleted. Keeping both would
double 12 GB to hold the same facts twice, and the TOML is the archival form.

### Storage

| what | where | why |
| --- | --- | --- |
| `*.pdf` | Git LFS | ~63 GB, already compressed, no benefit from packfiles |
| `*.ocr.toml` | plain git | text, packs about 5:1; LFS would bill for the full 7 GB |
| `*.toml` records | plain git | small |

63 GB of LFS exceeds GitHub's free tier and needs data packs on the remote before the
first push. Pushing proceeds in commits of a few hundred items rather than one enormous
commit, so an interrupted push loses minutes rather than days.

## Validation

Check 11, four codes:

- **E708** a `[[text]]` pointing at a sidecar that is not there
- **E709** a sidecar that is not valid TOML for the OCR model
- **E710** a sidecar whose `of` names a record other than the one pointing at it
- **E711** any of the internal inconsistencies in `ocr::Problem` — arrays of unequal length,
  `box` not exactly four per word, `region-bg` not one per region, `text` that disagrees
  with `word` and `brk`

**Corrected.** The draft said these "run over the whole archive by default, since they read
only TOML". That reasoning was wrong: the distinction is not TOML versus bytes, it is how
many gigabytes of them there are. Parsing 8 GB of sidecars turns `validate` from a
2.3-second command into a 2m37s one. E708 is a `stat` and runs always; everything that has
to parse a sidecar is behind `--probe`, for exactly the reason the image checks are.

Result on the full corpus: 38,715 records, **0 errors, 0 warnings** by default, and 0 errors
with `--probe` across all 38,377 sidecars.

One further correction fell out of the first full run: `id_is_preferred` treated `_` as
outside house style, which made 1,263 warnings out of identifiers like `procesverbal00_1_0`
and `case_oversize_frc_27598`. Those are the handles archive.org publishes and everyone else
cites. Underscore is now a group separator alongside hyphen — 1,263 warnings a reader cannot
act on is how a warning channel stops being read.

## Testing

- djvu.xml → TOML → djvu.xml round-trip over a sample of real fetched items
- the `brk` encoder and decoder, against hand-built nestings including the degenerate
  cases: an empty page, a page of one word, a word containing a space
- metadata mapping over a fixture set chosen for awkwardness: missing `date`, the `dfate`
  typo, multiple `creator` entries, a creator with an embedded role, no `publisher`
- ledger resume: interrupt mid-run, restart, confirm no item is fetched twice and none is
  skipped
- rate limiter timing, with a clock injected rather than by sleeping

## Deliberately not done

- **No `copy` or `document` layer per item.** Revisit only if an item turns out to bind
  several distinct works, which the metadata does not currently distinguish.
- **No text search index.** Grep over 7 GB is adequate; a real index is a different
  project with a different lifetime.
- **No re-OCR.** ABBYY 11 on 500 ppi scans of worn 18th-century type is mediocre, and
  visibly so in the confidence figures. Improving it is worth doing and is not this.
- **No MARC ingest.** The 3.5 MB MARC file covers a different, earlier collection
  (1560–1660 pamphlets) than the one being ingested.
