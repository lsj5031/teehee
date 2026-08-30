' Launch the log rotation without creating a console window.
' Runs rotate_logs.ps1 on its own schedule (TeeHeeRotate scheduled
' task, daily) to rotate oversized logs into logs/archive and prune
' old archives.
Option Explicit

Dim fso, shell, scriptDir, rotate, powershell, command, exitCode
Set fso = CreateObject("Scripting.FileSystemObject")
Set shell = WScript.Shell

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
rotate = fso.BuildPath(scriptDir, "rotate_logs.ps1")
powershell = shell.ExpandEnvironmentStrings("%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe")

command = """" & powershell & """ -NoLogo -NoProfile -NonInteractive" & _
          " -WindowStyle Hidden -ExecutionPolicy Bypass -File """ & rotate & """"
exitCode = shell.Run(command, 0, True)
WScript.Quit exitCode
