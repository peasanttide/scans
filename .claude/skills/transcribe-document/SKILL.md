---
name: transcribe-document
description: Use when transcribing a scanned document in this archive into markdown - a Journal de Paris issue or supplement, a bound insert, a map sheet. Covers rendering the pages from the PDF, transcribing each page from the image with headings and tables preserved, proofreading, marking what could not be read, and recording the result as [[page]] and [[page.text]] entries in the document's .toml.
---

# Transcribing a document

You are producing a **faithful, readable transcription of what is printed on the paper**, one
file per page, cited from the document's record.

Transcribe from the **page image**. Do not paste the PDF's text layer: it is a bad OCR pass of
eighteenth-century type and it invents things. `NUMÉRO 5` is in there as `NUMERO 25`, and one
page reads `NUMERO 807179`. It is useful for finding a page and worthless for quoting one.

## What you produce

For a document `<id>` with N pages, in the directory the record already lives in:

```
<id>.p1.md … <id>.pN.md      one transcription per page
<id>.toml                    gains one [[page]] per page, each citing its .md
```

Nothing else changes. The markdown is never inlined into the TOML — the design spec settles
that ("Text and OCR ... never inline in TOML"), and it is what keeps the record diffable.

## The loop

### 1. Find the document

```bash
./tools/target/release/scans.exe resolve <id>
./tools/target/release/scans.exe resolve <id>.p1
```

The first prints the record's path and layer; the second adds the container and the page
inside it, which tells you the addressing is sound before you spend anything on it. If the
binary is missing, build it — `--release`, because a debug build is roughly thirty times
slower and you are about to rasterise several pages:

```bash
cd tools && cargo build --release --features render
```

### 2. Render every page of the document, all three ways, before transcribing any of them

```bash
./tools/target/release/scans.exe render <id>
./tools/target/release/scans.exe render <id> --grid 2x2
./tools/target/release/scans.exe render <id> --grid 4x4
```

All of it lands in `.render/`, which is gitignored. Re-running is cheap: anything already
rendered is left alone.

| what you get | what it is for |
|---|---|
| `<id>.p<n>.png` — the whole page | the **layout**: how many columns, where the rules are, which lines are headings, what order to read in |
| `<id>.p<n>.r<r>c<c>.png` at `2x2` — four tiles | **reading prose**. For a two-column page the vertical boundary falls near the gutter, so `r1c1`+`r2c1` is roughly the left column and `r1c2`+`r2c2` the right |
| the same at `4x4` — sixteen tiles | **anything you must get exactly right**: every numeral, table cells, the printed page number, a word you are not sure of |

Render all pages up front because **a word broken across a page is rejoined on the page where
it starts**, and the rest of it is on the next page's image. When a page ends mid-word, open
the next page's first tile, take the rest of the word, and finish the sentence on the page
that began it. Nothing is left on the following page.

You will be tempted to read numbers off the whole page. Do not. A whole page is about
3500x4900 and arrives sampled down until the type is a few pixels tall — display type and
table rules survive that, numerals do not. **Every digit you write down comes from a `4x4`
tile.** That is what it is there for, and misreading `28. 4, 0` as `28. 4. 6` is the failure
this transcription is most likely to contain.

### 3. Transcribe, one page at a time

Whole page for the shape of it, `2x2` tiles for the words, `4x4` tiles for anything exact.
Write `<id>.p<n>.md` beside the record. No front matter, no title you invented, no page number
heading — the file is the page, and the record says which page it is.

### 4. Proofread it

Read the tiles again against what you wrote. You are looking for the mistakes this task
actually produces:

- a word you guessed at from its shape rather than read
- `f` where the paper has a long s, and the reverse
- dropped accents, and accents added where the paper has none
- numbers in tables transposed or shifted a column — check these at `4x4`
- a line skipped where a tile boundary fell

Fix what you can. Where you cannot, mark it — see *Marking what you could not read*.

### 5. When every page is done, edit the record once

One edit to `<id>.toml`. See *Recording it in the .toml*.

### 6. Validate

```bash
./tools/target/release/scans.exe validate source/<path-to>/<id>.toml
```

Zero errors, zero warnings. `E706` means a `[[page.text]]` names a file that is not there —
usually a typo in the filename, or a `.md` written in the wrong directory.

## How to transcribe

### Transcribe, do not translate and do not modernise

The text stays in its own language and its own spelling. `connoître` stays `connoître`,
`étoit` stays `étoit`, `Poëme` keeps its diaeresis, `enfans` keeps no `t`. `&` stays `&` and
is not expanded to `et`. Æ and œ stay ligatured. Accents are as printed, including where
eighteenth-century practice differs from modern French (`théatre`, `trés`).

Three things are normalised, and only these three:

- **The long s.** `ſ` is transcribed `s`. It is the same letter in a different sort, not a
  different letter, and leaving it in makes the file unsearchable for no gain. `Ceſſer` is
  written `Cesser`.
- **Superscripts come down to the baseline**, keeping their letters: `Sr`, `Dlle`, `Dme`,
  `Md`, `Cher`, `ve`, `Mgr`, and the ordinals `1er`, `2d`, `4e`, `83e`. `Nº` is written `No.`
- **Spacing before punctuation is regularised** to one space before `;` `:` `?` `!`, and none
  before `,` or `.` — which is French practice and what the paper is doing, but the paper's
  spaces are thin and uneven and no two people would read them the same way twice.

Editorial marks you add are in English and in square brackets, so they can never be mistaken
for the document's own words.

### Preserve the structure with markdown

| on the page | in the file |
|---|---|
| the paper's masthead, first page of an issue only | `# JOURNAL DE PARIS.` |
| the issue number above it, and the date line below | bold, each on its own line |
| a department heading in large caps on a line of its own — `EXTRAITS.`, `ADMINISTRATION.`, `SPECTACLES.`, `VARIÉTÉS.` | `## EXTRAITS.` |
| the title of a piece, on a line of its own, under one of those | `### BELLES-LETTRES.`, `### RÉSULTAT du Conseil d'État du Roi…` |
| italic in the original — titles of works, cited matter, names in some settings | `*…*` |
| a ruled table | a markdown table, its caption bold on the line above |
| verse | a blockquote, one printed line per line |
| a footnote keyed `(1)` at the foot of a column | at the foot of the page, after a `---` |

Keep the printed capitalisation of headings.

**A heading occupies a line of its own.** Caps at the head of a paragraph, with the text
running on from them on the same line, are text — `THÉATRE FRANÇOIS.` run into its notice is
part of the notice, while `PALAIS ROYAL.` centred on its own line is a heading. This is the
distinction that decides it; nothing else does.

**Heading levels record how the page sets the type, not a logical hierarchy.** `###` under a
`##` does not claim the piece belongs to the department above it — a notice from the register
of seals set as a titled piece is `###` even when the nearest `##` is `MUSIQUE.`, because the
paper gives it the same rank as every other titled piece. Do not renumber levels to build a
tree the page does not have.

**Do not italicise inside a heading.** The heading level carries it. `### RÉSULTAT du Conseil
d'État du Roi, tenu à Versailles le 27 Décembre 1788.`, not `### *RÉSULTAT…*`.

**A rule gets a `---` only when it is doing work no heading is already doing.** The rule above
a footnote does; the rule between two sections with nothing else to divide them does; the
short ornamental rules between spectacle notices that each already carry a heading do not.

Verse is a blockquote because it is the one construction where the **line breaks are the
form**, and a blockquote keeps one printed line on one line of the file without depending on
a renderer honouring trailing spaces:

```markdown
> O Monde ! aggrandis-toi : Copernic va paroître ;
> Il paroît, il a dit, l'univers est changé.
```

A table cell the original merges down several rows — one `Vent.` reading standing against
three `Époques.` — goes in the **first** of those rows, and the cells below it are left empty,
however many columns are merged:

```markdown
| A 7 h. ½ m. | — 17, 4 | 28. 2, 9 | E. S. E. | Ciel assez beau toute la matinée. |
| A 2 s. | — 9, 8 | 28. 1, 8 | | |
| A 9 s. | — 9, 0 | 27. 11, 2 | | |
```

Markdown cannot merge cells, and repeating the value would state three observations where the
paper made one. The caption goes bold on the line above **wherever the paper puts it** — the
meteorological table's caption is set rotated in the left margin inside a brace, and it still
goes above.

**Prose line breaks are not preserved.** A paragraph is one line in the file. A word broken
across a line by a hyphen is rejoined — `impor-` + `tante.` is `importante.`

**Column breaks are not marked.** The text simply continues.

### What is not part of the document

Leave these out entirely:

- the digitiser's watermark (`Digitized by Google`)
- library stamps, shelfmarks, and accession numbers added by a holding institution
- signature marks and catchwords at the foot of a leaf — binder's apparatus, not text
- bleed-through from the other side of the leaf

The **printed page number** is also not transcribed into the markdown. It goes in the record,
as `label`. See below.

### Marking what you could not read

In the markdown, in place of the text:

| mark | for |
|---|---|
| `[illegible]` | a word or more you cannot read at all |
| `[illegible: 3 words]` | when you can count what is missing |
| `[unclear: Sorin]` | you have a reading but would not stake a quote on it |
| `[torn]`, `[stained]`, `[cropped]` | the page itself is damaged or the scan cuts it off |
| `[sic]` | the paper's own error, transcribed as printed |

An editorial mark **replaces the text and its formatting both**. An italic word you cannot be
sure of is `[unclear: Abraham]`, not `*[unclear: Abraham]*` — the mark is your voice, and your
voice is not italic.

`[sic]` is the important one: **a misprint in the original is preserved, not corrected**. If
the paper prints `fauhourg` for `faubourg`, or a date that cannot be right, transcribe it as
it stands, mark it `[sic]`, and say so in the note. Silently fixing the source is the one
thing a transcription must never do — a proofread fixes *your* reading of the page, never the
page.

Then summarise the page's unresolved problems in **one line** on that page's `[[page.text]]`
`note`. Omit `note` entirely when there is nothing unresolved — an empty note is noise across
eight hundred pages. Good notes:

```toml
note = "Foot of the second column is cropped by the scan; four lines are lost."
note = "Prints \"fauhourg\" for \"faubourg\", transcribed as printed."
note = "Thermometer readings in the table are faint; the second row is a best reading."
```

## Recording it in the .toml

One `[[page]]` per page, in order, each with one `[[page.text]]`. Keep everything already in
the file, `#:schema` first, and keep `pages = { from, to }` — the loader checks that the
number of `[[page]]` entries equals the length of that range and errors if they disagree.

```toml
#:schema ../../../../../schemas/source.json
id    = "journal-de-paris-1789-01-02"
layer = "document"
of    = "journal-de-paris-1789-vol1"
type  = "issue"
no    = 2
date  = "1789-01-02"
pages = { from = 9, to = 12 }

[[page]]
n     = 1
label = "5"
[[page.text]]
file = "journal-de-paris-1789-01-02.p1.md"
kind = "transcription"
by   = "claude-opus-5"
lang = "fr"

[[page]]
n     = 2
label = "6"
[[page.text]]
file = "journal-de-paris-1789-01-02.p2.md"
kind = "transcription"
by   = "claude-opus-5"
lang = "fr"
note = "Second column is thickly inked; the last word of the page is a best reading."
```

- `n` runs `1..N`. It is the page's number **within this document**, which is what `.pN`
  addresses.
- **`label` is read off the page and is never calculated.** It is the folio the printer set,
  as a string. It is *not* `pages.from`, and it is *not* `n`: `pages` counts pages of the
  scanned PDF, which includes leaves the printer never numbered. The example above is real —
  the 2 January issue occupies PDF pages 9 to 12 and is printed 5 to 8, because four
  unnumbered leaves open the volume. Read the corner of each page at `4x4`. If you cannot read
  it, omit `label` and say so in the note; do not guess it from the page before.
- `kind = "transcription"`, not `ocr`. `ocr` means a machine pass nobody read.
- `by` names who produced it, because the archive expects to be re-transcribed one day and
  needs to know which pass a page is still on.
- Follow the file's existing formatting: `=` aligned within each block, `[[page]]` and
  `[[page.text]]` flush left, keys in the order shown.

## A worked page

`example-page.md`, beside this file, is the first page of the first issue of 1789
(`journal-de-paris-1789-01-01.p1`) transcribed to these rules. Read it before you start. It
shows, on one page: the masthead, a caption plus a ruled table with merged cells, two heading
levels, a long paragraph reassembled across a column break with its hyphenation rejoined,
italics on work titles, verse as a blockquote, and a keyed footnote below a rule.

## Do not

- Do not touch `id`, `of`, `layer`, `type`, `no`, `date`, `pages`, or `supplement_to`. If one
  of them looks wrong, say so in your report and leave it alone — those come from the ingest,
  and correcting one by hand hides a bug in it.
- Do not transcribe a page you did not look at.
- Do not summarise, abridge, or "clean up" the text. Every word on the page is in the file.
- Do not skip a blank page. It still gets a `[[page]]`, or the count stops matching the range.
  Its file holds the single line `[blank page]`, and its text entry carries
  `note = "Blank page."` — an absent file is a validation error, and an empty one is
  indistinguishable from a transcription that was never written.
