Option Explicit

Dim shell, fso, scriptDir, powershell, command, exitCode
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
powershell = shell.ExpandEnvironmentStrings("%SystemRoot%") & "\System32\WindowsPowerShell\v1.0\powershell.exe"
command = Chr(34) & powershell & Chr(34) _
  & " -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File " _
  & Chr(34) & scriptDir & "\run-reverse-tunnel-service.ps1" & Chr(34)

exitCode = shell.Run(command, 0, True)
WScript.Quit exitCode
