<#
    BARRAGE phase 0 (BRG-13): does binding sockets to two interfaces actually
    aggregate bandwidth on this machine?

    Runs three configurations at a CONSTANT sixteen requested connections:
      A  Wi-Fi only     aria2c --interface=<wifi>
      B  tether only    aria2c --interface=<tether>
      C  both           aria2c --multiple-interface=<wifi>,<tether>

    Holding the requested connection count constant is the method. A sixteen
    connection combined run measured against a one connection baseline would
    only prove that parallel connections help, which is already known.

    Configurations are interleaved A,B,C,A,B,C rather than A,A,B,B,C,C so that
    network drift over the session hits each configuration equally instead of
    favouring whichever ran first.

    WHAT IS ACTUALLY MEASURED, and why it is not total throughput.

    On this machine Wi-Fi runs at roughly 226 Mbps and a 4G tether at roughly
    25 Mbps. Perfect aggregation therefore shows up as an 11 percent gain in
    total throughput, which a shared building network's run-to-run noise
    swamps. Comparing totals cannot answer the question here.

    Get-NetAdapterStatistics gives each link's throughput SEPARATELY inside the
    combined run. The decisive test is whether each link delivers most of its
    solo rate at the same time as the other. A 20 percent wobble on Wi-Fi does
    not touch the tether's byte counter, so this is robust where a totals
    comparison is not. Total gain is still reported, as corroboration.

    Usage:
      .\phase0-aggregation-test.ps1 -WifiIp 192.168.166.118 -TetherIp 192.168.42.129

    Mobile data cost: about -Seconds * tether-Mbps / 8 MB per tether run, so
    roughly 400 MB in total at the defaults. Pass -Seconds 20 to cut it.
#>

param(
    [Parameter(Mandatory = $true)][string]$WifiIp,
    [Parameter(Mandatory = $true)][string]$TetherIp,
    [string]$Url = "https://sin-speed.hetzner.com/10GB.bin",
    [int]$Seconds = 30,
    [int]$Repeats = 2,
    [string]$Aria = ""
)

$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------- preflight

if (-not $Aria) {
    $cmd = Get-Command aria2c -ErrorAction SilentlyContinue
    if ($null -ne $cmd) {
        $Aria = $cmd.Source
    } else {
        $fallback = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links\aria2c.exe"
        if (Test-Path $fallback) { $Aria = $fallback }
    }
}
if (-not $Aria -or -not (Test-Path $Aria)) {
    throw "aria2c not found. Run: winget install aria2.aria2"
}

function Resolve-Adapter {
    param([string]$Ip, [string]$What)
    $addr = Get-NetIPAddress -AddressFamily IPv4 -IPAddress $Ip -ErrorAction SilentlyContinue
    if ($null -eq $addr) {
        throw "$What address $Ip is not on this machine. Re-check with: Get-NetIPAddress -AddressFamily IPv4"
    }
    if ($Ip.StartsWith("169.254.")) {
        throw "$What address $Ip is link-local, meaning DHCP failed. That interface has no internet."
    }
    return @($addr)[0].InterfaceAlias
}

$wifiAdapter   = Resolve-Adapter -Ip $WifiIp   -What "Wi-Fi"
$tetherAdapter = Resolve-Adapter -Ip $TetherIp -What "Tether"

if ($wifiAdapter -eq $tetherAdapter) {
    throw "Both addresses are on adapter '$wifiAdapter'. Two separate interfaces are required."
}

# A ranged GET, not HEAD: these speed-test servers close the connection on HEAD,
# and a ranged GET tests the exact capability the whole project depends on.
Write-Output "Preflight: asking the server for bytes 0-1023 ..."
$probe = & curl.exe -s -o NUL -r 0-1023 --max-time 25 -w "%{http_code} %{size_download}" $Url
$parts = $probe.Trim().Split(" ")
if ($parts[0] -ne "206" -or $parts[1] -ne "1024") {
    throw "Range probe returned '$probe', expected '206 1024'. Without range support every run collapses to a single stream and all numbers are meaningless. Pick another URL."
}
Write-Output "  HTTP 206, 1024 bytes. Range requests supported."

$work = Join-Path $env:TEMP "brg-phase0"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$free = [math]::Round((Get-PSDrive -Name ($work.Substring(0,1))).Free / 1GB, 1)
Write-Output ""
Write-Output "aria2        $Aria"
Write-Output "URL          $Url"
Write-Output "Wi-Fi        $WifiIp on '$wifiAdapter'"
Write-Output "Tether       $TetherIp on '$tetherAdapter'"
Write-Output "Window       $Seconds s per run, $Repeats repeats, $($Repeats * 3) runs"
Write-Output "Scratch      $work  ($free GB free, each run's file is deleted straight after)"
Write-Output ""
Write-Output "Close anything else using the network now. Other traffic lands in the"
Write-Output "same adapter counters and is indistinguishable from download throughput."
Write-Output ""

# ---------------------------------------------------------------- parsing

function Get-AriaSamples {
    param([string]$LogPath)

    $speeds = New-Object System.Collections.ArrayList
    $maxCn = 0
    $n206 = 0
    $n429 = 0

    if (-not (Test-Path $LogPath)) {
        return [pscustomobject]@{ Median = 0.0; MaxCn = 0; Ok206 = 0; Throttled = 0 }
    }

    foreach ($line in (Get-Content -LiteralPath $LogPath)) {
        if ($line -match '^HTTP/1\.[01] 206') { $n206++ }
        if ($line -match '^HTTP/1\.[01] 429') { $n429++ }

        $cn = [regex]::Match($line, 'CN:(\d+)')
        if ($cn.Success) {
            $v = [int]$cn.Groups[1].Value
            if ($v -gt $maxCn) { $maxCn = $v }
        }
        $dl = [regex]::Match($line, 'DL:([0-9.]+)(Ki|Mi|Gi)?B')
        if ($dl.Success) {
            $n = [double]$dl.Groups[1].Value
            switch ($dl.Groups[2].Value) {
                'Ki'    { $n = $n / 1024 }
                'Mi'    { }
                'Gi'    { $n = $n * 1024 }
                default { $n = $n / 1048576 }
            }
            [void]$speeds.Add($n)
        }
    }

    $median = 0.0
    if ($speeds.Count -gt 0) {
        # Drop the first 40 percent of samples. TCP slow start and connection
        # setup are not the steady state rate being measured.
        $keepCount = [int][math]::Max(1, [math]::Floor($speeds.Count * 0.6))
        $keep = @($speeds | Select-Object -Last $keepCount | Sort-Object)
        $median = $keep[[int][math]::Floor($keep.Count / 2)]
    }

    return [pscustomobject]@{
        Median    = [math]::Round($median * 8.388608, 1)   # MiB/s -> Mbps
        MaxCn     = $maxCn
        Ok206     = $n206
        Throttled = $n429
    }
}

# ---------------------------------------------------------------- one run

function Invoke-AriaRun {
    param([string]$Config, [string[]]$BindArgs, [int]$Repeat)

    $tag = "$Config-r$Repeat"
    $out = Join-Path $work "$tag.bin"
    $log = Join-Path $work "$tag.stdout.txt"
    $err = Join-Path $work "$tag.stderr.txt"
    Remove-Item -LiteralPath $out, "$out.aria2" -Force -ErrorAction SilentlyContinue

    # --disable-ipv6: binding an IPv4 source address and then connecting to a
    # dual-stack host's IPv6 address fails on Windows and wastes an attempt per
    # link. Phase 2 has to solve this properly; here it is simply excluded.
    $ariaArgs = @(
        "-x16", "-s16", "-k1M",
        "--stop=$Seconds",
        "--summary-interval=1",
        "--console-log-level=info",
        "--disable-ipv6=true",
        "--file-allocation=none",
        "--allow-overwrite=true",
        "--auto-file-renaming=false",
        "--dir=$work",
        "-o", "$tag.bin"
    ) + $BindArgs + @($Url)

    $beforeWifi   = (Get-NetAdapterStatistics -Name $wifiAdapter).ReceivedBytes
    $beforeTether = (Get-NetAdapterStatistics -Name $tetherAdapter).ReceivedBytes
    $watch = [System.Diagnostics.Stopwatch]::StartNew()

    # Start-Process rather than the call operator: PowerShell 5.1 wraps a native
    # executable's redirected stderr in ErrorRecords, which would trip
    # $ErrorActionPreference = Stop even on a clean exit.
    Start-Process -FilePath $Aria -ArgumentList $ariaArgs -NoNewWindow -Wait `
                  -RedirectStandardOutput $log -RedirectStandardError $err | Out-Null

    $watch.Stop()
    $elapsed  = $watch.Elapsed.TotalSeconds
    $wifiMb   = ((Get-NetAdapterStatistics -Name $wifiAdapter).ReceivedBytes   - $beforeWifi)   / 1MB
    $tetherMb = ((Get-NetAdapterStatistics -Name $tetherAdapter).ReceivedBytes - $beforeTether) / 1MB
    $parsed   = Get-AriaSamples -LogPath $log

    Remove-Item -LiteralPath $out, "$out.aria2" -Force -ErrorAction SilentlyContinue

    $wifiMbps   = ($wifiMb   * 1MB * 8) / 1e6 / $elapsed
    $tetherMbps = ($tetherMb * 1MB * 8) / 1e6 / $elapsed
    $totalMbps  = $wifiMbps + $tetherMbps
    $share = 0.0
    if ($totalMbps -gt 0) { $share = ($tetherMbps / $totalMbps) * 100 }

    return [pscustomobject]@{
        Config      = $Config
        Repeat      = $Repeat
        Secs        = [math]::Round($elapsed, 1)
        WifiMbps    = [math]::Round($wifiMbps, 1)
        TetherMbps  = [math]::Round($tetherMbps, 1)
        TotalMbps   = [math]::Round($totalMbps, 1)
        AriaMbps    = $parsed.Median
        TetherPct   = [math]::Round($share, 0)
        MaxCn       = $parsed.MaxCn
        Ok206       = $parsed.Ok206
        Throttled   = $parsed.Throttled
    }
}

# ---------------------------------------------------------------- the runs

$configs = @(
    @{ Name = "A-wifi";     Args = @("--interface=$WifiIp") },
    @{ Name = "B-tether";   Args = @("--interface=$TetherIp") },
    @{ Name = "C-combined"; Args = @("--multiple-interface=$WifiIp,$TetherIp") }
)

$results = New-Object System.Collections.ArrayList

for ($r = 1; $r -le $Repeats; $r++) {
    foreach ($c in $configs) {
        Write-Output "run $($c.Name) repeat $r ..."
        $res = Invoke-AriaRun -Config $c.Name -BindArgs $c.Args -Repeat $r
        [void]$results.Add($res)
        Write-Output ("  wifi {0,7} Mbps | tether {1,6} Mbps | total {2,7} Mbps | aria2 says {3,7} | CN {4} | 206s {5} | 429s {6}" -f `
            $res.WifiMbps, $res.TetherMbps, $res.TotalMbps, $res.AriaMbps, $res.MaxCn, $res.Ok206, $res.Throttled)
    }
}

# ---------------------------------------------------------------- verdict

Write-Output ""
Write-Output "=========================== all runs ==========================="
$results | Format-Table Config, Repeat, Secs, WifiMbps, TetherMbps, TotalMbps, AriaMbps, TetherPct, MaxCn, Ok206, Throttled -AutoSize |
    Out-String -Width 200 | Write-Output

function Get-Spread {
    param([object[]]$Rows, [string]$Field)
    $max = ($Rows | Measure-Object -Property $Field -Maximum).Maximum
    $min = ($Rows | Measure-Object -Property $Field -Minimum).Minimum
    $spread = 0.0
    if ($max -gt 0) { $spread = (($max - $min) / $max) * 100 }
    return [pscustomobject]@{ Max = [double]$max; Min = [double]$min; Spread = [math]::Round($spread, 0) }
}

$aRows = @($results | Where-Object { $_.Config -eq "A-wifi" })
$bRows = @($results | Where-Object { $_.Config -eq "B-tether" })
$cRows = @($results | Where-Object { $_.Config -eq "C-combined" })

$soloWifi   = Get-Spread -Rows $aRows -Field "WifiMbps"
$soloTether = Get-Spread -Rows $bRows -Field "TetherMbps"
$combTotal  = Get-Spread -Rows $cRows -Field "TotalMbps"

# Best combined run judged by total, then read its per-link split.
$bestC = $cRows | Sort-Object -Property TotalMbps -Descending | Select-Object -First 1

$sumSolo   = $soloWifi.Max + $soloTether.Max
$bestSingle = [math]::Max($soloWifi.Max, $soloTether.Max)
$gain = 0.0
if ($bestSingle -gt 0) { $gain = (($combTotal.Max - $bestSingle) / $bestSingle) * 100 }
$idealGain = 0.0
if ($bestSingle -gt 0) { $idealGain = (($sumSolo - $bestSingle) / $bestSingle) * 100 }

$wifiRetain = 0.0
$tetherRetain = 0.0
if ($soloWifi.Max   -gt 0) { $wifiRetain   = ($bestC.WifiMbps   / $soloWifi.Max)   * 100 }
if ($soloTether.Max -gt 0) { $tetherRetain = ($bestC.TetherMbps / $soloTether.Max) * 100 }

$noise = [math]::Max($soloWifi.Spread, [math]::Max($soloTether.Spread, $combTotal.Spread))

Write-Output "=========================== verdict ============================"
Write-Output ("Wi-Fi solo        best {0,7} Mbps   run-to-run spread {1}%" -f $soloWifi.Max, $soloWifi.Spread)
Write-Output ("Tether solo       best {0,7} Mbps   run-to-run spread {1}%" -f $soloTether.Max, $soloTether.Spread)
Write-Output ("Combined total    best {0,7} Mbps   run-to-run spread {1}%" -f $combTotal.Max, $combTotal.Spread)
Write-Output ""
Write-Output ("Best combined run split: Wi-Fi {0} Mbps + tether {1} Mbps" -f $bestC.WifiMbps, $bestC.TetherMbps)
Write-Output ("  Wi-Fi kept   {0}% of its solo rate while the tether was also running" -f [math]::Round($wifiRetain, 0))
Write-Output ("  Tether kept  {0}% of its solo rate while Wi-Fi was also running" -f [math]::Round($tetherRetain, 0))
Write-Output ""
Write-Output ("Noise band        {0}%  (largest spread within one configuration)" -f $noise)
Write-Output ("Observed gain     {0}%  over the better single link" -f [math]::Round($gain, 0))
Write-Output ("Perfect would be  {0}%  (solo rates summed)" -f [math]::Round($idealGain, 0))
Write-Output ""

$checks = New-Object System.Collections.ArrayList

# 1. Ranges honoured. CN:1 with no 206 means refused. CN:2 is NOT a failure:
#    these servers 429 surplus connections while serving ranges correctly.
$minCn = ($results | Measure-Object -Property MaxCn -Minimum).Minimum
$min206 = ($results | Measure-Object -Property Ok206 -Minimum).Minimum
if ($minCn -ge 2 -and $min206 -ge 1) {
    [void]$checks.Add(@{ Ok = $true; Gate = $true;
        Text = "range requests honoured in every run, lowest peak CN was $minCn" })
} else {
    [void]$checks.Add(@{ Ok = $false; Gate = $true;
        Text = "a run reached only CN:$minCn with $min206 range responses, so it collapsed to a single stream and its numbers are invalid" })
}

# 2. Did --interface actually bind? Judged on the tether-only runs, because
#    Wi-Fi is the default route and would carry traffic whether bound or not.
$leaked = @($bRows | Where-Object { $_.TetherPct -lt 70 })
if ($leaked.Count -eq 0) {
    [void]$checks.Add(@{ Ok = $true; Gate = $true;
        Text = "--interface binds, the tether-only runs went out the tether" })
} else {
    [void]$checks.Add(@{ Ok = $false; Gate = $true;
        Text = "a tether-only run sent most bytes over Wi-Fi, so --interface was ignored and every comparison here is Wi-Fi against itself. Suspect the Windows strong host model" })
}

# 3. THE DECISIVE CHECK. Both links delivering most of their solo rate at the
#    same time is aggregation, measured per-adapter and immune to noise in the
#    total.
if ($wifiRetain -ge 50 -and $tetherRetain -ge 50) {
    [void]$checks.Add(@{ Ok = $true; Gate = $true;
        Text = ("both links ran at once: Wi-Fi kept {0}% and the tether kept {1}% of their solo rates" -f [math]::Round($wifiRetain,0), [math]::Round($tetherRetain,0)) })
} else {
    [void]$checks.Add(@{ Ok = $false; Gate = $true;
        Text = ("one link was starved in the combined run: Wi-Fi kept {0}%, tether kept {1}%. Under 50% means the chunks went to one link, not both" -f [math]::Round($wifiRetain,0), [math]::Round($tetherRetain,0)) })
}

# 4. Corroborating only. Demoted from a gate because when the links are very
#    unequal, perfect aggregation produces a gain smaller than the noise band.
$advisoryNote = ""
if ($idealGain -le $noise) {
    $advisoryNote = " (cannot resolve: perfect aggregation would only be $([math]::Round($idealGain,0))%, inside the $noise% noise band)"
}
$midpoint = ($sumSolo + $bestSingle) / 2
if ($combTotal.Max -ge $midpoint) {
    [void]$checks.Add(@{ Ok = $true; Gate = $false;
        Text = "total throughput landed above the halfway mark between one link and both summed$advisoryNote" })
} else {
    [void]$checks.Add(@{ Ok = $false; Gate = $false;
        Text = "total throughput did not reach the halfway mark between one link and both summed$advisoryNote" })
}

foreach ($chk in $checks) {
    $kind = "advisory"
    if ($chk.Gate) { $kind = "gate    " }
    if ($chk.Ok) { Write-Output "  OK    $kind  $($chk.Text)" } else { Write-Output "  FAIL  $kind  $($chk.Text)" }
}

$failedGates = @($checks | Where-Object { -not $_.Ok -and $_.Gate })
Write-Output ""
if ($failedGates.Count -eq 0) {
    Write-Output "PASS. Per-socket source binding aggregates on this machine. Record the"
    Write-Output "numbers in PLAN.md section 9, close BRG-13, and phase 1 may start."
} else {
    Write-Output "FAIL. Do not start BRG-2. PLAN.md phase 0 lists the causes to separate:"
    Write-Output "the Windows strong host model, the shared building network being the real"
    Write-Output "bottleneck rather than either link, or the server capping us."
}

$transcript = Join-Path $work "phase0-results.csv"
$results | Export-Csv -LiteralPath $transcript -NoTypeInformation
Write-Output ""
Write-Output "Per-run rows   $transcript"
Write-Output "aria2 logs     $work\*.stdout.txt"
