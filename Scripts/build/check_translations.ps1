# Verifies that every UI translation file (src/lang/<code>.rs) covers every
# English key used through tr()/tr_fmt() in the source tree.
#
# Usage (from the repository root):
#   powershell -ExecutionPolicy Bypass -File Scripts/build/check_translations.ps1
#
# Exits with code 1 when a language is missing keys or has unknown extra keys.

$ErrorActionPreference = 'Stop'
$enc = [System.Text.Encoding]::UTF8

# 1. Collect the canonical key set from every tr(lang, "...") / tr_fmt(...).
$keys = New-Object System.Collections.Generic.HashSet[string]
$srcPat = 'tr(?:_fmt)?\(\s*lang\s*,\s*"((?:[^"\\]|\\.)*)"'
Get-ChildItem -Path src -Recurse -Filter *.rs |
    Where-Object { $_.DirectoryName -notlike '*\lang' -and $_.Name -ne 'lang.rs' } |
    ForEach-Object {
        $text = [System.IO.File]::ReadAllText($_.FullName, $enc)
        foreach ($m in [regex]::Matches($text, $srcPat, [System.Text.RegularExpressions.RegexOptions]::Singleline)) {
            $inner = [regex]::Replace($m.Groups[1].Value, '\\\r?\n\s*', '')
            [void]$keys.Add($inner)
        }
    }
Write-Host "canonical keys: $($keys.Count)"

# 2. Compare each language file's match arms against the canonical set.
$armPat = '"((?:[^"\\]|\\.)*)"\s*=>'
$failed = $false
foreach ($file in Get-ChildItem -Path src\lang -Filter *.rs) {
    $code = $file.BaseName
    $text = [System.IO.File]::ReadAllText($file.FullName, $enc)
    $arms = New-Object System.Collections.Generic.HashSet[string]
    foreach ($m in [regex]::Matches($text, $armPat)) { [void]$arms.Add($m.Groups[1].Value) }
    $missing = @($keys | Where-Object { -not $arms.Contains($_) })
    $extra = @($arms | Where-Object { -not $keys.Contains($_) })
    $status = if ($missing.Count -eq 0 -and $extra.Count -eq 0) { 'OK' } else { 'FAIL' }
    Write-Host ("{0,-4} arms={1,-4} missing={2,-3} extra={3,-3} {4}" -f $code, $arms.Count, $missing.Count, $extra.Count, $status)
    if ($missing.Count -gt 0) {
        $failed = $true
        foreach ($k in ($missing | Select-Object -First 10)) { Write-Host "     missing: $k" }
    }
    if ($extra.Count -gt 0) {
        $failed = $true
        foreach ($k in ($extra | Select-Object -First 10)) { Write-Host "     extra:   $k" }
    }
}

if ($failed) { exit 1 }
Write-Host 'All translation files are complete.'
