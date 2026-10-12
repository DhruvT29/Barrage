<#
    BARRAGE phase 0 (BRG-13): does binding sockets to two interfaces actually
    aggregate bandwidth on this machine?

    This script PERFORMS the runs. The decision lives in phase0-verdict.ps1 as
    a pure function, and phase0-verdict-tests.ps1 asserts its behaviour against
    synthetic runs. That split is PLAN.md's own rule applied to the tooling: a
    decision in a pure function can be tested without a network.

    Three configurations at a CONSTANT sixteen requested connections:
      A  Wi-Fi only     aria2c --interface=<wifi>
      B  tether only    aria2c --interface=<tether>
      C  both           aria2c --multiple-interface=<wifi>,<tether>

    Holding the requested connection count constant is the method. A sixteen
    connection combined run measured against a one connection baseline would
    only prove that parallel connections help, which is already known.

    Configurations are interleaved A,B,C,A,B,C rather than A,A,B,B,C,C so that
    network drift over the session hits each configuration equally instead of
    favouring whichever ran first. The verdict pairs each combined run with the
    solo runs from ITS OWN repeat for the same reason.

    USAGE
      # First: measure the tether's real rate and project the data cost.
      .\tools\phase0-aggregation-test.ps1 -Probe -TetherIp 192.168.155.162

      # Then the real thing.
      .\tools\phase0-aggregation-test.ps1 -WifiIp 192.168.0.116 -TetherIp 192.168.155.162

    MOBILE DATA. Run -Probe first and believe its projection rather than any
    assumption about the link. The original estimate for this test was 400 MB
    based on the plan's guess of a 25 Mbps tether; the tether actually ran at
    about 170 Mbps and the test consumed roughly 2.5 GB, a six-fold miss.
#>

param(
    [string]$WifiIp,
    [Parameter(Mandatory = $true)][string]$TetherIp,
    [string]$Url = "https://sin-speed.hetzner.com/10GB.bin",
    [int]$Seconds = 30,
    [int]$Repeats = 2,
    [switch]$Probe,
    [int]$ProbeSeconds = 6,
    [string]$Aria = ""
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "phase0-verdict.ps1")

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

# curl is used for the range probe. Shipped with Windows 10 1803 and later, but
# check rather than fail later with something cryptic.
$curlCmd = Get-Command curl.exe -ErrorAction SilentlyContinue
if ($null -eq $curlCmd) {
    throw "curl.exe not found, which the range preflight needs. It ships with Windows 10 1803 and later."
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

$tetherAdapter = Resolve-Adapter -Ip $TetherIp -What "Tether"

$work = Join-Path $env:TEMP "brg-phase0"
New-Item -ItemType Directory -Force -Path $work | Out-Null

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
        # Matches HTTP/1.0, HTTP/1.1 and HTTP/2 status lines. The previous
        # version only matched HTTP/1.x, so a server negotiating h2 would have
        # reported zero range responses and marked valid runs invalid.
        if ($line -match '^HTTP/\S+\s+206') { $n206++ }
        if ($line -match '^HTTP/\S+\s+429') { $n429++ }

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
        # setup took about ten seconds of a thirty second window when measured,
        # and that ramp is not the steady state rate being reported.
        $keepCount = [int][math]::Max(1, [math]::Floor($speeds.Count * 0.6))
        $keep = @($speeds | Select-Object -Last $keepCount | Sort-Object)
        if ($keep.Count % 2 -eq 1) {
            $median = $keep[[int][math]::Floor($keep.Count / 2)]
        } else {
            # Average the two middle samples. Taking the upper one, as the
            # previous version did, biases the figure upward.
            $hi = [int]($keep.Count / 2)
            $median = ($keep[$hi - 1] + $keep[$hi]) / 2.0
        }
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
    param(
        [string]$Config,
        [string[]]$BindArgs,
        [int]$Repeat,
        [int]$Duration,
        [string]$WifiAdapterName,
        [string]$TetherAdapterName
    )

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
        "--stop=$Duration",
        "--summary-interval=1",
        "--console-log-level=info",
        "--disable-ipv6=true",
        "--file-allocation=none",
        "--allow-overwrite=true",
        "--auto-file-renaming=false",
        "--dir=$work",
        "-o", "$tag.bin"
    ) + $BindArgs + @($Url)

    $beforeWifi = 0
    if ($WifiAdapterName) { $beforeWifi = (Get-NetAdapterStatistics -Name $WifiAdapterName).ReceivedBytes }
    $beforeTether = (Get-NetAdapterStatistics -Name $TetherAdapterName).ReceivedBytes
    $watch = [System.Diagnostics.Stopwatch]::StartNew()

    # Start-Process rather than the call operator: PowerShell 5.1 wraps a native
    # executable's redirected stderr in ErrorRecords, which would trip
    # $ErrorActionPreference = Stop even on a clean exit.
    Start-Process -FilePath $Aria -ArgumentList $ariaArgs -NoNewWindow -Wait `
                  -RedirectStandardOutput $log -RedirectStandardError $err | Out-Null

    $watch.Stop()
    $elapsed = $watch.Elapsed.TotalSeconds

    $wifiMiB = 0.0
    if ($WifiAdapterName) {
        $wifiMiB = ((Get-NetAdapterStatistics -Name $WifiAdapterName).ReceivedBytes - $beforeWifi) / 1MB
    }
    $tetherMiB = ((Get-NetAdapterStatistics -Name $TetherAdapterName).ReceivedBytes - $beforeTether) / 1MB
    $parsed = Get-AriaSamples -LogPath $log

    Remove-Item -LiteralPath $out, "$out.aria2" -Force -ErrorAction SilentlyContinue

    # 1MB is 1048576 in PowerShell, so these are mebibytes. Labelled MiB rather
    # than MB, because the data-usage figures quoted elsewhere derive from them.
    $wifiMbps = ($wifiMiB * 1MB * 8) / 1e6 / $elapsed
    $tetherMbps = ($tetherMiB * 1MB * 8) / 1e6 / $elapsed
    $totalMbps = $wifiMbps + $tetherMbps
    $share = 0.0
    if ($totalMbps -gt 0) { $share = ($tetherMbps / $totalMbps) * 100 }

    return [pscustomobject]@{
        Config     = $Config
        Repeat     = $Repeat
        Secs       = [math]::Round($elapsed, 1)
        WifiMbps   = [math]::Round($wifiMbps, 1)
        TetherMbps = [math]::Round($tetherMbps, 1)
        TotalMbps  = [math]::Round($totalMbps, 1)
        AriaMbps   = $parsed.Median
        WifiMiB    = [math]::Round($wifiMiB, 1)
        TetherMiB  = [math]::Round($tetherMiB, 1)
        TetherPct  = [math]::Round($share, 0)
        MaxCn      = $parsed.MaxCn
        Ok206      = $parsed.Ok206
        Throttled  = $parsed.Throttled
    }
}

# ---------------------------------------------------------------- probe mode

if ($Probe) {
    Write-Output "Probing the tether's real rate for $ProbeSeconds s, bound to $TetherIp on '$tetherAdapter'."
    Write-Output "This exists because assuming the rate produced a six-fold error in the"
    Write-Output "data estimate last time. Measure, then decide the window."
    Write-Output ""

    $p = Invoke-AriaRun -Config "probe" -BindArgs @("--interface=$TetherIp") -Repeat 0 `
                        -Duration $ProbeSeconds -WifiAdapterName "" -TetherAdapterName $tetherAdapter

    Write-Output ("tether measured   {0} Mbps steady state, {1} Mbps whole-run average" -f $p.AriaMbps, $p.TetherMbps)
    Write-Output ("probe itself used {0} MiB of mobile data" -f $p.TetherMiB)
    Write-Output ""

    $rate = $p.AriaMbps
    if ($rate -le 0) { $rate = $p.TetherMbps }
    if ($rate -le 0) {
        Write-Output "The probe moved no data. Check tethering is on and the cable carries data."
        return
    }

    Write-Output "Projected MOBILE data for a full run, at this rate:"
    Write-Output ""
    Write-Output "  window  repeats   tether-solo   combined(est 60%)   TOTAL mobile"
    foreach ($w in @(15, 20, 25, 30)) {
        foreach ($rep in @(2, 3)) {
            $perRun = ($rate * $w) / 8.0
            $solo = $perRun * $rep
            $comb = $perRun * 0.6 * $rep
            Write-Output ("  {0,4}s   {1,5}     {2,8} MB   {3,13} MB   {4,9} MB" -f `
                $w, $rep, [math]::Round($solo, 0), [math]::Round($comb, 0), [math]::Round($solo + $comb, 0))
        }
    }
    Write-Output ""
    Write-Output "Wi-Fi traffic is additional but unmetered. A window under about 20 s leaves"
    Write-Output "little steady state after the ramp, so prefer fewer repeats over a shorter"
    Write-Output "window if the budget is tight, bearing in mind the verdict needs at least 2."
    return
}

# ---------------------------------------------------------------- full run

if (-not $WifiIp) { throw "-WifiIp is required unless -Probe is given" }
$wifiAdapter = Resolve-Adapter -Ip $WifiIp -What "Wi-Fi"
if ($wifiAdapter -eq $tetherAdapter) {
    throw "Both addresses are on adapter '$wifiAdapter'. Two separate interfaces are required."
}

# A ranged GET, not HEAD: these speed-test servers close the connection on HEAD,
# and a ranged GET tests the exact capability the whole project depends on.
Write-Output "Preflight: asking the server for bytes 0-1023 ..."
$probeOut = & curl.exe -s -o NUL -r 0-1023 --max-time 25 -w "%{http_code} %{size_download}" $Url
$parts = ([string]$probeOut).Trim().Split(" ")
if ($parts[0] -ne "206" -or $parts[1] -ne "1024") {
    throw "Range probe returned '$probeOut', expected '206 1024'. Without range support every run collapses to a single stream and all numbers are meaningless. Pick another URL."
}
Write-Output "  HTTP 206, 1024 bytes. Range requests supported."

$free = [math]::Round((Get-PSDrive -Name ($work.Substring(0, 1))).Free / 1GB, 1)
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

$configs = @(
    @{ Name = $script:CfgWifi;     Args = @("--interface=$WifiIp") },
    @{ Name = $script:CfgTether;   Args = @("--interface=$TetherIp") },
    @{ Name = $script:CfgCombined; Args = @("--multiple-interface=$WifiIp,$TetherIp") }
)

$results = New-Object System.Collections.ArrayList

for ($r = 1; $r -le $Repeats; $r++) {
    foreach ($c in $configs) {
        Write-Output "run $($c.Name) repeat $r ..."
        $res = Invoke-AriaRun -Config $c.Name -BindArgs $c.Args -Repeat $r -Duration $Seconds `
                              -WifiAdapterName $wifiAdapter -TetherAdapterName $tetherAdapter
        [void]$results.Add($res)
        Write-Output ("  wifi {0,7} Mbps | tether {1,6} Mbps | total {2,7} Mbps | aria2 says {3,7} | CN {4} | 206s {5} | 429s {6}" -f `
            $res.WifiMbps, $res.TetherMbps, $res.TotalMbps, $res.AriaMbps, $res.MaxCn, $res.Ok206, $res.Throttled)
    }
}

Write-Output ""
Write-Output "=========================== all runs ==========================="
$results | Format-Table Config, Repeat, Secs, WifiMbps, TetherMbps, TotalMbps, AriaMbps, WifiMiB, TetherMiB, TetherPct, MaxCn, Ok206, Throttled -AutoSize |
    Out-String -Width 220 | Write-Output

$mobileMiB = ($results | Measure-Object -Property TetherMiB -Sum).Sum
$wifiTotalMiB = ($results | Measure-Object -Property WifiMiB -Sum).Sum
Write-Output ("Data consumed: {0} MiB over the tether (metered), {1} MiB over Wi-Fi." -f `
    [math]::Round($mobileMiB, 0), [math]::Round($wifiTotalMiB, 0))
Write-Output ""

$verdict = Get-Phase0Verdict -Results $results
Write-Phase0Verdict -Verdict $verdict

$transcript = Join-Path $work "phase0-results.csv"
$results | Export-Csv -LiteralPath $transcript -NoTypeInformation
Write-Output ""
Write-Output "Per-run rows   $transcript"
Write-Output "aria2 logs     $work\*.stdout.txt"

if (-not $verdict.Pass) { exit 1 }
