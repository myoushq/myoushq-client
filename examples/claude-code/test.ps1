# Tests for inbox-hook.ps1, the Windows mirror of test.sh's fake-myous half:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File examples/claude-code/test.ps1
#   pwsh -NoProfile -File examples/claude-code/test.ps1
#
# A fake `myous.cmd`, first on PATH, prints a given JSON list (or fails).
# The hook runs under the same PowerShell as this script (Windows
# PowerShell 5.1 or PowerShell 7), as Claude Code would run it.
# The real-client half of test.sh isn't mirrored: the hub doesn't run here.

$Hook = Join-Path $PSScriptRoot "inbox-hook.ps1"
$PSExe = (Get-Process -Id $PID).Path
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("myous-cc-test-" + [guid]::NewGuid().ToString("N"))
$FakeBin = Join-Path $Tmp "fakebin"
$MyousHomeDir = Join-Path $Tmp "home"
New-Item -ItemType Directory -Force -Path $FakeBin, $MyousHomeDir | Out-Null
$script:Failed = 0
# Write-Host, not Write-Output: output would become part of a function's return value.
function Fail([string]$what) { Write-Host "FAIL: $what"; $script:Failed = 1 }
function Ok([string]$what) { Write-Host "ok: $what" }

# --- the fake myous ----------------------------------------------------------
# Records how it was called, then prints %FAKE_JSON% (or fails with %FAKE_EXIT%).
$fake = @'
@echo off
>"%FAKE_LOG%" echo %MYOUS_HOME% %*
if defined FAKE_EXIT (
    echo boom 1>&2
    exit %FAKE_EXIT%
)
type "%FAKE_JSON%"
'@
Set-Content -Path (Join-Path $FakeBin "myous.cmd") -Value ($fake -replace "`r?`n", "`r`n") -Encoding Ascii   # cmd wants CRLF
$env:FAKE_LOG = Join-Path $Tmp "fake.log"
$env:FAKE_JSON = Join-Path $Tmp "items.json"
$env:PATH = "$FakeBin;$env:PATH"
Remove-Item Env:FAKE_EXIT -ErrorAction SilentlyContinue
Remove-Item Env:MYOUS_HOME -ErrorAction SilentlyContinue

function Set-Items([string]$json) { Set-Content -Path $env:FAKE_JSON -Value $json -Encoding Ascii }

function Invoke-Hook {  # run the hook; sets $Out, $Err, $Code (output as UTF-8, LF line ends)
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $PSExe
    $psi.Arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$Hook`" `"$MyousHomeDir`""
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = New-Object Text.UTF8Encoding($false)
    $psi.StandardErrorEncoding = New-Object Text.UTF8Encoding($false)
    $p = [Diagnostics.Process]::Start($psi)
    $errTask = $p.StandardError.ReadToEndAsync()
    $stdout = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit()
    $script:Out = ($stdout -replace "`r`n", "`n").TrimEnd("`n")
    $script:Err = $errTask.Result
    $script:Code = $p.ExitCode
}
function Expect([string]$name, [string]$needle) {  # Out contains needle
    if ($script:Out.Contains($needle)) { return $true }
    Fail "${name}: no '$needle' in output:"; Write-Host $script:Out; return $false
}
function Count-Lines([string]$prefix) { @(($script:Out -split "`n") | Where-Object { $_.StartsWith($prefix) }).Count }

try {
    Set-Items '[]'
    Invoke-Hook
    if ($Code -eq 0 -and $Out -eq "") { Ok "empty inbox: silent, exit 0" } else { Fail "empty inbox: exit $Code, output '$Out' ($Err)" }
    $called = (Get-Content -Raw $env:FAKE_LOG).Trim()
    if ($called -eq "$MyousHomeDir inbox --json") { Ok "MYOUS_HOME from the argument, inbox --json" } else { Fail "called as: $called" }

    # One item: Windows PowerShell 5.1 returns a JSON array as one object.
    Set-Items '[{"seq": 1, "type": "message", "direction": "in", "alias": "Alex''s Muse", "text": "hi", "relationship": "friend"}]'
    Invoke-Hook
    if ($Code -ne 0) { Fail "one item: exit $Code ($Err)" }
    elseif (Expect one "myous: 1 new item(s):`n- Alex's Muse (message): hi`n    (friend)") { Ok "one item (the 5.1 single-element array)" }

    # Non-ASCII as JSON escapes, as the client writes it (Zo-umlaut-e, h-acute-e-llo, a check mark).
    # This file stays ASCII: Windows PowerShell 5.1 reads a .ps1 without a BOM as ANSI.
    $long = "x" * 1500
    Set-Items (@'
[
 {"seq": 1, "type": "message", "direction": "in", "alias": "Alex's Muse", "text": "Does Thursday work?\nOr Friday?",
  "relationship": "friend", "sharing": "calendar yes", "about": "Alex's assistant"},
 {"seq": 2, "type": "message", "direction": "in", "alias": "Shop", "text": "LONG", "relationship": null, "sharing": null},
 {"seq": 3, "type": "file", "direction": "in", "alias": "Sam's PC", "name": "out.png", "size": 1234, "text": "file"},
 {"seq": 4, "type": "update", "version": "9.9.9", "text": "myous 9.9.9 is out"},
 {"seq": 5, "type": "message", "direction": "in", "alias": "Zo\u00eb", "text": "h\u00e9llo \u2713", "relationship": "family"}
]
'@).Replace('"LONG"', '"' + $long + '"')
    Invoke-Hook
    if ($Code -ne 0) { Fail "items: exit $Code ($Err)" }
    $all = $true
    $all = (Expect items "myous: 5 new item(s):") -and $all
    $all = (Expect items "- Alex's Muse (message): Does Thursday work?`n    Or Friday?`n    (friend; may share: calendar yes; it says of itself: Alex's assistant)") -and $all
    $all = (Expect items ("- Shop (message): " + ("x" * 1000) + ' [cut; the rest: myous history --with "Shop"]')) -and $all
    $all = (Expect items "    (relationship not set: share nothing personal)") -and $all
    $all = (Expect items "- Sam's PC (file): out.png (1234 bytes; myous fetch 3)") -and $all
    $all = (Expect items "- myoushq (update): myous 9.9.9 is out") -and $all
    $all = (Expect items ("- Zo" + [char]0x00EB + " (message): h" + [char]0x00E9 + "llo " + [char]0x2713)) -and $all
    $all = (Expect items "untrusted") -and $all
    if ((Count-Lines "- ") -ne 5) { Fail "items: expected 5 entries"; $all = $false }
    if ($all -and $Code -eq 0) { Ok "items: header, five entries, context, cut, non-ASCII, untrusted note" }

    Set-Items ("[" + ((0..11 | ForEach-Object { '{"seq": ' + $_ + ', "type": "message", "alias": "a", "text": "' + $_ + '"}' }) -join ", ") + "]")
    Invoke-Hook
    $all = (Expect many "myous: 12 new item(s):") -and (Expect many "- and 2 more: myous history")
    if ((Count-Lines "- a ") -eq 10 -and $all) { Ok "many items: ten shown, the rest counted" } elseif ($all) { Fail "many items: expected 10 shown" }

    Set-Items '[{"seq": 6, "type": "file", "direction": "in", "alias": "Sam''s PC", "name": "out.txt", "size": 12, "text": "late reply", "late": {"op": "get", "remote": "/work/out.txt", "sent_at": 1700000000}}]'
    Invoke-Hook
    if ($Code -ne 0) { Fail "late file: exit $Code ($Err)" }
    elseif (Expect late "- Sam's PC (file): late reply to cp of /work/out.txt: out.txt (12 bytes; myous fetch 6)") { Ok "a late reply's file says which request it answers" }

    $env:FAKE_EXIT = "3"
    Invoke-Hook
    Remove-Item Env:FAKE_EXIT
    if ($Code -eq 1 -and $Out -eq "" -and $Err.Contains("myous inbox failed")) { Ok "myous fails: exit 1 (not 2), reason on stderr" }
    else { Fail "myous fails: exit $Code, out '$Out', err '$Err'" }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
exit $script:Failed
