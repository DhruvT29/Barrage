<#
    BARRAGE phase 0: the decision, separated from the measurement.

    `Get-Phase0Verdict` is pure. It takes the per-run table and returns the
    checks and the verdict, touching no network, no adapter and no disk. The
    runner (phase0-aggregation-test.ps1) performs the runs and calls this; the
    tests (phase0-verdict-tests.ps1) feed it synthetic runs and assert the
    outcome.

    That split is PLAN.md's own rule applied to the tooling: a decision in a
    pure function can be tested without summoning a fake network adapter. It
    exists because the first version of this logic was inline, untestable, and
    wrong in four ways that only showed up once synthetic runs could be fed
    through it.

    EXPECTED INPUT: one object per run, with at least
      Config      "A-wifi" | "B-tether" | "C-combined"
      Repeat      1-based repeat number
      Secs        wall-clock seconds the run lasted
      WifiMbps    from Get-NetAdapterStatistics byte deltas
      TetherMbps  likewise
      TotalMbps   WifiMbps + TetherMbps
      TetherPct   tether share of the total, percent
      MaxCn       peak concurrent connections aria2 reported
      Ok206       count of HTTP 206 responses seen in the log
#>

Set-StrictMode -Off

$script:CfgWifi     = "A-wifi"
$script:CfgTether   = "B-tether"
$script:CfgCombined = "C-combined"

# A run that moved less than this did not measure anything.
$script:VoidRunMb = 1.0

# Each link must keep at least this share of its own solo rate while the other
# link is also running. That is what "both at once" means in numbers.
$script:RetentionFloorPct = 50.0

function New-Check {
    param(
        [ValidateSet("OK", "FAIL", "SKIP")][string]$State,
        [bool]$Gate,
        [string]$Text
    )
    [pscustomobject]@{ State = $State; Gate = $Gate; Text = $Text }
}

function Get-Spread {
    <#
      Percentage spread between the best and worst sample of one configuration.
      Returns $null for fewer than two samples, because one sample does not
      measure variability. Reporting 0% there would read as "perfectly stable"
      when the honest answer is "unknown".
    #>
    param([object[]]$Rows, [string]$Field)

    if ($null -eq $Rows -or @($Rows).Count -lt 2) { return $null }
    $max = ($Rows | Measure-Object -Property $Field -Maximum).Maximum
    $min = ($Rows | Measure-Object -Property $Field -Minimum).Minimum
    if ($max -le 0) { return $null }
    return [math]::Round(((($max - $min) / $max) * 100), 0)
}

function Get-RunBytesMb {
    param($Row)
    return ($Row.TotalMbps * $Row.Secs) / 8.0
}

function Get-Phase0Verdict {
    param([Parameter(Mandatory = $true)][object[]]$Results)

    $rows = @($Results)
    $aRows = @($rows | Where-Object { $_.Config -eq $script:CfgWifi })
    $bRows = @($rows | Where-Object { $_.Config -eq $script:CfgTether })
    $cRows = @($rows | Where-Object { $_.Config -eq $script:CfgCombined })

    $checks = New-Object System.Collections.ArrayList

    # ------------------------------------------------------------- void runs
    # Checked FIRST and reported for what it is. A run that moved no bytes is
    # not evidence that binding was ignored, it is evidence that nothing
    # happened: an unplugged cable, a refusing server, or adapter names that do
    # not match. The previous version blamed the Windows strong host model and
    # sent the reader to the wrong place entirely.
    $voidRuns = @()
    foreach ($row in $rows) {
        if ((Get-RunBytesMb -Row $row) -lt $script:VoidRunMb) {
            $voidRuns += "$($row.Config) r$($row.Repeat)"
        }
    }

    if ($voidRuns.Count -eq 0) {
        [void]$checks.Add((New-Check -State "OK" -Gate $true -Text "every run moved real traffic"))
    } else {
        [void]$checks.Add((New-Check -State "FAIL" -Gate $true -Text (
            "no bytes moved in: $($voidRuns -join ', '). These runs measured nothing, " +
            "so the comparison is void. This is NOT evidence about binding. Check the " +
            "cable and tethering toggle, that the server is reachable, and that the " +
            "adapter names resolved correctly.")))
    }
    $haveTraffic = ($voidRuns.Count -eq 0)

    # --------------------------------------------------------- enough repeats
    # A single repeat per configuration leaves run-to-run noise unmeasured, and
    # PLAN.md phase 0 requires at least two. An unrepeated result is a
    # measurement, not a decision.
    if (@($cRows).Count -ge 2) {
        [void]$checks.Add((New-Check -State "OK" -Gate $true -Text "$(@($cRows).Count) combined runs, so run-to-run noise is measurable"))
    } else {
        [void]$checks.Add((New-Check -State "FAIL" -Gate $true -Text (
            "only $(@($cRows).Count) combined run, so run-to-run noise is unmeasured. " +
            "Re-run with -Repeats 2 or more before treating this as a decision.")))
    }

    # ------------------------------------------------------- ranges honoured
    # CN:1 with no 206 means ranges were refused and every run collapsed to a
    # single stream. CN:2 is NOT a failure: these servers answer 429 to surplus
    # connections while serving ranges correctly.
    $minCn = ($rows | Measure-Object -Property MaxCn -Minimum).Minimum
    $min206 = ($rows | Measure-Object -Property Ok206 -Minimum).Minimum
    if ($minCn -ge 2 -and $min206 -ge 1) {
        [void]$checks.Add((New-Check -State "OK" -Gate $true -Text "range requests honoured in every run, lowest peak CN was $minCn"))
    } else {
        [void]$checks.Add((New-Check -State "FAIL" -Gate $true -Text (
            "a run reached only CN:$minCn with $min206 range responses, so it collapsed " +
            "to a single stream and its numbers are invalid")))
    }

    # ----------------------------------------------------------- bind works
    # Judged on the tether-only runs. Wi-Fi is normally the default route and
    # would carry traffic whether bound or not, so only the tether-only runs
    # can show that --interface did anything at all.
    $bindOk = $false
    if (-not $haveTraffic) {
        [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text "cannot assess --interface binding: a run moved no bytes"))
    } elseif (@($bRows).Count -eq 0) {
        [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text "cannot assess --interface binding: no tether-only runs"))
    } else {
        $leaked = @($bRows | Where-Object { $_.TetherPct -lt 70 })
        if ($leaked.Count -eq 0) {
            $bindOk = $true
            [void]$checks.Add((New-Check -State "OK" -Gate $true -Text "--interface binds, the tether-only runs went out the tether"))
        } else {
            [void]$checks.Add((New-Check -State "FAIL" -Gate $true -Text (
                "a tether-only run sent most bytes over Wi-Fi, so --interface was ignored " +
                "and every comparison here is Wi-Fi against itself. Suspect the Windows " +
                "strong host model.")))
        }
    }

    # ------------------------------------------------- simultaneity, paired
    # THE DECISIVE CHECK, and the one that was wrong.
    #
    # Each combined run is compared against the solo runs from ITS OWN repeat,
    # so numerator and denominator come from the same network conditions. The
    # previous version divided by the best solo rate seen anywhere in the
    # session, which on a link that varied 78% run to run meant comparing a
    # measurement against one taken minutes later under different conditions.
    #
    # And every combined run must pass, not merely the best one. The previous
    # version sorted combined runs by total and judged only the winner, so a
    # run with one link starved to 4% of its solo rate was never looked at.
    $pairs = New-Object System.Collections.ArrayList
    foreach ($c in $cRows) {
        $a = @($aRows | Where-Object { $_.Repeat -eq $c.Repeat }) | Select-Object -First 1
        $b = @($bRows | Where-Object { $_.Repeat -eq $c.Repeat }) | Select-Object -First 1

        $wifiSolo = 0.0
        $tetherSolo = 0.0
        if ($null -ne $a) { $wifiSolo = [double]$a.WifiMbps }
        if ($null -ne $b) { $tetherSolo = [double]$b.TetherMbps }

        $wifiRet = $null
        $tetherRet = $null
        if ($wifiSolo -gt 0) { $wifiRet = [math]::Round((([double]$c.WifiMbps / $wifiSolo) * 100), 0) }
        if ($tetherSolo -gt 0) { $tetherRet = [math]::Round((([double]$c.TetherMbps / $tetherSolo) * 100), 0) }

        $usable = ($null -ne $wifiRet -and $null -ne $tetherRet)
        $ok = $false
        if ($usable) {
            $ok = ($wifiRet -ge $script:RetentionFloorPct -and $tetherRet -ge $script:RetentionFloorPct)
        }

        [void]$pairs.Add([pscustomobject]@{
            Repeat     = $c.Repeat
            WifiSolo   = [math]::Round($wifiSolo, 1)
            TetherSolo = [math]::Round($tetherSolo, 1)
            CombWifi   = [math]::Round([double]$c.WifiMbps, 1)
            CombTether = [math]::Round([double]$c.TetherMbps, 1)
            WifiRet    = $wifiRet
            TetherRet  = $tetherRet
            Usable     = $usable
            Ok         = $ok
        })
    }

    if (-not $haveTraffic) {
        [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text "cannot assess simultaneity: a run moved no bytes"))
    } elseif (-not $bindOk) {
        # Dependent on the bind check. With a broken baseline the ratio is
        # computed against the same broken measurement and lands near 100%,
        # which previously printed a confident retention figure directly under
        # a FAIL saying binding did not work.
        [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text "cannot assess simultaneity: the solo baselines are invalid because --interface did not bind"))
    } elseif (@($pairs).Count -eq 0) {
        [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text "cannot assess simultaneity: no combined runs"))
    } else {
        $unusable = @($pairs | Where-Object { -not $_.Usable })
        $failed = @($pairs | Where-Object { $_.Usable -and -not $_.Ok })

        if ($unusable.Count -gt 0) {
            [void]$checks.Add((New-Check -State "SKIP" -Gate $true -Text (
                "cannot assess simultaneity for repeat(s) $(($unusable | ForEach-Object { $_.Repeat }) -join ', '): " +
                "a solo baseline for that repeat was zero")))
        } elseif ($failed.Count -eq 0) {
            $worst = $pairs | Sort-Object { [math]::Min($_.WifiRet, $_.TetherRet) } | Select-Object -First 1
            [void]$checks.Add((New-Check -State "OK" -Gate $true -Text (
                "both links ran at once in every combined run. Worst case was repeat " +
                "$($worst.Repeat): Wi-Fi kept $($worst.WifiRet)%, tether kept $($worst.TetherRet)% " +
                "of their own repeat's solo rates")))
        } else {
            $detail = ($failed | ForEach-Object { "repeat $($_.Repeat) (wifi $($_.WifiRet)%, tether $($_.TetherRet)%)" }) -join "; "
            [void]$checks.Add((New-Check -State "FAIL" -Gate $true -Text (
                "a link was starved in $detail. Below $($script:RetentionFloorPct)% means the chunks " +
                "went to one link rather than both.")))
        }
    }

    # -------------------------------------------------- totals, advisory only
    # Demoted from a gate because with unequal links the expected gain is
    # routinely smaller than the run-to-run noise. Reported for context.
    $maxWifiSolo = 0.0
    $maxTetherSolo = 0.0
    if (@($aRows).Count -gt 0) { $maxWifiSolo = [double]($aRows | Measure-Object -Property WifiMbps -Maximum).Maximum }
    if (@($bRows).Count -gt 0) { $maxTetherSolo = [double]($bRows | Measure-Object -Property TetherMbps -Maximum).Maximum }

    $bestCombined = 0.0
    if (@($cRows).Count -gt 0) { $bestCombined = [double]($cRows | Measure-Object -Property TotalMbps -Maximum).Maximum }

    $sumSolo = $maxWifiSolo + $maxTetherSolo
    $bestSingle = [math]::Max($maxWifiSolo, $maxTetherSolo)
    $midpoint = ($sumSolo + $bestSingle) / 2.0

    $gain = $null
    $idealGain = $null
    if ($bestSingle -gt 0) {
        $gain = [math]::Round(((($bestCombined - $bestSingle) / $bestSingle) * 100), 0)
        $idealGain = [math]::Round(((($sumSolo - $bestSingle) / $bestSingle) * 100), 0)
    }

    $noise = [pscustomobject]@{
        Wifi     = Get-Spread -Rows $aRows -Field "WifiMbps"
        Tether   = Get-Spread -Rows $bRows -Field "TetherMbps"
        Combined = Get-Spread -Rows $cRows -Field "TotalMbps"
    }

    if ($sumSolo -le 0) {
        # Previously this passed vacuously on 0 >= 0 and printed OK for a
        # session in which nothing moved.
        [void]$checks.Add((New-Check -State "SKIP" -Gate $false -Text "no throughput comparison possible: both solo baselines were zero"))
    } else {
        $note = ""
        $noiseValues = @($noise.Wifi, $noise.Tether, $noise.Combined) | Where-Object { $null -ne $_ }
        if (@($noiseValues).Count -gt 0) {
            $worstNoise = ($noiseValues | Measure-Object -Maximum).Maximum
            if ($null -ne $idealGain -and $idealGain -le $worstNoise) {
                $note = " (cannot resolve: perfect aggregation would only be $idealGain%, inside the $worstNoise% noise band)"
            }
        }
        if ($bestCombined -ge $midpoint) {
            [void]$checks.Add((New-Check -State "OK" -Gate $false -Text "total throughput landed above the halfway mark between one link and both summed$note"))
        } else {
            [void]$checks.Add((New-Check -State "FAIL" -Gate $false -Text "total throughput did not reach the halfway mark between one link and both summed$note"))
        }
    }

    $gateChecks = @($checks | Where-Object { $_.Gate })
    $pass = (@($gateChecks | Where-Object { $_.State -ne "OK" }).Count -eq 0)

    return [pscustomobject]@{
        Pass     = $pass
        Checks   = @($checks)
        Pairs    = @($pairs)
        VoidRuns = @($voidRuns)
        Noise    = $noise
        Totals   = [pscustomobject]@{
            MaxWifiSolo   = [math]::Round($maxWifiSolo, 1)
            MaxTetherSolo = [math]::Round($maxTetherSolo, 1)
            BestSingle    = [math]::Round($bestSingle, 1)
            SumSolo       = [math]::Round($sumSolo, 1)
            BestCombined  = [math]::Round($bestCombined, 1)
            Midpoint      = [math]::Round($midpoint, 1)
            Gain          = $gain
            IdealGain     = $idealGain
        }
    }
}

function Format-Pct {
    param($Value)
    if ($null -eq $Value) { return "n/a" }
    return "$Value%"
}

function Write-Phase0Verdict {
    param([Parameter(Mandatory = $true)]$Verdict)

    $t = $Verdict.Totals

    Write-Output "=========================== verdict ============================"
    Write-Output ("Wi-Fi solo        best {0,7} Mbps   run-to-run spread {1}" -f $t.MaxWifiSolo, (Format-Pct $Verdict.Noise.Wifi))
    Write-Output ("Tether solo       best {0,7} Mbps   run-to-run spread {1}" -f $t.MaxTetherSolo, (Format-Pct $Verdict.Noise.Tether))
    Write-Output ("Combined total    best {0,7} Mbps   run-to-run spread {1}" -f $t.BestCombined, (Format-Pct $Verdict.Noise.Combined))
    if ($null -eq $Verdict.Noise.Wifi -or $null -eq $Verdict.Noise.Tether -or $null -eq $Verdict.Noise.Combined) {
        Write-Output "                  n/a means a single run: variability is unmeasured, not zero"
    }
    Write-Output ""

    if (@($Verdict.Pairs).Count -gt 0) {
        Write-Output "Per-repeat pairing. Each combined run against ITS OWN repeat's solo runs,"
        Write-Output "so numerator and denominator share the same network conditions:"
        Write-Output ""
        $Verdict.Pairs |
            Select-Object Repeat, WifiSolo, CombWifi, WifiRet, TetherSolo, CombTether, TetherRet, Ok |
            Format-Table -AutoSize | Out-String -Width 200 | Write-Output
    }

    Write-Output ("Observed gain     {0}  over the better single link" -f (Format-Pct $t.Gain))
    Write-Output ("Perfect would be  {0}  (solo rates summed)" -f (Format-Pct $t.IdealGain))
    Write-Output ""

    foreach ($chk in $Verdict.Checks) {
        $kind = "advisory"
        if ($chk.Gate) { $kind = "gate    " }
        Write-Output ("  {0,-4}  {1}  {2}" -f $chk.State, $kind, $chk.Text)
    }

    Write-Output ""
    if ($Verdict.Pass) {
        Write-Output "PASS. Per-socket source binding aggregates on this machine. Record the"
        Write-Output "numbers in PLAN.md section 9, close BRG-13, and phase 1 may start."
    } else {
        Write-Output "FAIL. Phase 0 is not satisfied. A SKIP is not a pass: it means the"
        Write-Output "measurement could not answer the question, which needs fixing before"
        Write-Output "the result means anything."
    }
}
