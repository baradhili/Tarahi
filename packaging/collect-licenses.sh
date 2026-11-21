#!/usr/bin/env bash
# Gather every vendored asset's license text into ONE flat directory
# (PR #299 review: the hunspell dictionaries' licenses must accompany
# shipped binaries, and the font licenses shipped only in the macOS
# bundle). Flat, prefixed names keep the per-format installs (deb/rpm
# assets, AppImage usr/share/doc, NSIS, the portable zip, the macOS
# bundle) simple and identical. The SOURCE files stay the single
# source of truth — this only copies.
set -euo pipefail
dest="$1"
mkdir -p "$dest"
cp LICENSE-MIT "$dest/LICENSE-MIT"
cp LICENSE-APACHE "$dest/LICENSE-APACHE"
fonts="crates/dc/assets/fonts"
cp "$fonts/LICENSE-Carlito" "$dest/LICENSE-font-Carlito"
cp "$fonts/LICENSE-Liberation" "$dest/LICENSE-font-Liberation"
cp "$fonts/vazirmatn/OFL.txt" "$dest/LICENSE-font-Vazirmatn-OFL"
cp "$fonts/noto-sc/OFL.txt" "$dest/LICENSE-font-NotoSansSC-OFL"
cp "$fonts/noto-jp/OFL.txt" "$dest/LICENSE-font-NotoSansJP-OFL"
cp "$fonts/noto-kr/OFL.txt" "$dest/LICENSE-font-NotoSansKR-OFL"
cp "crates/dc-ui/assets/fonts/opensans/OFL.txt" "$dest/LICENSE-font-OpenSans-OFL"
dicts="crates/dc/assets/dictionaries"
cp "$dicts/en_US/LICENSE-en_US.txt" "$dest/LICENSE-dict-en-US"
cp "$dicts/en_US/README.txt" "$dest/README-dict-en_US"
cp "$dicts/en_US/README_en_US.txt" "$dest/README-dict-en_US-wordlist"
cp "$dicts/de_DE/COPYING.GPLv2" "$dest/LICENSE-dict-de-DE-GPLv2"
cp "$dicts/de_DE/COPYING.GPLv3" "$dest/LICENSE-dict-de-DE-GPLv3"
cp "$dicts/de_DE/README_de_DE_frami.txt" "$dest/README-dict-de-DE-frami"
cp "$dicts/fa_IR/LICENSE" "$dest/LICENSE-dict-fa-IR"
