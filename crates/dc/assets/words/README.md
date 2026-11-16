# Vendored English wordlist: american-english (SCOWL-derived)

Source: Debian `wamerican` 2020.12.07-2 (`/usr/share/dict/american-english`),
retrieved 2026-11-16 for #121 (spell check). Upstream: SCOWL (Spell Checker
Oriented Word Lists), http://wordlist.sourceforge.net/ — Kevin Atkinson.
License: the SCOWL permissive notice (see LICENSE beside the file —
"permission to use, copy, modify, distribute and sell ... for any purpose
without fee", MIT-style).

| file | contents |
|---|---|
| en-american.txt | 104 334 words, one per line (en_US 60 size class with proper nouns and apostrophe forms) |

Loaded into the spell checker (`crates/dc/src/spell.rs`) at first use via
`include_bytes!` and lowercased into a hash set — checking is
case-insensitive. NEVER extract Microsoft's proofing data (the shipped
Visio install's `MSZIP.DIC`, `msproof7.dll`, `PROOF/` directory) — all of
it is off limits under the no-copy rule.
