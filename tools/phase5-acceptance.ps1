<#
    BARRAGE phase 5 (BRG-77): acceptance on this machine. Does a torrent come
    down through both cables at once, with a metered cable never uploading,
    and DHT, UDP trackers and incoming peers on one cable?

    The rows (-Rows), which run in this order:
      Cables    -Torrent, both cables: it finishes, both carry, the faster
                cable holds more peer connections. The same run judges the
                upload line (the tether's sent under 1% of its received, the
                Wi-Fi uploads), the UDP line (the engine's UDP sockets on the
                UDP cable alone, none on the other, and the page names that
                cable) and the data used line (each cable's received and sent
                within 15% of Windows' adapter counts).
      Flap      -FlapTorrent, both cables; tethering off once the tether
                carries, on again: its peers move to the Wi-Fi, and once back
                it takes peer connections again after its cooldown.
      Page      By hand, on the page: a magnet link (-Magnet) and a .torrent
                file (-PageTorrent) added, one stopped and resumed.
      Incoming  -LongTorrent with TETHERING OFF, so it costs no mobile data:
                the port opens on the UDP cable; it closes after 10 running
                minutes with nobody calling in, and says why, or stays open
                once a peer calls. -LongTorrent must run past 10 minutes.
                Give it a session of its own: the 10 minutes count from the
                engine's start, so the rows before it use them up.

    "UDP counts": Windows counts no UDP bytes per adapter, so the row reads
    the addresses of the engine's own UDP sockets (Get-NetUDPEndpoint) a few
    times during the run.

    A YES PER ROW, as in phase 4: before each row the script waits for the
    file go-<Row> in -Work. Within a row it waits on what it can see: the
    engine's torrents and table, Windows' adapters. Each cue says what to do.

    USAGE
      # The preflight: no engine, a few hundred bytes through each cable.
      .\tools\phase5-acceptance.ps1
      # The session, once its cost is agreed:
      .\tools\phase5-acceptance.ps1 -Rows Cables,Flap,Page,Incoming -BudgetMB <agreed> `
          -Torrent a.torrent -FlapTorrent b.torrent -PageTorrent c.torrent `
          -Magnet "magnet:?..." -LongTorrent d.torrent

    MOBILE DATA. A torrent through both cables can take any share of its size
    through the tether, so each costs at most its size, and more with the
    overhead. The script stops the engine if the tether's counter passes
    -BudgetMB. Every address printed is masked.
#>

param(
    [ValidateSet("Cables", "Flap", "Page", "Incoming")][string[]]$Rows = @(),
    [string]$Torrent = "",
    [string]$FlapTorrent = "",
    [string]$PageTorrent = "",
    [string]$LongTorrent = "",
    [string]$Magnet = "",
    [string]$Wifi = "Wi-Fi",
    [string]$Tether = "",
    [double]$BudgetMB = 0,
    # The longest one torrent, or one wait on the user, may take.
    [int]$Minutes = 30,
    [int]$Port = 7700,
    [string]$Engine = "",
    [string]$Work = ""
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "acceptance-lib.ps1")

$Order = @("Cables", "Flap", "Page", "Incoming")
$Rows = @($Order | Where-Object { $Rows -contains $_ })
$Needs = @{
    Cables   = @{ Torrent = $Torrent }
    Flap     = @{ FlapTorrent = $FlapTorrent }
    Page     = @{ Magnet = $Magnet; PageTorrent = $PageTorrent }
    Incoming = @{ LongTorrent = $LongTorrent }
}
foreach ($r in $Rows) {
    foreach ($need in $Needs[$r].GetEnumerator()) {
        if (-not $need.Value) { throw "the $r row needs -$($need.Key)" }
        if ($need.Key -ne "Magnet" -and -not (Test-Path -LiteralPath $need.Value)) {
            throw "there is no file $($need.Value) for -$($need.Key)"
        }
    }
}
if (-not $Engine) { $Engine = Join-Path $PSScriptRoot "..\engine\target\release\barrage.exe" }
if ($Rows.Count -gt 0 -and -not (Test-Path $Engine)) {
    throw "no engine at $Engine. Build it first, in engine: cargo build --release --bin barrage"
}
if (-not $Work) { $Work = Join-Path $env:TEMP "brg-phase5" }
# A full path: .NET resolves a relative one against the process's folder,
# which Windows PowerShell does not keep in step with the location.
$Work = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Work)
# A name only this script uses: emptied at each session.
$downloads = Join-Path $Work "brg-phase5-downloads"
$api = "http://127.0.0.1:$Port/api"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$timeline = Join-Path $Work "timeline-$stamp.jsonl"
$script:EngineLog = Join-Path $Work "engine-$stamp.log"
$script:Proc = $null
$script:TetherName = $null
$script:TetherStart = 0.0
# Every link id the engine has shown, by cable.
$script:LinkNames = @{}
$script:Passed = [ordered]@{}
# Rows with a check that could not be judged, and why.
$script:Skipped = @{}
$Url = ""

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

# A check that could not be judged: the row's summary says SKIP unless
# another check failed.
function Skip([string]$Row, [string]$Why) {
    "  SKIP  $Why"
    if (-not $script:Passed.Contains($Row)) { $script:Passed[$Row] = $true }
    $script:Skipped[$Row] = $true
}

function Pass([string]$Row, [bool]$Ok, [string]$Why) {
    $word = "FAIL"
    if ($Ok) { $word = "PASS" }
    "  $word  $Why"
    if (-not $script:Passed.Contains($Row)) { $script:Passed[$Row] = $true }
    if (-not $Ok) { $script:Passed[$Row] = $false }
}

# ---------------------------------------------------------------- torrents

function Get-Torrents { Invoke-RestMethod "$api/torrents" }

# One torrent now, written to the timeline as `what` with the time.
function Get-Torrent([int]$Id, [string]$What = "") {
    $t = @((Get-Torrents).list | Where-Object { $_.id -eq $Id })[0]
    $line = [pscustomobject]@{
        at      = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
        what    = $What
        torrent = $t
    }
    Add-Content -LiteralPath $timeline -Encoding UTF8 -Value (Hide ($line | ConvertTo-Json -Compress -Depth 6))
    return $t
}

function Add-TorrentFile([string]$Path) {
    $added = Invoke-RestMethod -Method Post -Uri "$api/torrents" -ContentType "application/x-bittorrent" -InFile $Path
    return [int]$added.id
}

function Send-Control([int]$Id, [string]$Action) {
    try {
        Invoke-RestMethod -Method Post -Uri "$api/torrents/$Id/$Action" -ContentType "application/json" -Body "{}" | Out-Null
    } catch { }
}

# What a torrent holds through a cable: its row, or nothing yet.
function Get-Part($Torrent, [string]$Name) {
    @($Torrent.cables | Where-Object { $_.name -eq $Name })[0]
}

function Get-Connections($Torrent, [string]$Name) {
    $part = Get-Part $Torrent $Name
    if ($part) { return [int]$part.connections }
    return 0
}

# Each cable's received and sent as the engine counts them, every engine
# together: the two sides of the data used line.
function Get-EngineBoth {
    $used = @{}
    foreach ($u in @((Invoke-RestMethod "$api/downloads").used)) {
        $used[$u.cable] = [double]$u.bytes + [double]$u.sent
    }
    return $used
}

# The local addresses of the engine's UDP sockets, loopback left out.
function Get-UdpAddresses {
    @(Get-NetUDPEndpoint -OwningProcess $script:Proc.Id -ErrorAction SilentlyContinue |
        ForEach-Object { $_.LocalAddress } | Sort-Object -Unique |
        Where-Object { $_ -notin @("127.0.0.1", "::1") })
}

# The local addresses the engine listens on for TCP, its API's loopback left out.
function Get-Listening {
    @(Get-NetTCPConnection -OwningProcess $script:Proc.Id -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object { $_.LocalAddress } | Sort-Object -Unique |
        Where-Object { $_ -notin @("127.0.0.1", "::1") })
}

# What each cable moves over `Seconds` while nothing of ours should: other
# programs' traffic, which Windows' count holds and the engine's does not.
function Get-Quiet([string[]]$Cables, [int]$Seconds = 5) {
    $before = @{}
    foreach ($c in $Cables) { $before[$c] = Get-AdapterBytes $c }
    Start-Sleep -Seconds $Seconds
    $got = @{}
    foreach ($c in $Cables) { $got[$c] = (Get-AdapterBytes $c) - $before[$c] }
    return $got
}

# ---------------------------------------------------------------- rows

function Invoke-Cables {
    $cables = @($Wifi, $script:TetherName)
    $table = Get-Links
    $speed = @{}
    foreach ($c in $cables) {
        $speed[$c] = [double]@(Get-Usable $table $c | Where-Object { $null -ne $_.down_bps } | Select-Object -First 1).down_bps
    }
    $fast = $cables | Sort-Object { -$speed[$_] } | Select-Object -First 1
    $slow = $cables | Where-Object { $_ -ne $fast }
    "  the faster cable is $fast ($(MB ($speed[$fast] / 8)) MB/s measured; $slow $(MB ($speed[$slow] / 8)))"

    $quietBefore = Get-Quiet $cables
    $adapter = @{}
    foreach ($c in $cables) { $adapter[$c] = Get-AdapterBytes $c }
    $engine = Get-EngineBoth

    $id = Add-TorrentFile $Torrent
    "  added as torrent $id"
    $held = @{}
    foreach ($c in $cables) { $held[$c] = [System.Collections.Generic.List[int]]::new() }
    $addresses = [System.Collections.Generic.HashSet[string]]::new()
    $named = $null
    $deadline = (Get-Date).AddMinutes($Minutes)
    $polls = 0
    while ($true) {
        $t = Get-Torrent $id "Cables"
        if ($t.phase -in @("done", "failed")) { break }
        if ((Get-Date) -gt $deadline) { throw "torrent $id was not done after $Minutes minutes" }
        if ($t.phase -eq "downloading") {
            foreach ($c in $cables) { $held[$c].Add((Get-Connections $t $c)) }
        }
        if ($polls % 10 -eq 0) {
            foreach ($a in (Get-UdpAddresses)) { [void]$addresses.Add($a) }
            $named = (Get-Torrents).udp_cable
            "  {0:N1}% done, {1} peers, {2}" -f (100.0 * [double]$t.done / [math]::Max(1.0, [double]$t.length)), $t.peers,
                (($cables | ForEach-Object { "$_ $(Get-Connections $t $_)" }) -join ", ")
        }
        $polls += 1
        Assert-Budget
        Start-Sleep -Seconds 1
    }

    Pass "Cables" ($t.phase -eq "done") "the torrent finished, every piece checked ($($t.phase))"
    foreach ($c in $cables) {
        $part = Get-Part $t $c
        Pass "Cables" ($part -and [double]$part.received -gt 0) "$c carried: $(MB ([double]$part.received)) MB received"
    }
    $mean = @{}
    foreach ($c in $cables) { $mean[$c] = [double]($held[$c] | Measure-Object -Average).Average }
    Pass "Cables" ($mean[$fast] -gt $mean[$slow]) ("$fast held more peer connections: {0:N1} on average, $slow {1:N1}" -f $mean[$fast], $mean[$slow])

    $phone = Get-Part $t $script:TetherName
    $wifi_ = Get-Part $t $Wifi
    if ($phone) {
        Pass "Upload" ([double]$phone.sent -lt 0.01 * [double]$phone.received) `
            "the tether sent $(MB ([double]$phone.sent)) MB, under 1% of the $(MB ([double]$phone.received)) MB it received"
    }
    if ($wifi_) { Pass "Upload" ([double]$wifi_.sent -gt 0) "the Wi-Fi uploaded $(MB ([double]$wifi_.sent)) MB" }

    $tetherAddress = Get-Address $script:TetherName
    $wifiAddress = Get-Address $Wifi
    $seen = @($addresses)
    Pass "UDP" ($seen.Count -gt 0) "the engine's UDP sockets were seen: $(Hide ($seen -join ', '))"
    Pass "UDP" ($seen -notcontains $tetherAddress) "none on the tether"
    Pass "UDP" (@($seen | Where-Object { $_ -in @("0.0.0.0", "::") }).Count -eq 0) "none left to Windows' choice of link"
    Pass "UDP" ($named -eq $Wifi -and $seen -contains $wifiAddress) "the page names $named, and the sockets are on it"

    $quietAfter = Get-Quiet $cables
    $after = Get-EngineBoth
    foreach ($c in $cables) {
        if ($quietBefore[$c] -gt 1e5 -or $quietAfter[$c] -gt 1e5) {
            Skip "Used" ("$c was not quiet ({0} MB, then {1} MB, in 5 s): another program's traffic blurs it" -f (MB $quietBefore[$c]), (MB $quietAfter[$c]))
            continue
        }
        $windows = (Get-AdapterBytes $c) - $adapter[$c]
        $counted = [double]$after[$c] - [double]$engine[$c]
        $off = 100.0 * [math]::Abs($counted - $windows) / [math]::Max(1.0, $windows)
        Pass "Used" ($off -le 15) ("$c`: the engine counted {0} MB both ways, Windows {1} MB, {2:N1}% apart" -f (MB $counted), (MB $windows), $off)
    }
}

function Invoke-Flap {
    $name = $script:TetherName
    $id = Add-TorrentFile $FlapTorrent
    "  added as torrent $id"
    Wait-For "the tether to take peers" { (Get-Connections (Get-Torrent $id "Flap") $name) -gt 0 } | Out-Null
    $before = [double](Get-Torrent $id "Flap").done
    "  CUE: the tether carries: turn USB tethering OFF now"
    Wait-For "the tether to go" { @(Get-Usable (Get-Links) $name).Count -eq 0 } | Out-Null
    $gone = Get-Date
    "  the tether has gone"
    Wait-For "its connections to end and the Wi-Fi to hold peers" {
        $t = Get-Torrent $id "Flap"
        (Get-Connections $t $name) -eq 0 -and (Get-Connections $t $Wifi) -gt 0
    } -Seconds 30 | Out-Null
    Pass "Flap" $true ("its peers moved to the Wi-Fi within {0:N0} s" -f ((Get-Date) - $gone).TotalSeconds)
    $during = Get-Torrent $id "Flap"
    Pass "Flap" ($during.phase -eq "downloading" -or $during.phase -eq "done") "the torrent carried on ($($during.phase))"
    Pass "Flap" ([double]$during.done -ge $before) "nothing it had was lost"

    "  CUE: turn USB tethering back ON"
    Wait-For "the tether to come back" { @(Get-Usable (Get-Links) $name).Count -gt 0 } | Out-Null
    $back = Get-Date
    "  the tether is back"
    $took = $false
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        $t = Get-Torrent $id "Flap"
        if ($t.phase -eq "done") { break }
        if ((Get-Connections $t $name) -gt 1) { $took = $true; break }
        Assert-Budget
        Start-Sleep -Milliseconds 500
    }
    if ($t.phase -eq "done") {
        Skip "Flap" "not judged: the torrent finished before the tether took peers again"
    } else {
        Pass "Flap" $took ("the tether took peer connections again {0:N0} s after it came back" -f ((Get-Date) - $back).TotalSeconds)
    }
    Send-Control $id "stop"
    "  torrent $id stopped, to spend no more"
}

function Invoke-Page {
    $start = @((Get-Torrents).list).Count
    "  CUE: on the page, Add download: paste the magnet link from -Magnet, and press Download"
    Wait-For "the magnet in the list" { @((Get-Torrents).list).Count -gt $start } | Out-Null
    $first = @((Get-Torrents).list)[-1]
    Pass "Page" $true "a magnet link added, as torrent $($first.id)"

    "  CUE: Add download again: Choose a .torrent file, and pick $PageTorrent"
    Wait-For "the file in the list" { @((Get-Torrents).list).Count -gt ($start + 1) } | Out-Null
    $second = @((Get-Torrents).list)[-1]
    Pass "Page" $true "a .torrent file added, as torrent $($second.id)"

    "  CUE: press Stop on torrent $($second.id)"
    Wait-For "it stopped" { (Get-Torrent $second.id "Page").phase -eq "stopped" } | Out-Null
    Pass "Page" $true "Stop worked"
    "  CUE: press Resume on torrent $($second.id)"
    Wait-For "it resumed" { (Get-Torrent $second.id "Page").phase -ne "stopped" } | Out-Null
    Pass "Page" $true "Resume worked"
    "  Look at the two cards: progress, peers, each cable's part, and the line above the queue."
    "  They are stopped when the row ends. Note anything wrong for the results."
    foreach ($t in @($first, $second)) { Send-Control $t.id "stop" }
}

function Invoke-Incoming {
    $name = $script:TetherName
    "  CUE: turn USB tethering OFF, so this row costs no mobile data"
    Wait-For "the tether to go" { @(Get-Usable (Get-Links) $name).Count -eq 0 } | Out-Null
    $wifiAddress = Get-Address $Wifi
    $id = Add-TorrentFile $LongTorrent
    "  added as torrent $id; a peer may call in, or 10 running minutes may pass"
    $seen = Get-Listening
    # The try counts every running minute since the engine started, so rows
    # before this one may have used it up: run this row on its own to judge it.
    if ((Get-Torrents).listening.state -eq "closed") {
        Skip "Incoming" "not judged: the port had closed in an earlier row. Run -Rows Incoming alone, on a fresh engine"
        Send-Control $id "stop"
        return
    }
    Pass "Incoming" ($seen -contains $wifiAddress) "the port is open on the UDP cable ($(Hide ($seen -join ', ')))"
    $deadline = (Get-Date).AddMinutes($Minutes)
    while ($true) {
        $swarm = Get-Torrents
        $t = @($swarm.list | Where-Object { $_.id -eq $id })[0]
        if ($swarm.listening.state -in @("open", "closed")) { break }
        if ($t.phase -in @("done", "failed")) { break }
        if ((Get-Date) -gt $deadline) { throw "the port was still $($swarm.listening.state) after $Minutes minutes" }
        Start-Sleep -Seconds 5
    }
    $state = $swarm.listening.state
    if ($state -eq "open") {
        Pass "Incoming" ((Get-Listening) -contains $wifiAddress) "a peer called in and the port stays open"
    } elseif ($state -eq "closed") {
        $closed = Get-Date
        "  the page says: $(Hide $swarm.listening.why)"
        Pass "Incoming" ([bool]$swarm.listening.why) "it closed, and says why"
        Wait-For "the listener to go" { (Get-Listening).Count -eq 0 } -Seconds 60 | Out-Null
        Pass "Incoming" $true "the listener is gone"
        Wait-For "the torrent back at work" {
            $t = Get-Torrent $id "Incoming"
            [double]$t.down_bps -gt 0 -or $t.phase -eq "done"
        } | Out-Null
        "  the rebuild cost this torrent {0:N0} s without downloading" -f ((Get-Date) - $closed).TotalSeconds
    } else {
        Skip "Incoming" "not judged: the torrent was $($t.phase) before 10 running minutes passed"
    }
    Send-Control $id "stop"
    "  torrent $id stopped"
}

# ---------------------------------------------------------------- session

New-Item -ItemType Directory -Force $Work | Out-Null
if ($Rows.Count -gt 0) { Remove-Item -Recurse -Force -LiteralPath $downloads -ErrorAction SilentlyContinue }
try {
    "== Preflight: no engine yet"
    "  (other programs' downloads blur the data used line: pause Windows Update and quit Steam and OneDrive for the session)"
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
    $metered = Get-WindowsMetered $script:TetherName
    "  Windows says the tether is metered: $metered"
    if ($metered -ne $true) {
        throw "the no-upload line needs the tether metered: turn on Metered connection for it in Windows' Settings"
    }
    if ($Rows.Count -eq 0) {
        ""
        "  Each torrent can cost up to its size in mobile data, a fifth over."
        "  Agree the data and the torrents, then run again with -Rows and -BudgetMB."
        return
    }
    if ($BudgetMB -le 0) { throw "give the agreed mobile data as -BudgetMB" }

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
        if ($script:Passed[$k] -and $script:Skipped[$k]) { $word = "SKIP" }
        "  $word  $k"
    }
    "  mobile data this session, both ways: $(MB ((Get-AdapterBytes $script:TetherName) - $script:TetherStart)) MB"
    "  timeline: $timeline"
}
finally {
    Stop-Engine $script:Proc
}
