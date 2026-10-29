<#
    BARRAGE phase 3 (BRG-50): acceptance on this machine. Does the engine's
    own downloader do what aria2 did in phase 0: take one file through the
    Wi-Fi and a phone's tether at once, each keeping most of its own speed?

    This script performs the runs through the engine's API, as the page adds
    a download, and leaves the decision to phase 0's pure function,
    Get-Phase0Verdict in phase0-verdict.ps1, unchanged: every combined run
    must keep at least half of each cable's solo rate from its own repeat.

    Three configurations, interleaved A, B, C in each repeat, so drift over
    the session hits each alike:
      A  Wi-Fi only    "cables": [<Wi-Fi>]
      B  tether only   "cables": [<tether>]
      C  both          "cables": [<Wi-Fi>, <tether>]
    Each run fetches the whole file. Rates come from the adapters' byte
    counters (Get-NetAdapterStatistics), not from the engine, so the engine
    does not mark its own work; its own count per cable is shown beside
    them. Every finished file's SHA-256 must match the first run's, and the
    reference copy's when there is one.

    Two rows are watched together rather than measured (-Row):
      Yank     both cables; the tether's cable pulled at about 30%: the
               download carries on through the Wi-Fi and finishes right.
      Pause    Stop on the download's card, then Resume, as often as wanted:
               each stop saves what it had written, and each resume carries
               on from exactly that. Both cables, or with -WifiOnly the Wi-Fi
               alone, which takes long enough to stop by hand at half and
               spends no mobile data.

    THE REFERENCE. A file is checked against the SHA-256 of a copy of the
    same URL, length and version: with -Reference, one curl fetches in the
    preflight through the Wi-Fi alone (the whole file on the Wi-Fi, none on
    the phone), so a fault the engine made in every copy alike still shows;
    else the first file of the last measured runs, if they were of this
    version. A file the server names no strong ETag for has no reference.

    USAGE
      # The preflight, and what each session would cost:
      .\tools\phase3-acceptance.ps1 -Url <file>
      # A session, once its cost is agreed:
      .\tools\phase3-acceptance.ps1 -Url <file> -BudgetMB <agreed> -Run
      .\tools\phase3-acceptance.ps1 -Url <file> -BudgetMB <agreed> -Row Yank
      # Either, checked against a curl copy fetched over the Wi-Fi first:
      .\tools\phase3-acceptance.ps1 -Url <file> -BudgetMB <agreed> -Row Pause -Reference
      # Before any of it, with no phone: one run through the Wi-Fi alone,
      # every step of a session but the tether's, refused if any other
      # adapter has a way out.
      .\tools\phase3-acceptance.ps1 -Url <file> -Rehearse

    MOBILE DATA. A tether-only run fetches the whole file through the phone,
    a combined run part of it, and each start of the engine tests each
    cable's speed once, with up to 64 MiB. The preflight starts no engine:
    it asks Windows for each cable's address, then curl, through each, for
    the file's first KB and the cable's public address, a few KB in all.
    The phone's counter counts both ways, so each cost shown is the payload
    and a tenth more for what travels beside it. A session refuses to start
    past -BudgetMB, and stops the engine if the tether's counter passes the
    budget anyway. A download still not over after -Minutes ends the session.

    The engine runs in a window of its own, its downloads in a folder of
    their own, and a session opens its page, http://127.0.0.1:<port>, in the
    browser: the Downloads tab shows each cable's part as it runs. Every
    address this script prints is masked.
#>

param(
    [Parameter(Mandatory = $true)][string]$Url,
    [string]$Wifi = "Wi-Fi",
    [string]$Tether = "",
    [int]$Repeats = 2,
    [double]$BudgetMB = 0,
    [switch]$Run,
    [ValidateSet("", "Yank", "Pause")][string]$Row = "",
    # -Row Pause through the Wi-Fi alone: long enough to stop by hand at
    # half, and no mobile data at all.
    [switch]$WifiOnly,
    [switch]$Rehearse,
    # A reference copy by curl over the Wi-Fi in the preflight.
    [switch]$Reference,
    # The longest one download may take before the session gives up.
    [int]$Minutes = 30,
    [int]$Port = 7700,
    [string]$Engine = "",
    [string]$Work = ""
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "phase0-verdict.ps1")

if (@(@($Run.IsPresent, [bool]$Row, $Rehearse.IsPresent) | Where-Object { $_ }).Count -gt 1) {
    throw "-Run, -Row and -Rehearse are separate sessions: give one"
}
if ($WifiOnly -and $Row -ne "Pause") { throw "-WifiOnly goes with -Row Pause" }
if (-not $Engine) { $Engine = Join-Path $PSScriptRoot "..\engine\target\release\barrage.exe" }
if (($Run -or $Row -or $Rehearse) -and -not (Test-Path $Engine)) {
    throw "no engine at $Engine. Build it first, in engine: cargo build --release --bin barrage"
}
if (-not $Work) { $Work = Join-Path $env:TEMP "brg-phase3" }
# A name only this script uses: the folder is emptied at each session, so
# it must never be one that holds anything else, whatever -Work is.
$downloads = Join-Path $Work "brg-phase3-downloads"
$api = "http://127.0.0.1:$Port/api"
$results = Join-Path $Work "phase3-results.csv"
# The SHA-256 files are checked against, with the URL, length and ETag it
# is of.
$referenceFile = Join-Path $Work "phase3-reference.json"
# The file as this session's preflight found it.
$script:FileLength = $null
$script:FileEtag = $null
# Every status a session polls, one JSON line each, with the time and run.
$timeline = Join-Path $Work ("timeline-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".jsonl")

# What one start of the engine can spend on the tether's speed test (64 MiB).
$SpeedTestMB = 64 * 1MB / 1e6

# ---------------------------------------------------------------- helpers

# Any IPv4 or IPv6 address hidden, so nothing printed here names a network.
# An IPv6 address here has three colons at least, so a time such as
# 23:38:01 is left as it is.
function Hide([string]$Text) {
    $Text -replace '\b\d{1,3}(\.\d{1,3}){3}\b', 'x.x.x.x' -replace '\b[0-9a-fA-F]{0,4}(:[0-9a-fA-F]{0,4}){3,7}\b', 'x:x::x'
}

function MB([double]$Bytes) { [math]::Round($Bytes / 1e6, 1) }

# Bytes through an adapter both ways: what a phone's data counter counts. An
# adapter gone, its cable pulled, carries nothing more: its last count stands.
$script:LastBytes = @{}
function Get-AdapterBytes([string]$Name) {
    $s = Get-NetAdapterStatistics -Name $Name -ErrorAction SilentlyContinue
    if ($s) { $script:LastBytes[$Name] = [double]$s.ReceivedBytes + [double]$s.SentBytes }
    return [double]$script:LastBytes[$Name]
}

# A cable's IPv4 address as Windows holds it: Preferred, not link-local.
function Get-Address([string]$Name) {
    $a = Get-NetIPAddress -AddressFamily IPv4 -InterfaceAlias $Name -ErrorAction SilentlyContinue |
        Where-Object { $_.AddressState -eq "Preferred" -and $_.IPAddress -notlike "169.254.*" } |
        Select-Object -First 1
    if (-not $a) { throw "$Name has no usable IPv4 address: is it connected?" }
    return $a.IPAddress
}

# The tether: named, or the one adapter besides the Wi-Fi with a default route.
function Find-Tether {
    if ($Tether) {
        if ($Tether -eq $Wifi) { throw "the tether cannot be the Wi-Fi" }
        return $Tether
    }
    # In @(...): a function's one-item array reaches its caller as the item.
    $names = @(Get-OtherWaysOut)
    if ($names.Count -eq 0) { throw "no adapter besides $Wifi has a way out: is the phone tethered?" }
    if ($names.Count -gt 1) {
        throw "adapters with a way out besides $($Wifi): $($names -join ', '). Name the tether with -Tether."
    }
    return $names[0]
}

# The public address the internet sees from a cable, through Cloudflare's
# trace page: a few hundred bytes.
function Get-PublicAddress([string]$Name, [string]$Address) {
    $trace = & curl.exe -s --interface $Address --max-time 15 "https://www.cloudflare.com/cdn-cgi/trace"
    $line = @($trace) | Where-Object { $_ -like "ip=*" } | Select-Object -First 1
    if (-not $line) { throw "no public address came back through $Name" }
    return $line.Substring(3)
}

# One ranged 1 KB request from the cable's own address, as the engine's
# probe asks: 206 and 1024 bytes, and the file's length.
function Test-Range([string]$Name, [string]$Address) {
    $headers = Join-Path $Work ("range-" + ($Name -replace '\W', '_') + ".txt")
    $out = & curl.exe -s -L -o NUL -D $headers --interface $Address -r 0-1023 --max-time 25 `
        -w "%{http_code} %{size_download}" $Url
    $parts = ([string]$out).Trim().Split(" ")
    $range = Select-String -Path $headers -Pattern '^Content-Range:\s*bytes\s+0-1023/(\d+)' |
        Select-Object -Last 1
    $length = $null
    if ($range) { $length = [int64]$range.Matches[0].Groups[1].Value }
    # What the engine would keep as the file's version: a strong ETag, else
    # Last-Modified. With neither, a stopped download starts over.
    $etag = Select-String -Path $headers -Pattern '^ETag:\s*(\S.*?)\s*$' | Select-Object -Last 1
    $date = Select-String -Path $headers -Pattern '^Last-Modified:\s*\S' | Select-Object -Last 1
    $version = $null
    $strong = $null
    if ($etag -and $etag.Matches[0].Groups[1].Value -match '^".*"$') {
        $version = "a strong ETag"
        $strong = $etag.Matches[0].Groups[1].Value
    }
    elseif ($date) { $version = "its Last-Modified date" }
    [pscustomobject]@{
        Ok      = ($parts[0] -eq "206" -and $parts[1] -eq "1024")
        Answer  = "$($parts[0]) $($parts[1])"
        Length  = $length
        Version = $version
        Etag    = $strong
    }
}

# The file as this session's preflight found it, for the reference to be
# of: its length, and its strong ETag if it has one.
function Set-FileFound($Range) {
    $script:FileLength = $Range.Length
    $script:FileEtag = $Range.Etag
}

# The reference SHA-256 for this session's file: of the same URL, length and
# strong ETag, or none. A file with no strong ETag has none: nothing tells
# its versions apart.
function Get-Reference {
    if (-not $script:FileEtag -or -not (Test-Path $referenceFile)) { return $null }
    $ref = Get-Content -LiteralPath $referenceFile | Out-String | ConvertFrom-Json
    if ($ref.url -ne $Url -or [int64]$ref.length -ne $script:FileLength -or $ref.etag -ne $script:FileEtag) {
        return $null
    }
    return $ref
}

function Save-Reference([string]$Sha, [string]$From) {
    if (-not $script:FileEtag) { return }
    [pscustomobject]@{
        url    = $Url
        length = $script:FileLength
        etag   = $script:FileEtag
        sha    = $Sha
        from   = $From
    } | ConvertTo-Json -Compress | Set-Content -LiteralPath $referenceFile -Encoding UTF8
}

# A copy of the file the engine did not make: curl, through the Wi-Fi alone,
# so it spends no mobile data. Its SHA-256 becomes the reference. Refused if
# the server's ETag is not the one the preflight saw.
function New-CurlReference([string]$Address) {
    if (-not $script:FileEtag) { throw "-Reference needs a file whose server names a strong ETag" }
    $copy = Join-Path $Work "reference.bin"
    $headers = Join-Path $Work "reference-headers.txt"
    "  fetching the reference copy with curl through $($Wifi): $(MB $script:FileLength) MB on the Wi-Fi, none on the phone"
    & curl.exe -s -L --interface $Address --max-time 3600 -D $headers -o $copy $Url
    if ($LASTEXITCODE -ne 0) { throw "curl could not fetch the reference copy (exit $LASTEXITCODE)" }
    $tag = Select-String -Path $headers -Pattern '^ETag:\s*(\S.*?)\s*$' | Select-Object -Last 1
    $got = $null
    if ($tag) { $got = $tag.Matches[0].Groups[1].Value }
    $size = (Get-Item -LiteralPath $copy).Length
    if ($got -ne $script:FileEtag -or $size -ne $script:FileLength) {
        Remove-Item -LiteralPath $copy -Force
        throw "the file on the server changed while its reference copy came down"
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $copy).Hash.ToLower()
    Remove-Item -LiteralPath $copy -Force
    Save-Reference $hash "the curl copy"
    "  the reference copy's SHA-256: $($hash.Substring(0, 12))..."
}

# How long one download may take, and the session's end if it takes longer.
function Get-Deadline { (Get-Date).AddMinutes($Minutes) }

function Assert-Deadline($Deadline, $Download) {
    if ((Get-Date) -gt $Deadline) {
        Stop-Engine $script:Proc
        throw "after $Minutes min the download is still $($Download.state); note: $(Hide "$($Download.note)")"
    }
}

# The engine, in a window of its own, where its own words show. With -Open,
# its page opens in the browser on the Downloads tab.
function Start-Engine([switch]$Open) {
    if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
        throw "port $Port is taken: stop the barrage already running, or pass -Port"
    }
    New-Item -ItemType Directory -Force $downloads | Out-Null
    $proc = Start-Process -FilePath $Engine -PassThru `
        -ArgumentList @("--no-open", "--port", "$Port", "--downloads", "`"$downloads`"")
    $deadline = (Get-Date).AddSeconds(15)
    while ($true) {
        try { Invoke-RestMethod "$api/links" | Out-Null; break } catch { }
        if ($proc.HasExited) { throw "the engine stopped as it started (exit $($proc.ExitCode))" }
        if ((Get-Date) -gt $deadline) { throw "the engine did not answer within 15 s" }
        Start-Sleep -Milliseconds 300
    }
    if ($Open) { Start-Process "http://127.0.0.1:$Port/#downloads" }
    return $proc
}

function Stop-Engine($Proc) {
    if ($null -ne $Proc -and -not $Proc.HasExited) {
        Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue
    }
}

# Whether the engine counts the cable usable now, and has measured it.
function Get-Cable([object]$Table, [string]$Name) {
    $links = @($Table.links | Where-Object { $_.name -eq $Name -and $_.state -in @("up", "degraded") })
    [pscustomobject]@{
        Name     = $Name
        Usable   = ($links.Count -gt 0)
        Measured = (@($links | Where-Object { $null -ne $_.down_bps }).Count -gt 0)
    }
}

# Every cable usable to the engine, each speed test done so none lands in a
# run, and the engine too seeing the two on separate connections.
function Wait-Cables {
    $names = @($Wifi)
    if ($script:TetherName) { $names += $script:TetherName }
    $deadline = (Get-Date).AddSeconds(120)
    while ($true) {
        $table = Invoke-RestMethod "$api/links"
        $cables = @($names | ForEach-Object { Get-Cable $table $_ })
        if (@($cables | Where-Object { -not ($_.Usable -and $_.Measured) }).Count -eq 0) { break }
        if ((Get-Date) -gt $deadline) {
            $states = ($cables | ForEach-Object { "$($_.Name) usable: $($_.Usable), measured: $($_.Measured)" }) -join "; "
            throw "after 120 s the engine has $states"
        }
        Start-Sleep -Seconds 1
    }
    foreach ($group in @($table.connections)) {
        if ($script:TetherName -and (@($group) -contains $Wifi) -and (@($group) -contains $script:TetherName)) {
            throw "the engine sees $Wifi and $($script:TetherName) on one connection"
        }
    }
}

# The adapters besides the Wi-Fi with a way out, either family.
function Get-OtherWaysOut {
    @(Get-NetRoute -DestinationPrefix @("0.0.0.0/0", "::/0") -ErrorAction SilentlyContinue |
        ForEach-Object { $_.InterfaceAlias } | Sort-Object -Unique | Where-Object { $_ -ne $Wifi })
}

# Stop everything if the tether has carried more than the budget since the
# session began.
function Assert-Budget {
    if (-not $script:TetherName) { return }
    $used = (Get-AdapterBytes $script:TetherName) - $script:TetherStart
    if ($used / 1e6 -gt $BudgetMB) {
        Stop-Engine $script:Proc
        throw "stopped: the tether carried $(MB $used) MB this session, past the $BudgetMB MB agreed"
    }
}

# The download's status now, written to the timeline as `what` with the time.
function Get-Download([int]$Id, [string]$What = "") {
    $d = @((Invoke-RestMethod "$api/downloads").list | Where-Object { $_.id -eq $Id })[0]
    $line = [pscustomobject]@{
        at       = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
        what     = $What
        download = $d
    }
    Add-Content -LiteralPath $timeline -Encoding UTF8 -Value (Hide ($line | ConvertTo-Json -Compress -Depth 6))
    return $d
}

function Add-Download([string[]]$Cables) {
    $body = @{ url = $Url; cables = @($Cables) } | ConvertTo-Json -Compress
    $added = Invoke-RestMethod -Method Post -Uri "$api/downloads" -ContentType "application/json" -Body $body
    return [int]$added.id
}

# Each cable's notes, each said once, masked.
function Add-Notes($List, $Download) {
    foreach ($c in @($Download.cables)) {
        foreach ($n in @($c.notes)) {
            $line = Hide "$($c.name): $n"
            if (-not $List.Contains($line)) { [void]$List.Add($line) }
        }
    }
}

# The finished file's SHA-256, the file then removed so the next run gets
# the same name.
function Get-FinishedHash($Download) {
    $file = Join-Path $downloads $Download.name
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLower()
    Remove-Item -LiteralPath $file -Force
    return $hash
}

# ---------------------------------------------------------------- one run

function Invoke-Run([string]$Config, [int]$Repeat, [string[]]$Cables) {
    $wifiBefore = Get-AdapterBytes $Wifi
    $tetherBefore = 0.0
    if ($script:TetherName) { $tetherBefore = Get-AdapterBytes $script:TetherName }
    $id = Add-Download $Cables
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $deadline = Get-Deadline
    $peak = 0
    $notes = New-Object System.Collections.ArrayList
    while ($true) {
        Start-Sleep -Milliseconds 500
        $d = Get-Download $id "$Config r$Repeat"
        $open = [int](@($d.cables) | Measure-Object -Property connections -Sum).Sum
        if ($open -gt $peak) { $peak = $open }
        Add-Notes $notes $d
        Assert-Budget
        if ($d.state -in @("done", "failed")) { break }
        # Stopped from the open page, a measured run measures nothing.
        if ($d.state -eq "stopped") { throw "$Config r$($Repeat): stopped from the page, so it was not measured" }
        Assert-Deadline $deadline $d
    }
    $watch.Stop()
    $secs = $watch.Elapsed.TotalSeconds
    $wifiMbps = ((Get-AdapterBytes $Wifi) - $wifiBefore) * 8 / 1e6 / $secs
    $tetherMbps = 0.0
    if ($script:TetherName) {
        $tetherMbps = ((Get-AdapterBytes $script:TetherName) - $tetherBefore) * 8 / 1e6 / $secs
    }
    $total = $wifiMbps + $tetherMbps
    $share = 0.0
    if ($total -gt 0) { $share = $tetherMbps / $total * 100 }
    $hash = $null
    if ($d.state -eq "done") { $hash = Get-FinishedHash $d }
    $engineWifi = [double](@($d.cables | Where-Object { $_.name -eq $Wifi }) | Measure-Object -Property bytes -Sum).Sum
    $engineTether = [double](@($d.cables | Where-Object { $_.name -eq $script:TetherName }) | Measure-Object -Property bytes -Sum).Sum
    # Ok206 stands for "came in pieces": the engine counts no answers, but
    # says when a server sends the file only whole.
    $pieces = 1
    if ("$($d.note)" -like "*only whole*") { $pieces = 0 }
    [pscustomobject]@{
        Config       = $Config
        Repeat       = $Repeat
        Secs         = [math]::Round($secs, 1)
        WifiMbps     = [math]::Round($wifiMbps, 1)
        TetherMbps   = [math]::Round($tetherMbps, 1)
        TotalMbps    = [math]::Round($total, 1)
        TetherPct    = [math]::Round($share, 0)
        MaxCn        = $peak
        Ok206        = $pieces
        EngineWifiMB = MB $engineWifi
        EngineTethMB = MB $engineTether
        State        = $d.state
        Sha          = $hash
        Note         = Hide "$($d.note)"
        Notes        = ($notes -join " | ")
    }
}

# ---------------------------------------------------------------- the rows

function Show-Hash([string]$Hash) {
    $ref = Get-Reference
    if ($ref) { "  SHA-256 $($Hash.Substring(0, 12))..., the same as $($ref.from): $($Hash -eq $ref.sha)" }
    else { "  SHA-256 $($Hash.Substring(0, 12))... (no reference for this version of the file to compare with)" }
}

function Invoke-Yank {
    $id = Add-Download $script:Cables
    "  added. Watch the Downloads tab; at about 30%, pull the tether's cable."
    $pulled = $false
    $notes = New-Object System.Collections.ArrayList
    $deadline = Get-Deadline
    while ($true) {
        Start-Sleep -Milliseconds 250
        $d = Get-Download $id "yank"
        Assert-Deadline $deadline $d
        Add-Notes $notes $d
        if (-not $pulled) {
            Assert-Budget
            if (-not (Get-Cable (Invoke-RestMethod "$api/links") $script:TetherName).Usable) {
                $pulled = $true
                $pct = 0
                if ($d.length) { $pct = [math]::Round($d.done / $d.length * 100, 0) }
                "  the engine saw the tether go at $pct% ($(MB $d.done) MB)"
            }
        }
        if ($d.state -in @("done", "failed")) { break }
    }
    "  $($d.state): $(MB $d.done) of $(MB $d.length) MB; note: $(Hide "$($d.note)")"
    foreach ($c in @($d.cables)) { "  $($c.name) carried $(MB $c.bytes) MB" }
    foreach ($n in $notes) { "  note seen: $n" }
    if (-not $pulled) { "  the tether never went: its cable was not pulled before the end" }
    if ($d.state -eq "done") { Show-Hash (Get-FinishedHash $d) }
}

# Stop on the card, then Resume, as many times as the user likes: each stop
# saved every byte written, and each resume carried on from exactly what the
# stop before it saved.
function Invoke-Pause {
    $id = Add-Download $script:Cables
    "  added. Watch the Downloads tab; press Stop on its card once it shows a percentage, then Resume."
    $deadline = Get-Deadline
    $rounds = 0
    $exact = $true
    # What the last stop saved, for the round after it to be checked against.
    $had = $null
    while ($true) {
        while ($true) {
            Start-Sleep -Milliseconds 250
            $d = Get-Download $id "pause: running"
            Assert-Budget
            if ($d.state -in @("stopped", "done", "failed")) { break }
            Assert-Deadline $deadline $d
        }
        if ($null -ne $had) {
            # A resumed download's cables count only what they carried since,
            # so the rest of what is done was already here.
            $fetched = [double](@($d.cables) | Measure-Object -Property bytes -Sum).Sum
            $already = $d.done - $fetched
            "  round ${rounds}: $(MB $already) MB were already here on resuming, $(MB $had) MB saved: the same: $($already -eq $had)"
            if ($already -ne $had) { $exact = $false }
        }
        if ($d.state -ne "stopped") { break }
        $rounds++
        $pct = 0
        if ($d.length) { $pct = [math]::Round($d.done / $d.length * 100, 0) }
        "  stop ${rounds}: at $pct% ($(MB $d.done) MB); note: $(Hide "$($d.note)")"
        $saved = Get-Content -LiteralPath (Join-Path $downloads "$($d.name).part.json") | Out-String | ConvertFrom-Json
        $had = [double](@($saved.done) | ForEach-Object { $_[1] - $_[0] } | Measure-Object -Sum).Sum
        "  saved beside the .part: $(MB $had) MB in $(@($saved.done).Count) ranges; every byte written: $($had -eq $d.done)"
        if ($had -ne $d.done) { $exact = $false }

        $resumeBy = (Get-Date).AddMinutes(10)
        while ($d.state -eq "stopped") {
            if ((Get-Date) -gt $resumeBy) { throw "not resumed within 10 minutes" }
            Start-Sleep -Milliseconds 250
            $d = Get-Download $id "pause: stopped"
        }
        "  resumed"
    }
    if ($rounds -eq 0) {
        "  $($d.state) before Stop: nothing to resume"
        return
    }
    "  $($d.state) after $rounds stop(s); note: $(Hide "$($d.note)")"
    "  every stop saved what it had written, and every resume carried on from exactly that: $exact"
    if ($d.state -eq "done") { Show-Hash (Get-FinishedHash $d) }
}

# ---------------------------------------------------------------- session

New-Item -ItemType Directory -Force $Work | Out-Null
$script:Proc = $null
$script:TetherName = $null
if ($Run -or $Row -or $Rehearse) {
    # Nothing an earlier session left, which a run would resume from.
    Remove-Item -Recurse -Force -LiteralPath $downloads -ErrorAction SilentlyContinue
}
try {
    if ($Rehearse) {
        "== Rehearsal: the Wi-Fi alone, no phone"
        $others = @(Get-OtherWaysOut)
        if ($others.Count -gt 0) {
            throw "a rehearsal is the Wi-Fi alone, and $($others -join ', ') has a way out too: unplug it first"
        }
        $range = Test-Range $Wifi (Get-Address $Wifi)
        if (-not $range.Ok) { throw "$($Wifi): a ranged 1 KB request answered '$($range.Answer)', not '206 1024'" }
        "  $($Wifi): a ranged 1 KB request answered 206 with 1024 bytes; the file is $(MB $range.Length) MB"
        if ($range.Version) { "  the server names its version by $($range.Version)" }
        else { "  the server names no version: a stopped download would start over" }
        Set-FileFound $range
        if ($Reference) { New-CurlReference (Get-Address $Wifi) }
        $script:Proc = Start-Engine
        Wait-Cables
        "  the engine is up at http://127.0.0.1:$Port and has measured the Wi-Fi"
        $res = Invoke-Run -Config $script:CfgWifi -Repeat 1 -Cables @($Wifi)
        $res | Format-List Config, Secs, WifiMbps, TetherMbps, MaxCn, Ok206, EngineWifiMB, EngineTethMB, State, Sha, Note, Notes |
            Out-String -Width 200
        if ($res.Sha) { Show-Hash $res.Sha }
        return
    }

    if ($WifiOnly) {
        "== Row: Pause, the Wi-Fi alone, no phone"
        $others = @(Get-OtherWaysOut)
        if ($others.Count -gt 0) {
            throw "this row is the Wi-Fi alone, and $($others -join ', ') has a way out too: unplug it first"
        }
        $range = Test-Range $Wifi (Get-Address $Wifi)
        if (-not $range.Ok) { throw "$($Wifi): a ranged 1 KB request answered '$($range.Answer)', not '206 1024'" }
        if (-not $range.Version) { throw "-Row Pause needs a file whose server names its version" }
        "  $($Wifi): a ranged 1 KB request answered 206 with 1024 bytes; the file is $(MB $range.Length) MB, its version named by $($range.Version)"
        Set-FileFound $range
        if ($Reference) { New-CurlReference (Get-Address $Wifi) }
        $script:Cables = @($Wifi)
        $script:Proc = Start-Engine -Open
        Wait-Cables
        "  the engine is up at http://127.0.0.1:$Port and has measured the Wi-Fi"
        Invoke-Pause
        return
    }

    "== Preflight: no engine yet, a few KB through each cable"
    $script:TetherName = Find-Tether
    # Counted from here, so everything this script sends through the phone is.
    $script:TetherStart = Get-AdapterBytes $script:TetherName
    "  the tether is $($script:TetherName)"
    $addresses = @{}
    foreach ($name in @($Wifi, $script:TetherName)) { $addresses[$name] = Get-Address $name }
    $public = @{}
    foreach ($name in @($Wifi, $script:TetherName)) {
        $public[$name] = Get-PublicAddress $name $addresses[$name]
    }
    if ($public[$Wifi] -eq $public[$script:TetherName]) {
        throw ("$Wifi and $($script:TetherName) reach the internet from one public address: the phone " +
            "is on the laptop's network, not on mobile data. Turn the phone's Wi-Fi off.")
    }
    "  the two reach the internet from different public addresses"
    $length = $null
    foreach ($name in @($Wifi, $script:TetherName)) {
        $range = Test-Range $name $addresses[$name]
        if (-not $range.Ok) {
            throw "$($name): a ranged 1 KB request answered '$($range.Answer)', not '206 1024'"
        }
        "  $($name): a ranged 1 KB request answered 206 with 1024 bytes"
        $length = $range.Length
        $version = $range.Version
        Set-FileFound $range
    }
    if (-not $length) { throw "the server named no length in its Content-Range" }
    $fileMB = $length / 1e6
    "  the file: $([math]::Round($fileMB, 1)) MB"
    if ($version) {
        "  the server names its version by $version, so a stopped download can resume"
    } else {
        "  the server names no version: a stopped download would start over"
        if ($Row -eq "Pause") { throw "-Row Pause needs a file whose server names its version" }
    }

    # The most each session can cost the phone: a tether-only run is the
    # whole file, a combined run at most the whole file again, and each
    # start of the engine a speed test. The phone counts both ways, TLS and
    # acknowledgements too, so a tenth more: the measured runs of 2026-10-22
    # counted 2635 MB against about 2450 MB of files and speed test.
    $margin = 1.1
    $costs = @{
        Run     = [math]::Ceiling(($SpeedTestMB + $Repeats * 2 * $fileMB) * $margin)
        Yank    = [math]::Ceiling(($SpeedTestMB + $fileMB) * $margin)
        Pause   = [math]::Ceiling(($SpeedTestMB + $fileMB) * $margin)
    }
    if (-not $Run -and -not $Row) {
        ""
        "== What each session can cost in mobile data, at most, a tenth over the payload"
        "  -Run ($Repeats repeats of A, B, C): $($costs.Run) MB; about $([math]::Ceiling(($SpeedTestMB + $Repeats * 1.5 * $fileMB) * $margin)) MB if the tether carries half of each combined run"
        "  -Row Yank: $($costs.Yank) MB"
        "  -Row Pause: $($costs.Pause) MB; with -WifiOnly none"
        "  -Reference adds $([math]::Ceiling($fileMB)) MB on the Wi-Fi, none on the phone"
        "  Agree a budget, then run again with -BudgetMB and -Run or -Row."
        return
    }
    $session = "Run"
    if ($Row) { $session = $Row }
    if ($BudgetMB -lt $costs[$session]) {
        throw "this session can cost up to $($costs[$session]) MB of mobile data, more than -BudgetMB $BudgetMB"
    }
    if ($Reference) { New-CurlReference $addresses[$Wifi] }

    ""
    "== The engine"
    $script:Proc = Start-Engine -Open
    Wait-Cables
    "  up at http://127.0.0.1:$Port, both cables usable and measured; the tether has carried $(MB ((Get-AdapterBytes $script:TetherName) - $script:TetherStart)) MB so far"

    if ($Row) {
        ""
        "== Row: $Row"
        $script:Cables = @($Wifi, $script:TetherName)
        if ($Row -eq "Yank") { Invoke-Yank } else { Invoke-Pause }
    } else {
        ""
        "== The runs: $Repeats repeats of A (Wi-Fi), B (tether), C (both)"
        $configs = @(
            @{ Name = $script:CfgWifi; Cables = @($Wifi) },
            @{ Name = $script:CfgTether; Cables = @($script:TetherName) },
            @{ Name = $script:CfgCombined; Cables = @($Wifi, $script:TetherName) }
        )
        $rows = New-Object System.Collections.ArrayList
        for ($r = 1; $r -le $Repeats; $r++) {
            foreach ($c in $configs) {
                $res = Invoke-Run -Config $c.Name -Repeat $r -Cables $c.Cables
                [void]$rows.Add($res)
                "  {0} r{1}: {2} in {3} s | wifi {4} Mbps | tether {5} Mbps | engine {6} / {7} MB | peak {8} connections" -f `
                    $res.Config, $res.Repeat, $res.State, $res.Secs, $res.WifiMbps, $res.TetherMbps,
                    $res.EngineWifiMB, $res.EngineTethMB, $res.MaxCn
                if ($res.Notes) { "    notes: $($res.Notes)" }
                if ($res.State -ne "done") { throw "$($res.Config) r$($res.Repeat) failed: $($res.Note)" }
            }
        }
        $rows | Export-Csv -LiteralPath $results -NoTypeInformation

        ""
        "== All runs"
        $rows | Format-Table Config, Repeat, Secs, WifiMbps, TetherMbps, TotalMbps, TetherPct, MaxCn, EngineWifiMB, EngineTethMB -AutoSize |
            Out-String -Width 200
        $first = $rows[0].Sha
        $same = @($rows | Where-Object { $_.Sha -ne $first }).Count -eq 0
        "  every file's SHA-256 matches the first run's ($($first.Substring(0, 12))...): $same"
        $ref = Get-Reference
        if ($ref -and $ref.from -eq "the curl copy") {
            $same = $same -and ($first -eq $ref.sha)
            "  and the curl copy's: $($first -eq $ref.sha)"
        } elseif ($same) {
            # For a row later in the session, or another session, of this
            # same version of the file.
            Save-Reference $first "the measured runs' first file"
        }
        $used = ((Get-AdapterBytes $script:TetherName) - $script:TetherStart) / 1e6
        "  mobile data this session, both ways: $([math]::Round($used, 0)) MB"

        $verdict = Get-Phase0Verdict -Results $rows
        ""
        "== Verdict, by phase 0's rules"
        foreach ($p in $verdict.Pairs) {
            "  repeat $($p.Repeat): Wi-Fi kept $(Format-Pct $p.WifiRet) of its solo rate, the tether $(Format-Pct $p.TetherRet)"
        }
        foreach ($chk in $verdict.Checks) {
            $kind = "advisory"
            if ($chk.Gate) { $kind = "gate    " }
            "  {0,-4}  {1}  {2}" -f $chk.State, $kind, $chk.Text
        }
        ""
        if ($verdict.Pass -and $same) {
            "PASS. The engine took the file through both cables at once, each keeping"
            "at least half its own speed, and every copy was the same file."
        } else {
            "FAIL. A SKIP is not a pass: it means a run could not answer the question."
        }
        "Per-run rows: $results"
    }
}
finally {
    Stop-Engine $script:Proc
}
