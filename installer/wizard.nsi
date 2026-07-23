; Hanako Local Bridge - NSIS wizard shell.
; This is only a wizard UI. All real install work is done by the bundled Rust
; installer (HanakoLocalBridge-Setup.exe), which already handles stopping
; processes, transactional overwrite, scheduled tasks, shell integration, and
; starting the service. We never reimplement that logic here.
;
; Build with:
;   makensis /DVERSION=<ver> /DSETUP_EXE=<abs path to HanakoLocalBridge-Setup.exe> \
;            /DOUT_FILE=<abs output path> installer\wizard.nsi

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef SETUP_EXE
  !error "SETUP_EXE must be defined (path to HanakoLocalBridge-Setup.exe)"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "HanakoLocalBridge-Wizard-Setup.exe"
!endif

Name "Hanako Local Bridge"
OutFile "${OUT_FILE}"
; Per-user install, no admin. Matches the Rust installer's per-user design.
RequestExecutionLevel user
; The bundled Setup.exe is not code-signed, so post-build PE edits would break
; the NSIS CRC; disable the check (same reason as HanaAgent's installer).
CRCCheck off
InstallDir "$LOCALAPPDATA\HanakoLocalBridge"

!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

; Finish page: offer to launch the bridge (its no-arg launch opens the manager).
!define MUI_FINISHPAGE_RUN "$INSTDIR\hanako-bridge.exe"
!define MUI_FINISHPAGE_RUN_TEXT "启动 Hanako Local Bridge"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_LANGUAGE "SimpChinese"

Section "Install"
  SetOutPath "$INSTDIR"
  ; Drop the bundled Rust installer (it already contains the payload zip).
  File "/oname=HanakoLocalBridge-Setup.exe" "${SETUP_EXE}"

  DetailPrint "正在安装 Hanako Local Bridge ..."
  ; Pass --install-root so the Rust installer runs non-interactively (no extra
  ; MessageBox) and installs into the directory the user picked in this wizard.
  ; It does all the real work: stop processes, transactional overwrite, delete/
  ; recreate the scheduled task, write config, shell integration, start service.
  ;
  ; Passing /TESTMODE on the wizard's own command line forwards --test-mode to
  ; the Rust installer, which skips stopping processes / starting the service /
  ; launching the manager. This exists ONLY so isolated tests can exercise the
  ; wizard -> release -> file layout path without touching a running production
  ; bridge or its ports. Normal installs never pass it.
  ${GetParameters} $R0
  ${GetOptions} $R0 "/TESTMODE" $R1
  ${If} ${Errors}
    StrCpy $R2 ""
  ${Else}
    StrCpy $R2 " --test-mode"
  ${EndIf}
  nsExec::ExecToLog '"$INSTDIR\HanakoLocalBridge-Setup.exe" --install-root "$INSTDIR"$R2'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "安装失败（错误码 $0）。$\r$\n如果这台电脑装有企业杀毒/EDR，可能拦截了未签名的安装程序，请联系 IT 放行后重试。"
    Abort "安装失败"
  ${EndIf}
  DetailPrint "安装完成。"

  ; No uninstaller is generated here on purpose. The Rust installer's
  ; repair_shell_integration already writes the HKCU Uninstall entry, whose
  ; UninstallString points at "Setup.exe --uninstall --install-root <dir>".
  ; So "添加或删除程序" already has a working entry that runs the Rust
  ; uninstaller (scheduled task + shell integration + directory removal).
  ; A separate NSIS uninstaller would just be a duplicate entry.
SectionEnd

