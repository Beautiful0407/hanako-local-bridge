Option Explicit

Dim shell, fso, scriptDir, powershell, command
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
powershell = shell.ExpandEnvironmentStrings("%SystemRoot%") & "\System32\WindowsPowerShell\v1.0\powershell.exe"
command = Chr(34) & powershell & Chr(34) _
  & " -NoLogo -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File " _
  & Chr(34) & scriptDir & "\manager-ui.ps1" & Chr(34) _
  & " -InstallRoot " & Chr(34) & scriptDir & Chr(34)

shell.Run command, 0, False
