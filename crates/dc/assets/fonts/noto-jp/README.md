# Vendored UI font: Noto Sans JP (Japanese / CJK)

Source: Noto Sans JP subset OTFs (JP-region subset of Noto Sans
CJK — kana + JIS kanji, no Hangul) via github.com/notofonts/noto-cjk
(`Sans/SubsetOTF/JP/NotoSansJP-{Regular,Bold}.otf`, commit history at
that path), retrieved 2026-11-21 for #282 (ja/ko wave).

- NotoSansJP-Regular.otf —
  sha256 dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073
- NotoSansJP-Bold.otf —
  sha256 1b0edfb500b73a4fa8a4fcaae1bbbd403994e08e73e3e0da37e70d3853f42c5f

Upstream: https://github.com/notofonts/noto-cjk
License: SIL Open Font License 1.1 (see OFL.txt beside the files).
Copyright: © 2014-2021 Adobe (http://www.adobe.com/.) — carried in the
font's own name table (nameID 0, with the OFL text at nameID 13/14),
the upstream LICENSE file being the license text without a holder
line.

| file | face |
|---|---|
| NotoSansJP-Regular.otf | Noto Sans JP Regular |
| NotoSansJP-Bold.otf | Noto Sans JP Bold |

Rides the SAME one-drop contract as the zh-Hans wave (#282): the
measurement floor and the usvg raster fontdb register it through
`dc::register_complex_script_faces` (the idempotent entry point), and
dc-ui's `theme::install_fonts` also loads the bytes into egui as the
Japanese fallback of the proportional and monospace families. The
CANVAS stack keeps the fixed `dc::CJK_FACE_ORDER` (SC leads — the
zh rendering never moves), shared by the raster registration and both
measurement chains; only the EGUI SHELL varies the order by the
current language's regional face, so a Japanese shell paints Japanese
glyph forms while a Chinese shell keeps the SC forms the zh-Hans wave
shipped. Never extract fonts from Microsoft products.
