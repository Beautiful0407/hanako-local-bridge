Option Explicit

Dim shell, fso, scriptDir, nativeExe, nativeCommand, smokeResult
Dim powershell, fallbackCommand, launchResult
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
nativeExe = scriptDir & "\manager\HanakoBridgeManager.exe"

If fso.FileExists(nativeExe) Then
  On Error Resume Next
  nativeCommand = Chr(34) & nativeExe & Chr(34) _
    & " --smoke-test --install-root " & Chr(34) & scriptDir & Chr(34)
  smokeResult = shell.Run(nativeCommand, 0, True)
  If Err.Number = 0 And smokeResult = 0 Then
    launchResult = shell.Run( _
      Chr(34) & nativeExe & Chr(34) _
        & " --install-root " & Chr(34) & scriptDir & Chr(34), _
      1, _
      False _
    )
    If Err.Number = 0 Then
      WScript.Quit 0
    End If
  End If
  Err.Clear
  On Error GoTo 0
End If

powershell = shell.ExpandEnvironmentStrings("%SystemRoot%") & "\System32\WindowsPowerShell\v1.0\powershell.exe"
fallbackCommand = Chr(34) & powershell & Chr(34) _
  & " -NoLogo -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File " _
  & Chr(34) & scriptDir & "\manager-ui.ps1" & Chr(34) _
  & " -InstallRoot " & Chr(34) & scriptDir & Chr(34)

shell.Run fallbackCommand, 0, False
