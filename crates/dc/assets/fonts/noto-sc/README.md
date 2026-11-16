# Vendored CJK face: Noto Sans SC (Simplified Chinese)

Source: notofonts/noto-cjk `Sans/SubsetOTF/SC` (the Simplified-Chinese
subset OTFs), retrieved 2026-11-16 for #282 (zh-Hans — the first CJK
wave language). Upstream: https://github.com/notofonts/noto-cjk
License: SIL Open Font License 1.1 (see OFL.txt beside the files).

| file | face |
|---|---|
| NotoSansSC-Regular.otf | Noto Sans SC Regular |
| NotoSansSC-Bold.otf | Noto Sans SC Bold |

The subset OTFs cover the Han set Simplified Chinese needs (plus kana,
Hangul jamo and Latin), at roughly half the size of the full CJK
super-OTFs — the one shared drop for the language (#282 ranks ja/ko as
follow-on CJK waves; a Japanese/Korean extension reuses or extends this
drop rather than vendoring a second family).

Loaded into egui as the FALLBACK face of the proportional family
(after the Latin OpenSans stack and Vazirmatn), so Chinese chrome
renders without touching Latin metrics, and registered into the
measurement/raster font databases by `dc::register_complex_script_faces`
(called by the dc CLI's main and dc-ui's `theme::install_fonts`) —
ONE drop serves wrap measurement, raster, and shell, exactly the
#257 Vazirmatn coordination. Han runs measure with THIS face's shaped
advances (`crates/dc/src/textmetrics.rs` run-splitting), so the wrap
decision and the drawn glyphs agree by construction even on hosts that
carry system CJK fonts. Never extract fonts from Microsoft products.
