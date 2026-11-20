; Dawnroom's installer hooks (Tauri's NSIS_HOOK_* macros, included by installer.nsi).
; Install: stop an engine from a previous install, then put the install folder on the user's PATH so
; `dawnroom` works in a new terminal. Uninstall: stop the engine, drop the PATH entry and the
; start-at-sign-in entry. Per user only: HKCU, never admin.

; Adds ($0 = "add") or removes ($0 = "remove") $INSTDIR in HKCU\Environment\Path, keeping every
; other entry as written (%VARS% unexpanded) and the value an expandable string.
!macro DawnroomPath ACTION
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d='$INSTDIR'.TrimEnd('\'); $$k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$p=[string]$$k.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $$parts=@($$p -split ';' | Where-Object { $$_ -and ($$_.TrimEnd('\') -ne $$d) }); if ('${ACTION}' -eq 'add') { $$parts += $$d }; $$k.SetValue('Path', ($$parts -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)"`
  Pop $0
  ; Tell Explorer (and new terminals it starts) that the environment changed.
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

; The data folder into $DawnroomData: data_dir from %APPDATA%\Dawnroom\config.toml (it may have been
; moved), else the default %LOCALAPPDATA%\Dawnroom.
!macro DawnroomDataDir
  nsExec::ExecToStack `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$c = Join-Path $$env:APPDATA 'Dawnroom\config.toml'; $$d = $$null; if (Test-Path $$c) { $$m = Select-String -Path $$c -Pattern '^\s*data_dir\s*=\s*[''\"](.+?)[''\"]\s*$$' | Select-Object -First 1; if ($$m) { $$d = $$m.Matches[0].Groups[1].Value } }; if (-not $$d) { $$d = Join-Path $$env:LOCALAPPDATA 'Dawnroom' }; [Console]::Write($$d.TrimEnd([char]92))"`
  Pop $0
  Pop $DawnroomData
  ${If} $0 != 0
    StrCpy $DawnroomData "$LOCALAPPDATA\Dawnroom"
  ${EndIf}
!macroend

; Asks a running engine to stop (frees the GPU, keeps a download's .part). Quiet when none runs.
!macro DawnroomStopEngine
  ${If} ${FileExists} "$INSTDIR\dawnroom.exe"
    nsExec::Exec `"$INSTDIR\dawnroom.exe" stop`
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; One plain status line instead of every file and shortcut path.
  SetDetailsPrint textonly
  DetailPrint "Installing Dawnroom…"
  SetDetailsPrint none
  !insertmacro DawnroomStopEngine
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro DawnroomPath "add"
  SetDetailsPrint textonly
  DetailPrint "Done"
  SetDetailsPrint both
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro DawnroomStopEngine
  ; Start at sign-in (Settings toggle): the Run entry Studio writes.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Dawnroom"
  !insertmacro DawnroomPath "remove"
!macroend
