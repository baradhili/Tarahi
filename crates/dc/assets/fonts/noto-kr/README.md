# Vendored UI font: Noto Sans KR (Korean / CJK)

Source: Noto Sans KR subset OTFs (KR-region subset of Noto Sans
CJK — Hangul syllables + KS X 1001 Hanja) via
github.com/notofonts/noto-cjk
(`Sans/SubsetOTF/KR/NotoSansKR-{Regular,Bold}.otf`, commit history at
that path), retrieved 2026-11-21 for #282 (ja/ko wave).

- NotoSansKR-Regular.otf —
  sha256 69975a0ac8472717870aefeab0a4d52739308d90856b9955313b2ad5e0148d68
- NotoSansKR-Bold.otf —
  sha256 5a6ceb287ed2fc6cfc6213144ebea68cbd94b20fc9eb873d8486493bf02d9bda

Upstream: https://github.com/notofonts/noto-cjk
License: SIL Open Font License 1.1 (see OFL.txt beside the files).
Copyright: © 2014-2021 Adobe (http://www.adobe.com/.) — carried in the
font's own name table (nameID 0, with the OFL text at nameID 13/14),
the upstream LICENSE file being the license text without a holder
line.

| file | face |
|---|---|
| NotoSansKR-Regular.otf | Noto Sans KR Regular |
| NotoSansKR-Bold.otf | Noto Sans KR Bold |

Rides the SAME one-drop contract as the zh-Hans wave (#282): the
measurement floor and the usvg raster fontdb register it through
`dc::register_complex_script_faces` (the idempotent entry point), and
dc-ui's `theme::install_fonts` also loads the bytes into egui as the
Korean fallback of the proportional and monospace families. Korean
needs no wrap-class work (the zh wave's note in `textmetrics.rs`):
Korean is space-separated, so the space-driven wrap already breaks it;
Hangul is deliberately absent from the CJK break-grant classes. Never
extract fonts from Microsoft products.
