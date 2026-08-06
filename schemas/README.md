# schemas/

## source.json is GENERATED. Do not edit it.

`schemas/source.json` is derived from the Rust types in `tools/src/model.rs` via
`schemars`. It is not a hand-maintained document, and editing it by hand is the one
thing that must never happen here: the schema would start disagreeing with the
validator that actually enforces the archive, and a schema that lies is worse than no
schema, because it is believed.

Regenerate it after any change to the record types:

```sh
cd tools
cargo run -- schema
```

Verify it has not drifted — this is what CI runs, and it exits non-zero when the
checked-in file differs from what the types generate:

```sh
cd tools
cargo run -- schema --check
```

One definition in `model.rs` drives three things at once: Rust type checking, TOML
deserialisation, and this JSON Schema. That is the whole point of generating it.

## What the schema describes

A single archive record. It is a `oneOf` of three branches discriminated on `layer`:

| `layer`    | required fields   |
| ---------- | ----------------- |
| `source`   | `layer`, `title`  |
| `copy`     | `layer`, `title`  |
| `document` | `layer`           |

`document` deliberately does not require `title` — a terse issue record has none.
`layer = "page"` is not a branch and is rejected: pages exist only inline, as
`[[page]]` entries inside a record, never as files of their own.

Because the discriminator is a `const` on `layer`, writing `layer = "copy"` narrows
both autocomplete and the required-field set to the copy branch alone.

## How a record gets validated in the editor

Each record carries a `#:schema` directive as its first line, pointing at this file
**relative to the record**:

```toml
#:schema ../../schemas/source.json
layer = "source"
title = "Plan de Turgot"
```

The number of `../` segments depends on the record's depth: two from
`source/turgot/`, more from `source/journal-de-paris/1789/01/03/`. Whatever writes
records — including `scans migrate` — must emit this line with the correct depth.

The directive is the only portable mechanism, and that is a finding, not a preference.
`evenBetterToml.schema.associations` cannot express a repo-relative schema path:

- a relative value such as `./schemas/source.json` is resolved against an internal
  `root:///` base rather than the workspace, so the schema fails to load;
- `${workspaceFolder}` is not expanded;
- a bare Windows path like `D:/…` is parsed as a URL with the scheme `d`.

In all three cases the association *matches* the document and then produces **zero
diagnostics**. Nothing is reported in the editor — the failure is visible only in the
extension's output channel. A committed association would therefore hand every
contributor the appearance of validation and none of the substance. `.vscode/settings.json`
explains this at the point of temptation.

Install the extension recommended in `.vscode/extensions.json`
(`tamasfe.even-better-toml`); without it, TOML files get no schema validation at all.

## Enforcing the schema outside the editor

`.taplo.toml` maps `source/**/*.toml` to this schema for the
[taplo](https://taplo.tamasfe.dev/) CLI, so the same rules apply in CI:

```sh
# validate every archive record against schemas/source.json
taplo check

# if the config's globs misbehave (seen on Windows), pass the paths explicitly
taplo check "source/**/*.toml"

# check formatting too, without rewriting anything
taplo fmt --check --diff
```

Install with `cargo install taplo-cli --locked`.

A full CI sequence, cheapest check first:

```sh
cd tools && cargo run -- schema --check   # schema matches the types
taplo check                               # records match the schema
cd tools && cargo run -- validate         # the 10 semantic checks
```

The schema and `scans validate` do different jobs and neither replaces the other. The
schema checks the *shape* of one file in isolation — field names, types, which fields
each layer requires. `scans validate` checks everything that needs more than one file
or more than one field: that `of` resolves and does not cycle, that ids are unique,
that page ranges fit inside `scan.count` and do not collide between siblings, that
dates parse as EDTF and fall inside their copy's `covers`.

## Verification status

Confirmed by driving the bundled taplo language server directly:

- `schemas/source.json` parses, is a structurally valid JSON Schema
  (`https://json-schema.org/draft/2020-12/schema`), and its `oneOf` discriminates on
  `layer` via three `const` branches;
- `cargo run -- schema --check` reports the checked-in file up to date;
- with a `#:schema` directive, an invalid record is flagged and a valid one is clean;
  a `source` missing `title` is flagged, a `document` missing `title` is not, and
  `layer = "page"` is rejected — the per-layer discrimination genuinely works.

Not verified, and stated plainly rather than assumed:

- **The `taplo` CLI itself was never run.** `cargo install taplo-cli` failed here with
  `os error 112` — the disk holding `CARGO_HOME` is full. Every `taplo` command above
  is written from its documented interface, not from observed output, and the
  `.taplo.toml` rule syntax is unconfirmed against a real binary. Run `taplo check`
  once on a machine with disk space before relying on it in CI.
- **The archive has not been migrated yet.** The records still live in
  `journal-de-paris/`, `turgot/` and `verniquet/` at the repo root, in the legacy
  field layout, with no `source/` directory and no `#:schema` directives. They will
  *not* validate against this schema, and `taplo check` currently matches no files.
  That is expected until `scans migrate` runs; it is not a fault in the schema.
