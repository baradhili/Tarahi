; Tarahi Windows installer (NSIS). Build:
;   makensis -DVERSION=<ver> packaging/installer.nsi
; from the repo root; expects target\release\tarahi.exe.

!define APPNAME "Tarahi"
; Local builds: makensis -DVERSION=x packaging/installer.nsi from the
; repo root defaults LICENSEDIR to the repo root.
!ifndef LICENSEDIR
  !define LICENSEDIR "."
!endif
; Binary directory passed in via -DBINDIR=... (NSIS resolves File
; paths relative to this script, not the invocation directory).
Name "${APPNAME}"
OutFile "Tarahi_${VERSION}_windows_x64_setup.exe"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin
Unicode true
SetCompressor /SOLID lzma

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "${APPNAME} (required)"
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${BINDIR}\tarahi.exe"
  CreateDirectory "$INSTDIR\licenses"
  SetOutPath "$INSTDIR\licenses"
  File /oname=LICENSE-MIT "${LICENSEDIR}\LICENSE-MIT"
  File /oname=LICENSE-APACHE "${LICENSEDIR}\LICENSE-APACHE"
  ; Every vendored asset's license ships with the installer (PR #299
  ; review: the hunspell dictionaries' notices require the license to
  ; accompany the binaries). The flat names match
  ; packaging/collect-licenses.sh, which the workflow stages into
  ; licenses-third-party before makensis runs.
  File /oname=LICENSE-font-Carlito "${LICENSEDIR}\licenses-third-party\LICENSE-font-Carlito"
  File /oname=LICENSE-font-Liberation "${LICENSEDIR}\licenses-third-party\LICENSE-font-Liberation"
  File /oname=LICENSE-font-Vazirmatn-OFL "${LICENSEDIR}\licenses-third-party\LICENSE-font-Vazirmatn-OFL"
  File /oname=LICENSE-font-NotoSansSC-OFL "${LICENSEDIR}\licenses-third-party\LICENSE-font-NotoSansSC-OFL"
  File /oname=LICENSE-font-NotoSansJP-OFL "${LICENSEDIR}\licenses-third-party\LICENSE-font-NotoSansJP-OFL"
  File /oname=LICENSE-font-NotoSansKR-OFL "${LICENSEDIR}\licenses-third-party\LICENSE-font-NotoSansKR-OFL"
  File /oname=LICENSE-font-OpenSans-OFL "${LICENSEDIR}\licenses-third-party\LICENSE-font-OpenSans-OFL"
  File /oname=LICENSE-dict-en-US "${LICENSEDIR}\licenses-third-party\LICENSE-dict-en-US"
  File /oname=README-dict-en_US "${LICENSEDIR}\licenses-third-party\README-dict-en_US"
  File /oname=README-dict-en_US-wordlist "${LICENSEDIR}\licenses-third-party\README-dict-en_US-wordlist"
  File /oname=LICENSE-dict-de-DE-GPLv2 "${LICENSEDIR}\licenses-third-party\LICENSE-dict-de-DE-GPLv2"
  File /oname=LICENSE-dict-de-DE-GPLv3 "${LICENSEDIR}\licenses-third-party\LICENSE-dict-de-DE-GPLv3"
  File /oname=README-dict-de-DE-frami "${LICENSEDIR}\licenses-third-party\README-dict-de-DE-frami"
  File /oname=LICENSE-dict-fa-IR "${LICENSEDIR}\licenses-third-party\LICENSE-dict-fa-IR"
  SetOutPath "$INSTDIR"
  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
    "DisplayName" "${APPNAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
    "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
    "DisplayVersion" "${VERSION}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Start Menu shortcut"
  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\tarahi.exe"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\Uninstall ${APPNAME}.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Desktop shortcut"
  CreateShortcut "$DESKTOP\${APPNAME}.lnk" "$INSTDIR\tarahi.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\licenses\LICENSE-MIT"
  Delete "$INSTDIR\licenses\LICENSE-APACHE"
  Delete "$INSTDIR\licenses\LICENSE-*"
  Delete "$INSTDIR\licenses\README-*"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\tarahi.exe"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$DESKTOP\${APPNAME}.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Uninstall ${APPNAME}.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"
  DeleteRegKey HKLM "Software\${APPNAME}"
SectionEnd
