# Primary source & scan archive — schema design

Status: approved design, not yet implemented
Date: 2026-08-05

## Purpose

This repository is an archive of primary sources from the French Revolution and the
scans of them. Its product is **stable, citable addresses**. Structured facts and
translations live in a separate repository, which cites into this one. Everything in
this schema exists to make those addresses resolvable, unambiguous, and correct.

The game consumes three things through those addresses: pixels (crops, map tiles),
OCR'd text, and quotes with citations.

## Diagnosis of the current layout

The existing tree works but does not scale, for three specific reasons.

**`kind` mixes three dimensions into one field.**

| current `kind` value | what it actually says |
|---|---|
| `newspaper`, `map` | genre of the publication |
| `volume` | this is a physical object |
| `issue`, `supplement` | type of the intellectual unit |

The chain of `of = ...` therefore looks uniform while meaning something different at
every level.

**A layer is missing.** The Lausanne bound volume — shelfmark 1094184846, scanned by
Google, front matter trimmed — is not the same thing as "Journal de Paris, 1789,
volume 1" the publication. One is paper in Switzerland; the other is an editorial
fact. `journal-de-paris-1789-vol1.toml` fuses them, which is why it carries a prose
comment explaining that its page numbers are off by one. That comment is a schema gap.

**Inherited data is copied by hand.** Every issue file repeats its parent's licence,
attribution, and source PDF. Every Turgot sheet file repeats the author, date,
licence, and attribution of the atlas. `author = "Louis Bretez (survey and drawing);
Claude Lucas (engraving); Aubin (lettering)"` is a string no program can parse,
duplicated 21 times.

## Settled decisions

These were decided during design and are not open:

1. **Four layers**: `source`, `copy`, `document`, `page`.
2. **Flat ids.** Ids never contain `/`. `journal-de-paris-1789-vol1`, not a path.
3. **One file per document.** File grain stays as it is today; the fix for
   repetition is inheritance, not consolidation.
4. **Pages are not entities.** They get no ids of their own, no special naming
   scheme, and no files of their own. A page is declared inline as `[[page]]` and
   addressed as `<id>.p<n>`.
5. **`.pN` is document-local.** `n` is scoped to the document that declares it.
6. **EDTF dates** (ISO 8601-2:2019), always quoted strings, always Gregorian.
7. **Every page states both its numbers**; counting is the default; the validator
   errors when the numbers do not fit.
8. **All sources live under `source/`.**

Rejected along the way: text-span and named-region citation grain (too much
bookkeeping for the benefit); consolidating many documents into one file per volume;
a flat relational layout; Republican-calendar machinery.

## The model

### Four layers

Named after TEI equivalents where TEI has a good word.

| `layer` | TEI analogue | what it is | example id |
|---|---|---|---|
| `source` | `teiCorpus` / `sourceDesc` | the publication or work, independent of any copy | `journal-de-paris` |
| `copy` | `witness` / `msDesc` | one physical object that was digitized | `journal-de-paris-1789-vol1` |
| `document` | `msItem` | the citable intellectual unit | `journal-de-paris-1789-01-03` |
| `page` | `surface` | one scanned side, carrying graphics | addressed `…-01-03.p1` |

`graphic` (TEI `<graphic>`) is an image file. `zone` (TEI `<zone>`) is a rectangle on
a page, available for crops but not required.

Two words were deliberately retired. **`collection`** is already taken in TEI, where
`<collection>` means the named collection *within a holding repository* ("the David
Rumsey Map Collection") — using it for the publication burns the word needed to record
where a copy is held. **`issue`** is a serials word that cannot honestly describe a map
sheet, a play, or a letter; it is one `type` of `document`, not the category itself.

### Two structural fields replace `kind`

- **`layer`** — which of the four. Closed set, structural, always stated.
- **`type`** — genre within the layer. `newspaper`/`map`/`diary` on a source;
  `issue`/`supplement`/`sheet`/`letter`/`play`/`engraving` on a document.

`type = "issue"` is no longer ambiguous because `layer = "document"` has already said
what kind of thing is being typed.

### Layers may be skipped

`of` points at *any* ancestor, not necessarily the layer directly above. When a layer
is skipped, its fields collapse into the nearest declared ancestor.

This is load-bearing, not a convenience. Most sources in this archive are one-off
engravings and pamphlets, and they must not pay four files of ceremony. Journal de
Paris genuinely needs the `copy` layer — two volumes, two shelfmarks, two PDFs, 181
issues sharing one file. Turgot does not: one atlas, one digitization, so the Rumsey
information sits on the source and pages address as `turgot-1739.p0`.

The four layers are a vocabulary, not a required depth.

## Identity and addressing

- Every `source`, `copy`, and `document` has an `id`.
- `id` defaults to the filename stem and must be globally unique across the repo.
- Ids never contain `/`; `:` and `.` are reserved as separators.
- Pages have no `id`. A page is `<owner-id>.p<n>` where `n` is the page's `n` field
  within the document (or other owner) that declares it.

```
journal-de-paris-1789-01-03        a document
journal-de-paris-1789-01-03.p1     its first page
turgot-1739.p0                     the atlas key sheet
```

Because ids are flat and location-independent, **directories are cosmetic**. Folders
may be reorganized at any time without breaking a single citation in the consuming
repository. The `1789/01/03/` tree exists because it is pleasant to browse.

The per-sheet `turgot_NN.toml` and `verniquet_NN.toml` files are folded into their
source files and deleted, so underscores survive only on the `.jp2` filenames — where
they are harmless, since a graphic is named explicitly by `graphic.file` and nothing
parses it. The 94 image files are not renamed; they are in LFS and their names carry no
meaning.

## Repository layout

```
source/
  journal-de-paris/
    journal-de-paris.toml                     source
    1789/
      journal-de-paris-1789-vol1.pdf
      journal-de-paris-1789-vol1.toml         copy
      journal-de-paris-1789-vol2.pdf
      journal-de-paris-1789-vol2.toml         copy
      01/03/
        journal-de-paris-1789-01-03.toml            document
        journal-de-paris-1789-01-03-supplement.toml document
  turgot/
    turgot-1739.toml            source; copy collapsed in, 21 pages inline
    turgot_00.jp2 … turgot_20.jp2
  verniquet/
    verniquet-1795.toml         source; copy collapsed in, 73 pages inline
    verniquet_00.jp2 … verniquet_72.jp2
  engravings/
docs/
```

## File format

### Source

```toml
id        = "journal-de-paris"
layer     = "source"
type      = "newspaper"
title     = "Journal de Paris"
language  = "fr"
place     = "Paris"
founded   = "1777-01-01"
frequency = "daily"
note      = "First daily newspaper published in France."

[rights]
work = "PD-old-100-expired"

[[link]]
rel = "index"
url = "https://gazetier-revolutionnaire.gazettes18e.fr/periodique/journal-de-paris-1799"
```

### Copy

```toml
id     = "journal-de-paris-1789-vol1"
layer  = "copy"
of     = "journal-de-paris"
title  = "Journal de Paris, année 1789, volume 1 (janvier–juin)"
covers = "1789-01-01/1789-06-30"

[scan]
file  = "journal-de-paris-1789-vol1.pdf"
count = 888
by    = "Google Books"
url   = "http://books.google.fr/books?id=wjkTAAAAQAAJ"
note  = "Google front-matter leaf removed; graphic page indices are 1-based into the trimmed PDF."

[holding]
repository = "Bibliothèque cantonale et universitaire, Lausanne"
shelfmark  = "1094184846"

[identifier]
google_books = "wjkTAAAAQAAJ"

[rights]
work        = "PD-old-100-expired"
attribution = "Digitised by Google Books."
```

### Document

Terse form, which is what 99% of issues will use:

```toml
id    = "journal-de-paris-1789-01-03"
layer = "document"
of    = "journal-de-paris-1789-vol1"
type  = "issue"
no    = 3
date  = "1789-01-03"
pages = { from = 13, to = 16 }
```

`pages = { from, to }` expands by counting into four pages: `n` = 1…4, graphic page =
13…16, graphic file inherited from the copy's `scan.file`. Nothing is repeated from the
parent.

Supplements point at an id rather than a bare integer:

```toml
id    = "journal-de-paris-1789-01-03-supplement"
layer = "document"
of    = "journal-de-paris-1789-vol1"
type  = "supplement"
supplement_to = "journal-de-paris-1789-01-03"
date  = "1789-01-03"
pages = { from = 17, to = 20 }
```

A one-off with no parent at all — this is the shape most of the archive will take:

```toml
id    = "serment-du-jeu-de-paume-1791"
layer = "document"
type  = "engraving"
title = "Le Serment du Jeu de Paume"
date  = "1791"

[[resp]]
name = "Jacques-Louis David"
role = "artist"

[[page]]
n = 1
[[page.graphic]]
file   = "serment-du-jeu-de-paume.jpg"
width  = 4000
height = 2800

[rights]
work = "PD-old-100-expired"
```

### Page

**Pages are always declared inline**, as `[[page]]` within the file that owns them.
There is no such thing as a page file. `page` remains one of the four layers, but it is
the one layer that is never a file of its own — which means `layer` on a file is only
ever `source`, `copy`, or `document`.

Map sheets are the case that tempts you the other way, and they are exactly where inline
pays off: Turgot's 21 sheets differ only in `n`, `title`, and their graphic, so as a
single array they read as a table and diff as a table. Twenty-one files, each repeating
`of` and `layer`, would show the same information as twenty-one separate diffs.

```toml
# source/turgot/turgot-1739.toml — continues the source file shown above

[[page]]
n     = 0
title = "key sheet"
[[page.graphic]]
file   = "turgot_00.jp2"
width  = 23964
height = 16934
url    = "https://www.davidrumsey.com/rumsey/download.pl?image=/166/10059022.jp2"

[[page]]
n = 1
[[page.graphic]]
file   = "turgot_01.jp2"
width  = 23964
height = 16934
```

Addresses are `turgot-1739.p0`, `turgot-1739.p1`, and so on. Turgot states `n = 0`
explicitly because the atlas numbers its own sheets from zero; the counting default
starts at 1, and any owner that numbers itself differently says so on its first page.

### Responsibility

`[[resp]]` replaces the unparseable author string, stated once on the source and
inherited by all 21 sheets:

```toml
[[resp]]
name = "Louis Bretez"
role = ["surveyor", "draughtsman"]
[[resp]]
name = "Claude Lucas"
role = "engraver"
[[resp]]
name = "Aubin"
role = "lettering"
```

### Dates

Every date field is a **quoted EDTF string**, never a TOML native date. Unquoted `1739`
is a TOML integer and unquoted `1789-01-03` is a TOML local date, but `1789?`, `178X`,
and `1791/1799` are neither — mixing types by how certain a date happens to be would be
miserable to consume.

Dates are always Gregorian. Where a document prints a Republican date, it is converted;
if the conversion is uncertain, EDTF says so.

| EDTF | meaning |
|---|---|
| `"1789-01-03"` | that day |
| `"1739"` | that year |
| `"1791/1799"` | interval |
| `"1795~"` | approximate |
| `"1789-01?"` | uncertain |
| `"178X"` | unspecified digit |
| `"../1789-06-30"` | open start |

### Links and identifiers

`url` is the canonical landing page. `[[link]]` with a `rel` carries everything else,
retiring the ad-hoc `fetch` and `index` fields. `[identifier]` holds catalogue
identifiers, retiring `google_books_id`.

### Text and OCR

Page-grain, matching the citation grain, and never inline in TOML:

```toml
[[text]]
file = "journal-de-paris-1789-01-03.p1.txt"
kind = "ocr"          # or "transcription"
by   = "tesseract 5.3 fra"
lang = "fr"
```

`by` exists because the archive will be re-OCR'd eventually, and the pages still on a
bad pass must be identifiable.

## Inheritance

A file states only what is new or different. A resolver walks the `of` chain and fills
in the rest.

Inheritance is an **allowlist**, not a denylist:

| inherits | never inherits |
|---|---|
| `[rights]`, `[holding]`, `[identifier]` | `id`, `of`, `layer`, `type`, `title`, `short_title` |
| `[[resp]]`, `language`, `place` | `date`, `n`, `no`, `note` |
| `scan.by`, `scan.url`, `scan.file` | `scan.count`, `scan.note`, `pages`, `covers` |
| | `url`, `[[link]]`, `[[page]]`, `[[text]]` |

`scan.file` inherits so that a document's terse `pages = { from, to }` form knows which
PDF it is indexing into. `url` and `[[link]]` do not inherit: every layer has its own
landing page, and silently borrowing a parent's would misattribute it.

An allowlist because the failure mode matters. An over-inherited `date` would silently
stamp every issue with `1789` and nothing would error.

Merge semantics:

- **Scalars** — child wins.
- **Tables** — merged key by key; child's key wins.
- **Arrays** (`[[resp]]`, `[[graphic]]`) — child **replaces** wholesale
  rather than appending. Replacement is chosen so a wrong inherited value is always
  correctable; restate the entries worth keeping, or write `resp = []` to clear.

## Pages and graphics

Every page states two numbers, and there is no arithmetic anywhere:

- `page.n` — what page it is, within its document. This is what `.pN` matches.
- `graphic.page` — what page it is within the graphic file. Omitted when the graphic
  is a standalone image rather than a multi-page container.

```toml
[[page]]
n = 1
[[page.graphic]]
file = "journal-de-paris-1789-vol1.pdf"
page = 13
```

**If not specified, assume counting.** `pages = { from = 13, to = 16 }` expands to
`n` = 1,2,3,4 against graphic pages 13,14,15,16. A page that omits `n` continues the
count from the previous page.

**The validator errors when the counting does not fit** — a range whose length
disagrees with an explicit page list, a graphic page beyond the copy's `scan.count`, a
duplicated `n`. Counting is a convenience, never a guess that is allowed to stand.

## Validation

The linter is load-bearing here, because document-local `.pN` addressing was chosen
knowing it gives up eyeball-verifiable citations. These checks buy that back:

1. Ids globally unique; every `of` resolves; no cycles in the `of` chain.
2. Child `layer` at or below parent's, in the order `source` > `copy` > `document` >
   `page`.
3. `n` unique within its owner.
4. **Sibling documents' `pages` ranges do not overlap** within a copy.
5. Every `pages` range fits inside the copy's `scan.count`.
6. Every `date` parses as EDTF, and a document's date falls inside its copy's `covers`
   interval.
7. Every `graphic.file` exists on disk, and `width`/`height` match the real file.
8. Cross-references (`supplement_to`, and similar) resolve to existing ids.
9. Counting expansions fit, per the rule above.
10. Gap report: missing issue numbers in a serial's expected run.

Checks 4 and 6 catch the error class that is no longer visible by eye — an issue whose
pages silently overlap its neighbour, or one filed under the wrong volume.

## Migration

The existing 99 TOML files are mechanically convertible; nothing needs re-research.

An earlier draft of this spec claimed 516, on the mistaken belief that the Journal de Paris
daily issues had been transcribed. They have not. The archive holds 3 Journal de Paris
records (the serial and its two volumes), 22 Turgot (atlas + 21 sheets) and 74 Verniquet
(atlas + 73 sheets). The `1789/MM/DD/` directories exist but are empty — a walk of the
directory tree looks populated, which is where the wrong number came from. Entering the
issues is data work still to be done, and the schema is built to receive it.

1. Move everything under `source/`.
2. Add `layer` to every file; split the current `kind` value into `layer` + `type`.
3. Hoist `licence`, `attribution`, `author`, and `url` out of leaf files into the
   nearest ancestor that owns them; delete the copies.
4. Convert `author` strings to `[[resp]]` records. This is the one step needing
   judgment, and there are only a handful of distinct strings.
5. Convert `date` values to quoted EDTF.
6. Rename `source = "…vol1.pdf"` on documents to inherited `scan.file` on the copy.
7. Convert `pages = "13-16"` to `pages = { from = 13, to = 16 }`.
8. Fold the 21 `turgot_NN.toml` and 73 `verniquet_NN.toml` files into `[[page]]`
   arrays in `turgot-1739.toml` and `verniquet-1795.toml`, then delete them. Each
   contributes `n`, `title`, and one `[[page.graphic]]` carrying `file`, `width`,
   `height`, and `url`; their repeated `author`, `date`, `licence`, `attribution`,
   and `of` are dropped as inherited. The `.jp2` files are not renamed.
9. Run the validator; fix what it finds.

## Deliberately excluded

- **Zones/crops as first-class citable entities.** A crop is a page plus a rectangle;
  the page is the stable address and the rectangle is the consumer's business. `zone`
  is available but nothing requires it.
- **Structured facts, entities, and translations.** These live in the consuming
  repository and cite into this one.
- **Text spans.** Citation grain is the page.
- **Generated titles.** Titles stay hand-written strings for now; a `title_pattern`
  on the parent is a possible later optimization, not part of this design.
- **Standalone page files.** An earlier draft let a page be either inline or its own
  file. With Turgot and Verniquet inline, nothing in the archive used the file form,
  so it was removed rather than left as an unused second code path. Reinstating it
  later costs one loader branch.

## Open question for review

"All sources go in a source subfolder" is implemented above as a single top-level
`source/` directory containing `journal-de-paris/`, `turgot/`, `verniquet/`, and
`engravings/`. If the intent was instead that each source keeps its own folder at the
repo root (already true today), that changes step 1 of the migration only.
