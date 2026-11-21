# Vendored Hunspell dictionaries (#121)

Embedded spell-check dictionaries, consumed by `dc::spellcheck` through the
pure-Rust [`spellbook`] crate (Hunspell `.aff`/`.dic` format). Like
`assets/fonts` and the MDI icon set, every file carries its license beside it
and this README is the provenance record.

**Source of truth:** upstream LibreOffice `dictionaries` repository,
<https://github.com/LibreOffice/dictionaries> — snapshot of `master` at commit
`c0865a85eb67dc377f2cf8e8fe2c4fa9e53f0dcb` (2026-11-21). All three are `SET
UTF-8` Hunspell dictionaries loadable by `spellbook::Dictionary::new`.

| Directory | Dictionary | License | Files |
|---|---|---|---|
| `en_US/` | en_US 2010-03-09 (SCOWL/Pspell wordlist) | wordlist **LGPL** (Kevin Atkinson, Aspell); aff **BSD** (Geoff Kuenning, ispell) | `en_US.aff`, `en_US.dic`, upstream `README.txt`, `LICENSE-en_US.txt` (the GPL text the upstream folder also carries) |
| `de_DE/` | de_DE_frami (igerman98-derived, Franz Michael Baumann) | **GNU GPL v2 or v3** (author's statement in `README_de_DE_frami.txt`) | `de_DE_frami.aff`, `de_DE_frami.dic`, upstream `README_de_DE_frami.txt`, `COPYING.GPLv2`, `COPYING.GPLv3` |
| `fa_IR/` | fa-IR "Lilak" Persian dictionary | **Apache-2.0** | `fa-IR.aff`, `fa-IR.dic`, `LICENSE` |

## Transformations applied at vendor time

- **`de_DE_frami.aff`/`.dic` were transcoded ISO-8859-1 → UTF-8** (upstream
  declares `SET ISO8859-1`; the word lists themselves are single-byte latin-1).
  The vendored copies are byte-for-byte the same words, re-encoded, with the
  `SET` declaration rewritten to `SET UTF-8` — `spellbook` requires UTF-8
  input (it does not implement the aff `SET` directive; see its parser TODO).
  No entries were added, removed, or reordered.
- `en_US` and `fa_IR` files are byte-identical to upstream (already UTF-8).

## Local (system) dictionaries

The embedded set is the deterministic baseline. At first use the spell dialog
also scans the standard Hunspell dictionary locations for `<stem>.aff` +
`<stem>.dic` pairs and prefers a **local dictionary over the embedded one for
the same stem** (the stencil built-ins' disk-first rule, mirrored):

- Linux: `/usr/share/hunspell`, `/usr/share/myspell/dicts`, `/usr/share/myspell`
- macOS: `/Library/Spelling`
- Windows: LibreOffice's `share/extensions/dict` directories
- the user's app-config dir: `<config>/diagramcraft/dictionaries/` — a
  supported drop-point for dictionaries the system package manager doesn't
  ship.

Legacy-encoded local dictionaries are transcoded at load time from their
declared `SET` encoding (`encoding_rs`); headless ctors pin the embedded
fallback so kittest never reads the user's machine (the stencil precedent).

Never extract Microsoft proofing data (`MSZIP.DIC`, `msproof7.dll`,
`PROOF/`) — the #121 no-copy rule stands.

[`spellbook`]: https://crates.io/crates/spellbook
