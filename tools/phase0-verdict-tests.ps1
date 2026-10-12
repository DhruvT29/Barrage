<#
    Tests for the phase 0 decision logic.

    Run:  .\tools\phase0-verdict-tests.ps1
    Exit: 0 if every scenario behaves as asserted, 1 otherwise.

    These exist because the first version of the verdict logic passed on the
    real data for the wrong reasons, and nothing caught it. Four of the
    scenarios below are the specific bugs found in review:

      starved          a link starved in one combined run, yet PASS was reported
                       because only the best run by total was ever examined
      notraffic        nothing moved at all, yet it was blamed on --interface
                       binding and the Windows strong host model
      bindignored      binding demonstrably broken, yet a confident retention
                       figure of 109% was printed underneath the failure
      onerepeat        a single run reported "noise band 0%", which reads as a
                       perfectly stable network rather than an unmeasured one

    Every one of those must now fail or skip. `real` must still pass, using the
    numbers actually measured on 2026-10-12, so the fixes cannot quietly
    invalidate the phase 0 result they were meant to scrutinise.
#>

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "phase0-verdict.ps1")

function Row {
    param($Config, $Repeat, $Wifi, $Tether, $Cn = 4, $Ok206 = 3, $Secs = 30)
    $total = $Wifi + $Tether
    $pct = 0
    if ($total -gt 0) { $pct = [math]::Round(($Tether / $total) * 100, 0) }
    [pscustomobject]@{
        Config = $Config; Repeat = $Repeat; Secs = $Secs
        WifiMbps = [double]$Wifi; TetherMbps = [double]$Tether; TotalMbps = [double]$total
        AriaMbps = [double]$total; TetherPct = $pct
        MaxCn = $Cn; Ok206 = $Ok206; Throttled = 12
    }
}

$scenarios = @(

    @{
        Name = "real"
        Why  = "the numbers measured on 2026-10-12. Must still pass."
        Rows = @(
            (Row "A-wifi" 1 30.7 0 2 1), (Row "B-tether" 1 0 173.7 2 1), (Row "C-combined" 1 64.1 146.3 4 3),
            (Row "A-wifi" 2 138.4 0 2 1), (Row "B-tether" 2 0 144.5 2 1), (Row "C-combined" 2 111.3 180.6 4 3)
        )
        ExpectPass = $true
        Expect = @(
            @{ State = "OK"; Match = "both links ran at once in every combined run" }
        )
        # Locks the paired baseline in place. Against its own repeat's solo run
        # (30.7) Wi-Fi delivered 64.1, which is 209%. The old code divided by
        # the best solo rate seen anywhere (138.4) and got 46%, below the floor,
        # a value produced purely by comparing across network conditions four
        # minutes and four times apart.
        ExpectPair = @{ Repeat = 1; WifiRetAtLeast = 200 }
    },

    @{
        Name = "starved"
        Why  = "REGRESSION: one combined run has Wi-Fi starved to 4%. Old code reported PASS."
        Rows = @(
            (Row "A-wifi" 1 140 0 2 1), (Row "B-tether" 1 0 170 2 1), (Row "C-combined" 1 5 165 4 3),
            (Row "A-wifi" 2 140 0 2 1), (Row "B-tether" 2 0 170 2 1), (Row "C-combined" 2 120 175 4 3)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "FAIL"; Match = "a link was starved in repeat 1" }
        )
    },

    @{
        Name = "onerepeat"
        Why  = "REGRESSION: a single repeat cannot measure noise, so it is not a decision."
        Rows = @(
            (Row "A-wifi" 1 140 0 2 1), (Row "B-tether" 1 0 170 2 1), (Row "C-combined" 1 120 175 4 3)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "FAIL"; Match = "run-to-run noise is unmeasured" }
        )
        ExpectNoiseNull = $true
    },

    @{
        Name = "notraffic"
        Why  = "REGRESSION: nothing moved. Must be reported as void, never blamed on binding."
        Rows = @(
            (Row "A-wifi" 1 0 0 2 1), (Row "B-tether" 1 0 0 2 1), (Row "C-combined" 1 0 0 2 1),
            (Row "A-wifi" 2 0 0 2 1), (Row "B-tether" 2 0 0 2 1), (Row "C-combined" 2 0 0 2 1)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "FAIL"; Match = "no bytes moved in" },
            @{ State = "SKIP"; Match = "cannot assess --interface binding" },
            @{ State = "SKIP"; Match = "cannot assess simultaneity" }
        )
        # The old output asserted the strong host model was at fault here.
        ExpectNotMatch = @("strong host model")
    },

    @{
        Name = "bindignored"
        Why  = "REGRESSION: binding broken, so retention is computed against a broken baseline."
        Rows = @(
            (Row "A-wifi" 1 140 0 2 1), (Row "B-tether" 1 168 2 2 1), (Row "C-combined" 1 150 3 4 3),
            (Row "A-wifi" 2 140 0 2 1), (Row "B-tether" 2 165 1 2 1), (Row "C-combined" 2 152 2 4 3)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "FAIL"; Match = "--interface was ignored" },
            @{ State = "SKIP"; Match = "the solo baselines are invalid" }
        )
    },

    @{
        Name = "noranges"
        Why  = "ranges refused: single stream, no 206 anywhere."
        Rows = @(
            (Row "A-wifi" 1 40 0 1 0), (Row "B-tether" 1 0 50 1 0), (Row "C-combined" 1 20 30 1 0),
            (Row "A-wifi" 2 40 0 1 0), (Row "B-tether" 2 0 50 1 0), (Row "C-combined" 2 20 30 1 0)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "FAIL"; Match = "collapsed" }
        )
    },

    @{
        Name = "missingbaseline"
        Why  = "a combined run with no solo runs in its repeat cannot be judged."
        Rows = @(
            (Row "A-wifi" 1 140 0 2 1), (Row "B-tether" 1 0 170 2 1), (Row "C-combined" 1 120 175 4 3),
            (Row "C-combined" 2 118 172 4 3)
        )
        ExpectPass = $false
        Expect = @(
            @{ State = "SKIP"; Match = "a solo baseline for that repeat was zero" }
        )
    }
)

# ------------------------------------------------------------------ runner

$failures = 0

foreach ($sc in $scenarios) {
    Write-Output ""
    Write-Output "=============================================================="
    Write-Output "scenario: $($sc.Name)"
    Write-Output "  $($sc.Why)"

    $v = Get-Phase0Verdict -Results $sc.Rows
    $allText = ($v.Checks | ForEach-Object { $_.Text }) -join " || "

    if ($v.Pass -eq $sc.ExpectPass) {
        Write-Output ("  OK    Pass = {0} as expected" -f $v.Pass)
    } else {
        Write-Output ("  BAD   Pass = {0}, expected {1}" -f $v.Pass, $sc.ExpectPass)
        $failures++
    }

    # @($null) is an array of one null in PowerShell, not an empty array, so an
    # absent key would otherwise run the loop once with $m = $null and match
    # everything. Filter rather than trusting @() to come back empty.
    foreach ($e in @($sc.Expect | Where-Object { $null -ne $_ })) {
        $hit = @($v.Checks | Where-Object { $_.State -eq $e.State -and $_.Text -like "*$($e.Match)*" })
        if ($hit.Count -gt 0) {
            Write-Output ("  OK    {0} check matching '{1}'" -f $e.State, $e.Match)
        } else {
            Write-Output ("  BAD   no {0} check matching '{1}'" -f $e.State, $e.Match)
            $failures++
        }
    }

    foreach ($m in @($sc.ExpectNotMatch | Where-Object { $null -ne $_ })) {
        if ($allText -like "*$m*") {
            Write-Output ("  BAD   output must not mention '{0}'" -f $m)
            $failures++
        } else {
            Write-Output ("  OK    output does not mention '{0}'" -f $m)
        }
    }

    if ($null -ne $sc.ExpectPair) {
        $p = @($v.Pairs | Where-Object { $_.Repeat -eq $sc.ExpectPair.Repeat }) | Select-Object -First 1
        if ($null -eq $p) {
            Write-Output ("  BAD   no pairing row for repeat {0}" -f $sc.ExpectPair.Repeat)
            $failures++
        } elseif ($p.WifiRet -ge $sc.ExpectPair.WifiRetAtLeast) {
            Write-Output ("  OK    repeat {0} paired Wi-Fi retention {1}% >= {2}%" -f $p.Repeat, $p.WifiRet, $sc.ExpectPair.WifiRetAtLeast)
        } else {
            Write-Output ("  BAD   repeat {0} paired Wi-Fi retention {1}%, expected at least {2}% (is it dividing by the global max again?)" -f $p.Repeat, $p.WifiRet, $sc.ExpectPair.WifiRetAtLeast)
            $failures++
        }
    }

    if ($true -eq $sc.ExpectNoiseNull) {
        if ($null -eq $v.Noise.Combined) {
            Write-Output "  OK    combined noise reported as undefined, not 0%"
        } else {
            Write-Output ("  BAD   combined noise reported as {0}, expected undefined" -f $v.Noise.Combined)
            $failures++
        }
    }
}

Write-Output ""
Write-Output "=============================================================="
if ($failures -eq 0) {
    Write-Output "ALL SCENARIOS BEHAVED AS ASSERTED"
    exit 0
} else {
    Write-Output "$failures ASSERTION(S) FAILED"
    exit 1
}
