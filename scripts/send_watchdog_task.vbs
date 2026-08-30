' Launch the watchdog without creating a console window.
' Runs send_watchdog.ps1 on its own schedule (TeeHeeWatchdog scheduled
' task, every 10 min) to verify the TeeHeeWatch heartbeat is fresh.
Option Explicit

Dim fso, shell, scriptDir, watchdog, powershell, command, exitCode
Set fso = CreateObject("Scripting.FileSystemObject")
Set shell = WScript.Shell

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
watchdog = fso.BuildPath(scriptDir, "send_watchdog.ps1")
powershell = shell.ExpandEnvironmentStrings("%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe")

command = """" & powershell & """ -NoLogo -NoProfile -NonInteractive" & _
          " -WindowStyle Hidden -ExecutionPolicy Bypass -File """ & watchdog & """"
exitCode = shell.Run(command, 0, True)
WScript.Quit exitCode
