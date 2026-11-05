# Run with powershell -NoProfile -File tests\rotate_logs_tests.ps1.
# Extract only Rotate-Log, never run the production rotation/pruning dispatch.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $root 'scripts\rotate_logs.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw "Could not parse rotate_logs.ps1: $parseErrors" }
$definition = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Rotate-Log'
}, $true)
if (-not $definition) { throw 'Rotate-Log not found' }
. ([scriptblock]::Create($definition.Extent.Text))

# Simulate appends at the copy boundary, without touching real sender logs.
function Copy-Item {
    param($Path, $Destination, [switch]$Force, $ErrorAction)
    switch ($script:copyMode) {
        'append' { [System.IO.File]::AppendAllText($Path, "`nnew stats record") }
        'fail' { throw 'simulated archive failure' }
        'short' {
            [System.IO.File]::WriteAllText($Destination, 'short')
            return
        }
    }
    Microsoft.PowerShell.Management\Copy-Item -LiteralPath $Path -Destination $Destination `
        -Force -ErrorAction Stop
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) ("teehee-rotation-test-" + [guid]::NewGuid())
[System.IO.Directory]::CreateDirectory($temp) | Out-Null
$archiveDir = $temp
$source = Join-Path $temp 'live.jsonl'
$original = 'original stats record'
try {
    foreach ($mode in @('stable', 'append', 'fail', 'short')) {
        $script:copyMode = $mode
        $stamp = $mode
        [System.IO.File]::WriteAllText($source, $original)
        Rotate-Log $source 'send' 1
        $live = [System.IO.File]::ReadAllText($source)
        $archive = Join-Path $temp "send-$stamp.jsonl"
        if ($mode -eq 'stable' -or $mode -eq 'append') {
            if ($live.Length -ne 0) { throw "$mode did not rotate the live log" }
            $expected = $original
            if ($mode -eq 'append') { $expected += "`nnew stats record" }
            if ([System.IO.File]::ReadAllText($archive) -ne $expected) {
                throw "$mode did not preserve the complete copied log"
            }
        } elseif ($live -ne $original) {
            throw "$mode changed the live log despite archive failure"
        }
    }
    Write-Host 'PASS: stable rotation, concurrent append, failed copy, and short archive'
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force
}
