<#
    BARRAGE phase 4 (BRG-63): acceptance on this machine. Does the scheduler
    share a download's connections by speed, within what a server allows,
    treat two cables on one connection as one, survive a cable flapping, and
    show what each cable is costing?

    The rows, in the order of PLAN.md's phase 4 acceptance list (-Rows; the
    data used is checked in every row that downloads):
      Limited  Hetzner's request-limited 100 MB file, both cables: it
               finishes, both carry, no cable rests, tens of requests per
               cable (counted from the engine's log), not hundreds.
      Weights  A download CDN file, both cables: the faster cable holds more
               connections on average, and both carry.
      Metered  Metered connection turned on for the tether in Windows'
               Settings, then off: the engine's mark follows Windows within
               seconds each way (Windows' own cost read beside it).
      Shared   The phone's Wi-Fi joined to the laptop's network, tethering
               on: the engine joins the two into one connection, and a
               download leaves the tether sitting out with the engine's note.
               Then the phone's Wi-Fi off again.
      Flap     A long file, both cables; tethering turned off once the tether
               carries, and on again within 20 s: it comes back degraded,
               holding one connection, then up after the cooldown (30 s) with
               its share at once. The download is stopped as soon as that
               shows, so the long file costs only what the row needs.
    Data used: for each cable that carried, the engine's count for the run
    against what Windows' adapter received: at most a tenth less, never more.

    ONE ENGINE, A YES PER ROW. The engine starts once, with --log, so the
    tether's speed test is paid once. Before each row the script waits for
    the file go-<Row> in -Work, which is made when the user says yes to that
    row. Within a row it waits on what it can see: the engine's table, its
    downloads, Windows' adapters; each cue says what to do next.

    USAGE
      # The preflight, and what the session would cost:
      .\tools\phase4-acceptance.ps1
      # The session, once its cost is agreed:
      .\tools\phase4-acceptance.ps1 -Rows Limited,Weights,Metered,Shared,Flap -BudgetMB <agreed>
      # Before any of it, with no phone: a download through the Wi-Fi alone,
      # its data used and requests, and Windows' cost read for the Wi-Fi.
      .\tools\phase4-acceptance.ps1 -Rehearse

    MOBILE DATA. The preflight starts no engine and asks only the Wi-Fi for
    the files' first KB; through the phone it fetches only its public
    address, a few hundred bytes. Each estimate is the payload a tenth over.
    The session refuses to start past -BudgetMB and stops the engine if the
    tether's counter passes it anyway. Every address printed is masked.
#>

param(
    [ValidateSet("Limited", "Weights", "Metered", "Shared", "Flap")][string[]]$Rows = @(),
    [string]$LimitedUrl = "https://sin-speed.hetzner.com/100MB.bin",
    [string]$Url = "https://nodejs.org/dist/v20.11.0/node-v20.11.0.tar.gz",
    [string]$LongUrl = "https://desktop.docker.com/win/main/amd64/Docker%20Desktop%20Installer.exe",
    [string]$Wifi = "Wi-Fi",
    [string]$Tether = "",
    [double]$BudgetMB = 0,
    [switch]$Rehearse,
    # The longest one download, or one wait on the user, may take.
    [int]$Minutes = 15,
    [int]$Port = 7700,
    [string]$Engine = "",
    [string]$Work = ""
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "acceptance-lib.ps1")

$Order = @("Limited", "Weights", "Metered", "Shared", "Flap")
$Rows = @($Order | Where-Object { $Rows -contains $_ })
if ($Rehearse -and $Rows.Count -gt 0) { throw "-Rehearse is a session of its own: give it alone" }
if (-not $Engine) { $Engine = Join-Path $PSScriptRoot "..\engine\target\release\barrage.exe" }
if (($Rows.Count -gt 0 -or $Rehearse) -and -not (Test-Path $Engine)) {
    throw "no engine at $Engine. Build it first, in engine: cargo build --release --bin barrage"
}
if (-not $Work) { $Work = Join-Path $env:TEMP "brg-phase4" }
# A name only this script uses: emptied at each session.
$downloads = Join-Path $Work "brg-phase4-downloads"
$api = "http://127.0.0.1:$Port/api"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$timeline = Join-Path $Work "timeline-$stamp.jsonl"
$script:EngineLog = Join-Path $Work "engine-$stamp.log"
$SpeedTestMB = 64 * 1MB / 1e6
$script:Proc = $null
$script:TetherName = $null
$script:TetherStart = 0.0
# Every link id the engine has shown, by cable, so a request in the log is
# put down to its cable even after the link has gone.
$script:LinkNames = @{}
$script:Passed = [ordered]@{}

# ---------------------------------------------------------------- helpers

function Get-Links {
    $table = Invoke-RestMethod "$api/links"
    foreach ($l in @($table.links)) { $script:LinkNames[[string]$l.id] = $l.name }
    return $table
}

# The usable links of a cable now, from a table.
function Get-Usable($Table, [string]$Name) {
    @($Table.links | Where-Object { $_.name -eq $Name -and $_.state -in @("up", "degraded") })
}

# Bytes Windows says an adapter received; the last count stands once it is gone.
$script:LastReceived = @{}
function Get-AdapterReceived([string]$Name) {
    $s = Get-NetAdapterStatistics -Name $Name -ErrorAction SilentlyContinue
    if ($s) { $script:LastReceived[$Name] = [double]$s.ReceivedBytes }
    return [double]$script:LastReceived[$Name]
}

# What the engine says each cable has received, every download together.
function Get-EngineUsed {
    $used = @{}
    foreach ($u in @((Invoke-RestMethod "$api/downloads").used)) { $used[$u.cable] = [double]$u.bytes }
    return $used
}

# Whether Windows says the connection a cable is on now is metered: the cost
# of the profile its adapter is connected to, read as the engine reads it.
function Get-WindowsMetered([string]$Name) {
    $adapter = Get-NetAdapter -Name $Name -ErrorAction SilentlyContinue
    if (-not $adapter) { return $null }
    $guid = $adapter.InterfaceGuid.Trim("{}").ToLower()
    $null = [Windows.Networking.Connectivity.NetworkInformation, Windows.Networking.Connectivity, ContentType = WindowsRuntime]
    foreach ($p in [Windows.Networking.Connectivity.NetworkInformation]::GetConnectionProfiles()) {
        if ($p.GetNetworkConnectivityLevel().ToString() -eq "None") { continue }
        $na = $p.NetworkAdapter
        if ($null -eq $na -or $na.NetworkAdapterId.ToString().ToLower() -ne $guid) { continue }
        $cost = $p.GetConnectionCost()
        return ($cost.NetworkCostType.ToString() -in @("Fixed", "Variable")) -or $cost.Roaming -or $cost.OverDataLimit
    }
    return $false
}

# The engine's log from byte `From` on, and where it now ends. Read shared,
# since the engine holds it open.
function Read-Log([long]$From) {
    if (-not (Test-Path $script:EngineLog)) { return @{ Text = ""; End = 0 } }
    $stream = [System.IO.File]::Open($script:EngineLog, "Open", "Read", "ReadWrite")
    try {
        [void]$stream.Seek($From, "Begin")
        $text = (New-Object System.IO.StreamReader($stream)).ReadToEnd()
        return @{ Text = $text; End = $stream.Length }
    } finally { $stream.Close() }
}

# The requests in a stretch of the log, by cable: how many, and how many the
# server turned away (429 or 503).
function Measure-Requests([string]$Text) {
    $by = @{}
    foreach ($line in ($Text -split "`n")) {
        if ($line -notmatch "asked for a part of the file") { continue }
        if ($line -notmatch "link=(\d+)") { continue }
        $name = $script:LinkNames[$Matches[1]]
        if (-not $name) { $name = "link $($Matches[1])" }
        if (-not $by.ContainsKey($name)) { $by[$name] = [pscustomobject]@{ Asked = 0; Refused = 0 } }
        $by[$name].Asked++
        if ($line -match "status=(429|503)") { $by[$name].Refused++ }
    }
    return $by
}

# Wait until `Check` returns something, or the row's time runs out.
function Wait-For([string]$What, [scriptblock]$Check, [int]$Seconds = $Minutes * 60) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ($true) {
        $got = & $Check
        if ($got) { return $got }
        Assert-Budget
        if ((Get-Date) -gt $deadline) { throw "waited $Seconds s for $What" }
        Start-Sleep -Milliseconds 250
    }
}

# The user's yes for a row: the file go-<Row> in -Work.
function Wait-Go([string]$Row) {
    $go = Join-Path $Work "go-$Row"
    Remove-Item -LiteralPath $go -Force -ErrorAction SilentlyContinue
    ""
    "== Row: $Row, waiting for the yes ($go)"
    Wait-For "the yes for $Row" { Test-Path $go } -Seconds 3600 | Out-Null
    Remove-Item -LiteralPath $go -Force -ErrorAction SilentlyContinue
    "  started $(Get-Date -Format 'HH:mm:ss')"
}

function Stop-Download([int]$Id) {
    try {
        Invoke-RestMethod -Method Post -Uri "$api/downloads/$Id/stop" -ContentType "application/json" -Body "{}" | Out-Null
    } catch { }
}

# Every byte of the downloads folder gone, so the next row starts afresh.
function Clear-Downloads {
    Get-ChildItem -LiteralPath $downloads -Force -ErrorAction SilentlyContinue | Remove-Item -Force -Recurse
}

# One download through `Cables`, polled into the timeline, with each cable's
# connections, wanted and speed at every poll. `Each` sees each poll's
# download and table, and may return "stop" to stop the download there.
function Invoke-Download([string]$From, [string[]]$Cables, [string]$What, [scriptblock]$Each = $null) {
    $received = @{}
    foreach ($c in $Cables) { $received[$c] = Get-AdapterReceived $c }
    $usedBefore = Get-EngineUsed
    $logStart = (Read-Log 0).End
    $id = Add-Download $Cables $From
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $deadline = (Get-Date).AddMinutes($Minutes)
    $samples = New-Object System.Collections.ArrayList
    $notes = New-Object System.Collections.ArrayList
    while ($true) {
        Start-Sleep -Milliseconds 400
        $d = Get-Download $id $What
        $table = Get-Links
        Add-Notes $notes $d
        if ($d.state -eq "downloading") {
            foreach ($c in @($d.cables)) {
                [void]$samples.Add([pscustomobject]@{
                    At = $watch.Elapsed.TotalSeconds; Cable = $c.name; Connections = [int]$c.connections
                    Wanted = [int]$c.wanted; Bps = [double]$c.bps
                })
            }
        }
        Assert-Budget
        if ($d.state -in @("done", "failed", "stopped")) { break }
        if ($Each -and ((& $Each $d $table) -eq "stop")) {
            Stop-Download $id
            $d = Wait-For "the download to stop" {
                $now = Get-Download $id "$What stopping"
                if ($now.state -in @("stopped", "done", "failed")) { $now }
            } -Seconds 30
            break
        }
        if ((Get-Date) -gt $deadline) {
            Stop-Download $id
            throw "after $Minutes min the download is still $($d.state); note: $(Hide "$($d.note)")"
        }
    }
    $watch.Stop()
    # The engine publishes its counts with each status; one more beat for the last.
    Start-Sleep -Milliseconds 500
    $usedAfter = Get-EngineUsed
    $use = @{}
    foreach ($c in $Cables) {
        $use[$c] = [pscustomobject]@{
            Engine  = [double]$usedAfter[$c] - [double]$usedBefore[$c]
            Windows = (Get-AdapterReceived $c) - $received[$c]
        }
    }
    $requests = Measure-Requests (Read-Log $logStart).Text
    Clear-Downloads
    [pscustomobject]@{
        Download = $d; Secs = [math]::Round($watch.Elapsed.TotalSeconds, 1); Samples = $samples
        Notes = $notes; Use = $use; Requests = $requests
    }
}

function Pass([string]$Row, [bool]$Ok, [string]$Why) {
    $word = "FAIL"
    if ($Ok) { $word = "PASS" }
    "  $word  $Why"
    if (-not $script:Passed.Contains($Row)) { $script:Passed[$Row] = $true }
    if (-not $Ok) { $script:Passed[$Row] = $false }
}

# The data used row, for each cable that carried in a run: the engine's count
# at most a tenth under what Windows' adapter received, and never over it by
# more than a rounding.
function Show-Used([string]$Row, $Res) {
    foreach ($c in $Res.Use.Keys) {
        $u = $Res.Use[$c]
        if ($u.Engine -lt 1e6) { continue }
        $ratio = 0
        if ($u.Windows -gt 0) { $ratio = $u.Engine / $u.Windows }
        Pass $Row (($ratio -ge 0.9) -and ($ratio -le 1.02)) `
            ("data used, $($c): the engine $(MB $u.Engine) MB, Windows received $(MB $u.Windows) MB ({0:P0})" -f $ratio)
    }
}

function Show-Requests($Res) {
    foreach ($c in $Res.Requests.Keys) {
        "  $($c): $($Res.Requests[$c].Asked) requests, $($Res.Requests[$c].Refused) turned away"
    }
}

function Show-Shares($Res) {
    $d = $Res.Download
    "  $($d.state) in $($Res.Secs) s: $(MB $d.done) of $(MB $d.length) MB; note: $(Hide "$($d.note)")"
    foreach ($c in @($d.cables)) { "  $($c.name) carried $(MB $c.bytes) MB" }
    foreach ($n in $Res.Notes) { "  note seen: $n" }
}

# ---------------------------------------------------------------- the rows

function Invoke-Limited {
    $res = Invoke-Download $LimitedUrl @($Wifi, $script:TetherName) "limited"
    Show-Shares $res
    Show-Requests $res
    $d = $res.Download
    Pass "Limited" ($d.state -eq "done") "it finished"
    foreach ($c in @($Wifi, $script:TetherName)) {
        $bytes = [double](@($d.cables | Where-Object { $_.name -eq $c }) | Measure-Object -Property bytes -Sum).Sum
        Pass "Limited" ($bytes -gt 0) "$c carried part ($(MB $bytes) MB)"
        $asked = 0
        if ($res.Requests.ContainsKey($c)) { $asked = $res.Requests[$c].Asked }
        Pass "Limited" ($asked -lt 100) "$c asked $asked times: tens, not hundreds"
    }
    $rests = @($res.Notes | Where-Object { $_ -match "rests" })
    Pass "Limited" ($rests.Count -eq 0) "no cable rested ($($rests.Count) rest notes)"
    Show-Used "Limited" $res
}

function Invoke-Weights {
    $res = Invoke-Download $Url @($Wifi, $script:TetherName) "weights"
    Show-Shares $res
    $d = $res.Download
    Pass "Weights" ($d.state -eq "done") "it finished"
    $by = @{}
    foreach ($c in @($Wifi, $script:TetherName)) {
        $mine = @($res.Samples | Where-Object { $_.Cable -eq $c })
        $by[$c] = [pscustomobject]@{
            Bps         = [double]($mine | Where-Object { $_.Bps -gt 0 } | Measure-Object -Property Bps -Average).Average
            Connections = [double]($mine | Measure-Object -Property Connections -Average).Average
            Bytes       = [double](@($d.cables | Where-Object { $_.name -eq $c }) | Measure-Object -Property bytes -Sum).Sum
        }
        "  {0}: {1:N1} Mbps on average, {2:N1} connections on average" -f $c, ($by[$c].Bps / 1e6), $by[$c].Connections
        Pass "Weights" ($by[$c].Bytes -gt 0) "$c carried part ($(MB $by[$c].Bytes) MB)"
    }
    $fast, $slow = $Wifi, $script:TetherName
    if ($by[$slow].Bps -gt $by[$fast].Bps) { $fast, $slow = $slow, $fast }
    if ($by[$fast].Bps -lt 1.2 * $by[$slow].Bps) {
        "  SKIP  the two ran within a fifth of each other's speed: nothing to tell by"
        $script:Passed["Weights"] = $false
    } else {
        Pass "Weights" ($by[$fast].Connections -gt $by[$slow].Connections) "the faster, $fast, held more connections"
    }
    Show-Used "Weights" $res
}

# When the engine's mark and Windows' cost each turn to `To`, and the time
# between.
function Wait-Metered([bool]$To) {
    $windowsAt = $null
    $engineAt = $null
    Wait-For "the metered setting to change" {
        $now = Get-Date
        if ($null -eq $windowsAt -and (Get-WindowsMetered $script:TetherName) -eq $To) { $windowsAt = $now }
        $mark = @((Get-Links).links | Where-Object { $_.name -eq $script:TetherName -and $_.metered -eq $To }).Count -gt 0
        if ($null -eq $engineAt -and $mark) { $engineAt = $now }
        ($null -ne $windowsAt) -and ($null -ne $engineAt)
    } | Out-Null
    $lag = ($engineAt - $windowsAt).TotalSeconds
    Pass "Metered" ($lag -le 10) ("metered {0}: Windows at {1:HH:mm:ss}, the engine {2:N1} s later" -f $To, $windowsAt, $lag)
}

function Invoke-Metered {
    if ((Get-WindowsMetered $script:TetherName) -eq $true) { throw "the tether is already metered: turn it off first" }
    "  CUE: Settings > Network & internet > Ethernet > $($script:TetherName): turn Metered connection ON"
    Wait-Metered $true
    "  CUE: now turn Metered connection OFF again"
    Wait-Metered $false
}

function Test-Joined($Table) {
    foreach ($group in @($Table.connections)) {
        if ((@($group) -contains $Wifi) -and (@($group) -contains $script:TetherName)) { return $true }
    }
    return $false
}

function Invoke-Shared {
    "  CUE: on the phone, turn Wi-Fi on and join the laptop's network; keep USB tethering on"
    Wait-For "the engine to join the two cables" { Test-Joined (Get-Links) } | Out-Null
    "  the engine sees $Wifi and $($script:TetherName) on one connection"
    $res = Invoke-Download $Url @($Wifi, $script:TetherName) "shared"
    Show-Shares $res
    $d = $res.Download
    $tether = @($d.cables | Where-Object { $_.name -eq $script:TetherName })
    $tetherBytes = [double]($tether | Measure-Object -Property bytes -Sum).Sum
    $tetherConns = [int](@($res.Samples | Where-Object { $_.Cable -eq $script:TetherName }) |
        Measure-Object -Property Connections -Maximum).Maximum
    $note = @($tether | ForEach-Object { $_.notes }) -join " "
    Pass "Shared" ($d.state -eq "done") "it finished"
    Pass "Shared" ($tetherConns -eq 0 -and $tetherBytes -eq 0) "the tether held no connections and carried nothing"
    Pass "Shared" ($note -match "same connection as $([regex]::Escape($Wifi))") "the tether's note: $(Hide $note)"
    "  (the Links tab: both cards say 'shared with' the other)"
    "  CUE: on the phone, turn Wi-Fi off again, back to mobile data"
    Wait-For "the two to be separate connections again, both measured" {
        $table = Get-Links
        (-not (Test-Joined $table)) -and
            (@(Get-Usable $table $script:TetherName | Where-Object { $null -ne $_.down_bps }).Count -gt 0)
    } | Out-Null
    "  the tether is on its own connection again, and measured"
}

function Invoke-Flap {
    $script:Flap = [ordered]@{ Phase = "carry"; Off = $null; Degraded = $null; Up = $null; Share = $null; HeldDegraded = 0; FirstAfterUp = $null }
    "  waiting for the tether to carry, then the cue"
    $res = Invoke-Download $LongUrl @($Wifi, $script:TetherName) "flap" {
        param($d, $table)
        $f = $script:Flap
        $now = Get-Date
        $links = @($table.links | Where-Object { $_.name -eq $script:TetherName })
        $held = [int](@($d.cables | Where-Object { $_.name -eq $script:TetherName }) | Measure-Object -Property connections -Sum).Sum
        switch ($f.Phase) {
            "carry" {
                if ($held -gt 0) {
                    "  CUE: the tether carries ($held connections): turn USB tethering OFF now" | Out-Host
                    $f.Phase = "off"
                }
            }
            "off" {
                if (@($links | Where-Object { $_.state -in @("up", "degraded") }).Count -eq 0) {
                    $f.Off = $now
                    "  the tether has gone. CUE: turn USB tethering back ON within 20 s" | Out-Host
                    $f.Phase = "back"
                }
            }
            "back" {
                if (@($links | Where-Object { $_.state -eq "up" }).Count -gt 0) {
                    $f.Up = $now
                    $f.Phase = "share"
                    "  the tether came back straight up, not degraded" | Out-Host
                } elseif (@($links | Where-Object { $_.state -eq "degraded" }).Count -gt 0) {
                    $f.Degraded = $now
                    $f.Phase = "degraded"
                    "  the tether is back, degraded" | Out-Host
                }
            }
            "degraded" {
                if ($held -gt $f.HeldDegraded) { $f.HeldDegraded = $held }
                if (@($links | Where-Object { $_.state -eq "up" }).Count -gt 0) {
                    $f.Up = $now
                    $f.Phase = "share"
                    "  the tether is up again" | Out-Host
                }
            }
            "share" {
                if ($null -eq $f.FirstAfterUp -and $held -gt 0) { $f.FirstAfterUp = $held }
                if ($held -gt 1) {
                    $f.Share = $now
                    return "stop"
                }
                if (($now - $f.Up).TotalSeconds -gt 20) { return "stop" }
            }
        }
    }
    Show-Shares $res
    $f = $script:Flap
    Pass "Flap" ($null -ne $f.Degraded) "the tether came back degraded"
    if ($f.Degraded) {
        Pass "Flap" ($f.HeldDegraded -le 1) "it held at most one connection while degraded ($($f.HeldDegraded))"
    }
    if ($f.Degraded -and $f.Up) {
        $cool = ($f.Up - $f.Degraded).TotalSeconds
        Pass "Flap" ($cool -ge 25 -and $cool -le 40) ("up again {0:N1} s after it came back (the cooldown is 30 s)" -f $cool)
    }
    if ($f.Up) {
        $at = $null
        if ($f.Share) { $at = ($f.Share - $f.Up).TotalSeconds }
        Pass "Flap" ($null -ne $at -and $at -le 5 -and $f.FirstAfterUp -gt 1) `
            ("its share at once: {0} connections first after up, more than one {1} s after" -f $f.FirstAfterUp, $at)
    }
    Show-Used "Flap" $res
}

# ---------------------------------------------------------------- session

New-Item -ItemType Directory -Force $Work | Out-Null
if ($Rows.Count -gt 0 -or $Rehearse) {
    Remove-Item -Recurse -Force -LiteralPath $downloads -ErrorAction SilentlyContinue
}
try {
    if ($Rehearse) {
        "== Rehearsal: the Wi-Fi alone, no phone"
        $others = @(Get-OtherWaysOut)
        if ($others.Count -gt 0) { throw "a rehearsal is the Wi-Fi alone, and $($others -join ', ') has a way out too" }
        $range = Test-Range $Wifi (Get-Address $Wifi) $Url
        if (-not $range.Ok) { throw "$($Wifi): a ranged 1 KB request answered '$($range.Answer)', not '206 1024'" }
        "  $($Wifi): the file is $(MB $range.Length) MB"
        "  Windows says $Wifi is metered: $(Get-WindowsMetered $Wifi)"
        $script:Proc = Start-Engine
        Wait-For "the Wi-Fi measured" { @(Get-Usable (Get-Links) $Wifi | Where-Object { $null -ne $_.down_bps }).Count -gt 0 } -Seconds 120 | Out-Null
        $mark = @((Get-Links).links | Where-Object { $_.name -eq $Wifi } | ForEach-Object { $_.metered })
        "  the engine says $Wifi is metered: $($mark -join ', ')"
        $res = Invoke-Download $Url @($Wifi) "rehearsal"
        Show-Shares $res
        Show-Requests $res
        Show-Used "Rehearsal" $res
        return
    }

    "== Preflight: no engine yet"
    $script:TetherName = Find-Tether
    $script:TetherStart = Get-AdapterBytes $script:TetherName
    "  the tether is $($script:TetherName)"
    $wifiAddress = Get-Address $Wifi
    $tetherAddress = Get-Address $script:TetherName
    if ((Get-PublicAddress $Wifi $wifiAddress) -eq (Get-PublicAddress $script:TetherName $tetherAddress)) {
        throw ("$Wifi and $($script:TetherName) reach the internet from one public address: the phone is on " +
            "the laptop's network, not on mobile data. Turn the phone's Wi-Fi off.")
    }
    "  the two reach the internet from different public addresses"
    "  Windows says the tether is metered: $(Get-WindowsMetered $script:TetherName)"
    $sizes = @{}
    foreach ($f in @(@("Limited", $LimitedUrl), @("Weights", $Url), @("Flap", $LongUrl))) {
        $range = Test-Range $Wifi $wifiAddress $f[1]
        if (-not $range.Ok) { throw "$($f[0])'s file: a ranged 1 KB request through $Wifi answered '$($range.Answer)', not '206 1024'" }
        $sizes[$f[0]] = $range.Length / 1e6
        "  $($f[0])'s file: $([math]::Round($sizes[$f[0]], 1)) MB, ranges answered (asked through $Wifi only)"
    }

    # The most each row can cost the phone, a tenth over the payload: a row's
    # file all through the tether at worst; a network change, a speed test.
    $margin = 1.1
    $costs = [ordered]@{
        Start   = $SpeedTestMB
        Limited = $sizes["Limited"]
        Weights = $sizes["Weights"]
        Metered = 0
        Shared  = $SpeedTestMB
        # The tether's speed test on coming back, its one connection through
        # the cooldown, a few seconds of its share before and after.
        Flap    = $SpeedTestMB + 200
    }
    $total = $costs.Start
    foreach ($r in $Rows) { $total += $costs[$r] }
    $total = [math]::Ceiling($total * $margin)
    if ($Rows.Count -eq 0) {
        ""
        "== What each row can cost in mobile data, at most, a tenth over"
        foreach ($k in $costs.Keys) { "  {0,-8} {1,5} MB" -f $k, [math]::Ceiling($costs[$k] * $margin) }
        $all = $costs.Start
        foreach ($r in $Order) { $all += $costs[$r] }
        "  every row in one session: $([math]::Ceiling($all * $margin)) MB"
        "  Agree a budget, then run again with -Rows and -BudgetMB."
        return
    }
    if ($BudgetMB -lt $total) { throw "these rows can cost up to $total MB of mobile data, more than -BudgetMB $BudgetMB" }

    ""
    "== The engine (log: $($script:EngineLog))"
    $script:Proc = Start-Engine -Open
    Wait-For "both cables usable and measured" {
        $table = Get-Links
        @(Get-Usable $table $Wifi | Where-Object { $null -ne $_.down_bps }).Count -gt 0 -and
            @(Get-Usable $table $script:TetherName | Where-Object { $null -ne $_.down_bps }).Count -gt 0
    } -Seconds 180 | Out-Null
    "  up at http://127.0.0.1:$Port, both cables measured; the tether has carried $(MB ((Get-AdapterBytes $script:TetherName) - $script:TetherStart)) MB so far"

    foreach ($r in $Rows) {
        Wait-Go $r
        & "Invoke-$r"
        "  mobile data so far, both ways: $(MB ((Get-AdapterBytes $script:TetherName) - $script:TetherStart)) MB"
    }

    ""
    "== Rows"
    foreach ($k in $script:Passed.Keys) {
        $word = "FAIL"
        if ($script:Passed[$k]) { $word = "PASS" }
        "  $word  $k"
    }
    "  mobile data this session, both ways: $(MB ((Get-AdapterBytes $script:TetherName) - $script:TetherStart)) MB"
    "  timeline: $timeline"
}
finally {
    Stop-Engine $script:Proc
}
