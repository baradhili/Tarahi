# Vendored UI font: Noto Sans SC (Simplified Chinese / CJK)

Source: Noto Sans SC subset OTFs (SC-only region subset of Noto Sans
CJK) via github.com/notofonts/noto-cjk
(`Sans/SubsetOTF/SC/NotoSansSC-{Regular,Bold}.otf`, commit history at
that path), retrieved 2026-11-19 for #282 (zh-Hans wave).

- NotoSansSC-Regular.otf —
  sha256 faa6c9df652116dde789d351359f3d7e5d2285a2b2a1f04a2d7244df706d5ea9
- NotoSansSC-Bold.otf —
  sha256 c6cb5a93abaa9edc8ee7463b7ebb7f42d618d40e6ed2f7a5371c97b0b64767c0

Upstream: https://github.com/notofonts/noto-cjk
License: SIL Open Font License 1.1 (see OFL.txt beside the files).

| file | face |
|---|---|
| NotoSansSC-Regular.otf | Noto Sans SC Regular |
| NotoSansSC-Bold.otf | Noto Sans SC Bold |

The ONE CJK drop serves every consumer (the #282 design): the wrap
measurement floor and the usvg raster fontdb register it through
`dc::register_complex_script_faces` (the same idempotent entry point
the Vazirmatn drop rides), and dc-ui's `theme::install_fonts` also
loads the bytes into egui as the CJK fallback of the proportional and
monospace families — after the Latin OpenSans stack and Vazirmatn, so
Latin metrics never move. Chinese shape text resolves through the
per-cluster fallback chain exactly like Arabic does (#257). Never
extract fonts from Microsoft products.
