; SPDX-License-Identifier: GPL-3.0-or-later
; The Windows installer (roadmap 1.8). Built by packaging/windows/build.sh
; in the release workflow. It installs for the current user only, into
; %LOCALAPPDATA%\Programs\DAW, so it needs no administrator rights. It is
; not signed yet (docs/REVIEW_QUEUE.md, 0.7 Windows signing), so Windows
; warns before running it. Uninstalling removes the program only: projects,
; takes, settings and crash reports stay.
;
; makensis -DVERSION=0.1.0 -DSRC=<folder with daw.exe> -DOUT=<setup.exe>

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !error "Pass -DVERSION=x.y.z"
!endif
!ifndef SRC
  !error "Pass -DSRC=<folder with daw.exe>"
!endif
; File version fields take numbers only: 1.0.0-beta.1 gives 1.0.0.
!ifndef NUMVERSION
  !define NUMVERSION "${VERSION}"
!endif
!ifndef OUT
  !define OUT "daw-${VERSION}-windows-x64-setup.exe"
!endif

!define NAME "DAW"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"

Name "${NAME} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${NAME}"
InstallDirRegKey HKCU "Software\${NAME}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ManifestDPIAware true

!define MUI_ICON "${__FILEDIR__}\icon.ico"
!define MUI_UNICON "${__FILEDIR__}\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\daw.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${SRC}\LICENSE.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

VIProductVersion "${NUMVERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${NUMVERSION}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "LegalCopyright" "GPL-3.0-or-later"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${SRC}\daw.exe"
  File "${SRC}\LICENSE.txt"
  File "${SRC}\README.txt"
  File "${__FILEDIR__}\icon.ico"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\daw.exe" "" "$INSTDIR\icon.ico"

  WriteRegStr HKCU "Software\${NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/andymantell/daw"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$INSTDIR\daw.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\${NAME}"
SectionEnd
