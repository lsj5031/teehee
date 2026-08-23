' Launch the scheduled one-shot watcher without creating a console window.
Option Explicit

Dim fso, shell, scriptDir, watcher, powershell, command, exitCode
Set fso = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
watcher = fso.BuildPath(scriptDir, "send_watch.ps1")
powershell = shell.ExpandEnvironmentStrings("%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe")

command = """" & powershell & """ -NoLogo -NoProfile -NonInteractive" & _
          " -WindowStyle Hidden -ExecutionPolicy Bypass -File """ & watcher & """ -Check"
exitCode = shell.Run(command, 0, True)
WScript.Quit exitCode
