<#
    BARRAGE phase 6 (BRG-91): acceptance on this machine. Do more transfers
    than the limit wait in line, in priority order, and does everything come
    back after a restart, even after the engine is killed?

    The rows (-Rows), which run in this order:
      Queue         -Urls and -Torrent, more transfers than the limit: as many
                    run as the limit, the rest are queued with a place, and
                    each starts as a place frees up. Stopping one frees its
                    place; resumed, it joins the back of the line.
      Priority      With a line waiting: the last one set high goes first, and
                    nothing running is stopped for it.
      CleanRestart  You close the engine with Ctrl+C in its window, transfers
                    mid-way. On restart the list, order, priorities and limit
                    are back, and they carry on.
      Kill          The engine killed outright, transfers mid-way. The same,
                    and no file is fetched again beyond the last moments.
      Check         Every finished file: each -Sha256 file by its hash, and
                    each torrent by a full piece check. Fast resume reads only
                    a sample, so this starts an engine with an empty state
                    folder over the same files: done, with nothing fetched,
                    means every piece passed.
      Resume        -LongTorrent, large and part-fetched, killed and started
                    again: how long it takes to come back, beside how long a
                    full check of it takes.
      Damaged       No network: state.json overwritten with rubbish. The engine
                    starts with an empty list, keeps the file aside, and the
                    page says so.

    A YES PER ROW, as in phases 4 and 5: before each row the script waits for
    the file go-<Row> in -Work. Each cue says what to do. Every address
    printed is masked.

    USAGE
      # Lists the rows and touches nothing, the network least of all:
      .\tools\phase6-acceptance.ps1
      # The session, once its files and data are agreed:
      .\tools\phase6-acceptance.ps1 -Rows Queue,Priority,CleanRestart,Kill,Check `
          -Urls a,b,c,d -Torrent e.torrent -Sha256 @{ "a.iso" = "<hash>" }

    Its state and downloads are under -Work, never the user's own. The
    downloads are removed at the end unless -Keep is given.
#>

param(
    [ValidateSet("Queue", "Priority", "CleanRestart", "Kill", "Check", "Resume", "Damaged")]
    [string[]]$Rows = @(),
    [string[]]$Urls = @(),
    [string]$Torrent = "",
    [string]$LongTorrent = "",
    [hashtable]$Sha256 = @{},
    # The longest one wait may take.
    [int]$Minutes = 20,
    [int]$Port = 7700,
    [string]$Engine = "",
    [string]$Work = "",
    [switch]$Keep
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "acceptance-lib.ps1")

$Order = @("Queue", "Priority", "CleanRestart", "Kill", "Check", "Resume", "Damaged")
$Rows = @($Order | Where-Object { $Rows -contains $_ })
if ($Rows.Count -eq 0) {
    Write-Output "No rows given, so nothing was run. The rows are: $($Order -join ', ')."
    return
}
if ($Rows -contains "Queue" -and ($Urls.Count -lt 3 -or -not $Torrent)) {
    throw "the Queue row needs -Torrent and at least three -Urls: more transfers than the limit"
}
if ($Rows -contains "Resume" -and -not $LongTorrent) { throw "the Resume row needs -LongTorrent" }
foreach ($file in @($Torrent, $LongTorrent)) {
    if ($file -and -not (Test-Path -LiteralPath $file)) { throw "there is no file $file" }
}
if (-not $Engine) { $Engine = Join-Path $PSScriptRoot "..\engine\target\release\barrage.exe" }
if (-not (Test-Path $Engine)) {
    throw "no engine at $Engine. Build it first, in engine: cargo build --release --bin barrage"
}
if (-not $Work) { $Work = Join-Path $env:TEMP "brg-phase6" }
$Work = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Work)
# Names only this script uses: emptied at each session.
$downloads = Join-Path $Work "brg-phase6-downloads"
$script:StateDir = Join-Path $Work "brg-phase6-state"
$api = "http://127.0.0.1:$Port/api"
$script:EngineLog = Join-Path $Work "engine-$(Get-Date -Format 'yyyyMMdd-HHmmss').log"
$script:Proc = $null
$script:Passed = [ordered]@{}

# ---------------------------------------------------------------- helpers

function Wait-For([string]$What, [scriptblock]$Check, [int]$Seconds = $Minutes * 60) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $got = & $Check
        if ($got) { return $got }
        Start-Sleep -Milliseconds 500
    }
    throw "never $What, in $Seconds s"
}

function Wait-Go([string]$Row, [string]$Cue) {
    Write-Output ""
    Write-Output "== ${Row}: $Cue"
    $go = Join-Path $Work "go-$Row"
    Remove-Item -LiteralPath $go -ErrorAction SilentlyContinue
    Write-Output "   When ready, create the file $go"
    Wait-For "the go for $Row" { Test-Path -LiteralPath $go } (24 * 3600) | Out-Null
}

function Post([string]$Path, [string]$Body = "{}") {
    Invoke-RestMethod -Method Post -Uri "$api$Path" -ContentType "application/json" -Body $Body
}

function Add-Torrent([string]$File) {
    Invoke-RestMethod -Method Post -Uri "$api/torrents" -ContentType "application/x-bittorrent" `
        -InFile $File
}

function Get-Queue { Invoke-RestMethod "$api/queue" }
function Get-Webs { @((Invoke-RestMethod "$api/downloads").list) }
function Get-Swarm { @((Invoke-RestMethod "$api/torrents").list) }

# Each transfer as "kind id state priority", in id order, to compare two runs.
function Get-Shape {
    $webs = Get-Webs | ForEach-Object { "web $($_.id) $($_.priority)" }
    $swarm = Get-Swarm | ForEach-Object { "torrent $($_.id) $($_.priority)" }
    return (@($webs) + @($swarm) | Sort-Object) -join "; "
}

function Get-TotalReceived {
    $used = @((Invoke-RestMethod "$api/downloads").used)
    return ($used | Measure-Object -Property bytes -Sum).Sum
}

function Assert-That([bool]$Ok, [string]$What) {
    if (-not $Ok) { throw "FAILED: $What" }
    Write-Output "   ok: $What"
}

# ---------------------------------------------------------------- the rows

function Test-Queue {
    $added = @($Urls | ForEach-Object { Post "/downloads" (@{ url = $_ } | ConvertTo-Json) })
    Add-Torrent $Torrent | Out-Null
    $queue = Wait-For "transfers in line" {
        $q = Get-Queue
        if ($q.holding -ge 1 -and @($q.places).Count -ge 1) { $q }
    } 60
    Assert-That ($queue.holding -le $queue.limit) "$($queue.holding) run, the limit is $($queue.limit)"
    Assert-That (@($queue.places)[0].place -eq 1) "the line starts at place 1"
    Assert-That ($added.Count -ge 3) "$($added.Count) web downloads added"

    $first = $added[0].id
    Post "/downloads/$first/stop" | Out-Null
    Wait-For "a stopped one's place going to the next" {
        (Get-Queue).holding -le (Get-Queue).limit
    } 60 | Out-Null
    Post "/downloads/$first/resume" | Out-Null
    $back = @((Get-Queue).places | Where-Object { $_.kind -eq "web" -and $_.id -eq $first })
    Assert-That ($back.Count -le 1) "resumed, it joins the line"
}

function Test-Priority {
    $line = Wait-For "two in line" { $p = @((Get-Queue).places); if ($p.Count -ge 2) { , $p } } 300
    $last = $line[-1]
    $route = if ($last.kind -eq "web") { "downloads" } else { "torrents" }
    $running = (Get-Queue).holding
    Post "/$route/$($last.id)/priority" '{"priority": "high"}' | Out-Null
    Wait-For "the high one first in line" {
        $top = @((Get-Queue).places)[0]
        $top.kind -eq $last.kind -and $top.id -eq $last.id
    } 30 | Out-Null
    Assert-That ((Get-Queue).holding -eq $running) "nothing running was stopped for it"
}

function Test-Back([string]$Before, [double]$Received) {
    Wait-For "the engine answering" { try { Get-Queue } catch { $null } } 60 | Out-Null
    Assert-That ((Get-Shape) -eq $Before) "the list and the priorities are back"
    $now = Get-TotalReceived
    Assert-That ($now -ge $Received) "data used carried over: $(MB $now) MB"
    Wait-For "something running again" { (Get-Queue).holding -ge 1 } 120 | Out-Null
}

function Test-CleanRestart {
    $before = Get-Shape
    $received = Get-TotalReceived
    Write-Output "   Press Ctrl+C in the engine's window now."
    Wait-For "the engine closing" { $script:Proc.HasExited } 300 | Out-Null
    $script:Proc = Start-Engine
    Test-Back $before $received
}

function Test-Kill {
    $before = Get-Shape
    $received = Get-TotalReceived
    Stop-Engine $script:Proc
    $script:Proc = Start-Engine
    Test-Back $before $received
}

function Test-Check {
    Wait-For "everything finished" {
        @(Get-Webs | Where-Object { $_.state -notin @("done", "failed") }).Count -eq 0 -and
        @(Get-Swarm | Where-Object { $_.phase -notin @("done", "failed") }).Count -eq 0
    } | Out-Null
    foreach ($name in $Sha256.Keys) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $downloads $name) -Algorithm SHA256).Hash
        Assert-That ($hash -eq "$($Sha256[$name])".ToUpper()) "$name matches its hash"
    }
    Stop-Engine $script:Proc
    if (-not $Torrent) { return }
    # An empty state folder: no fast resume data, so every piece is read.
    $script:StateDir = Join-Path $Work "brg-phase6-check-state"
    Remove-Item -LiteralPath $script:StateDir -Recurse -Force -ErrorAction SilentlyContinue
    $script:Proc = Start-Engine
    Add-Torrent $Torrent | Out-Null
    Wait-For "every piece checked" { @(Get-Swarm | Where-Object { $_.phase -eq "done" }).Count -eq 1 } | Out-Null
    $fetched = Get-TotalReceived
    Assert-That ($fetched -lt 50e6) "every piece passed, $(MB $fetched) MB fetched"
}

function Test-Resume {
    Add-Torrent $LongTorrent | Out-Null
    Wait-For "a good part of it here" {
        $t = @(Get-Swarm)[-1]
        $t.length -gt 0 -and $t.done / $t.length -gt 0.3
    } | Out-Null
    Stop-Engine $script:Proc
    $started = Get-Date
    $script:Proc = Start-Engine
    Wait-For "it back and running" {
        $t = @(Get-Swarm)[-1]
        $t -and $t.phase -eq "downloading" -and $t.peers -gt 0
    } | Out-Null
    Write-Output "   Back in $([int]((Get-Date) - $started).TotalSeconds) s. Compare a full check of its size."
}

function Test-Damaged {
    Stop-Engine $script:Proc
    Set-Content -LiteralPath (Join-Path $script:StateDir "state.json") -Value "not a saved list"
    $script:Proc = Start-Engine
    $queue = Wait-For "the engine answering" { try { Get-Queue } catch { $null } } 60
    Assert-That ($queue.notice -match "could not be read") "the page is told"
    Assert-That (@(Get-Webs).Count -eq 0) "it started with an empty list"
    $aside = @(Get-ChildItem -LiteralPath $script:StateDir -Filter "state.broken-*.json")
    Assert-That ($aside.Count -eq 1) "the file was kept aside"
}

# ---------------------------------------------------------------- the session

New-Item -ItemType Directory -Force $Work | Out-Null
Remove-Item -LiteralPath $downloads, $script:StateDir -Recurse -Force -ErrorAction SilentlyContinue
try {
    $script:Proc = Start-Engine -Open
    $cues = @{
        Queue        = "more transfers than the limit go in; watch them wait and start."
        Priority     = "the last in line is set high; nothing running stops."
        CleanRestart = "keep transfers mid-way; you will press Ctrl+C in the engine's window."
        Kill         = "keep transfers mid-way; the engine is killed outright."
        Check        = "this waits for everything to finish, then checks every file."
        Resume       = "a large torrent is fetched to 30%, killed, and timed coming back."
        Damaged      = "no network needed: the saved list is overwritten with rubbish."
    }
    foreach ($row in $Rows) {
        Wait-Go $row $cues[$row]
        & "Test-$row"
        $script:Passed[$row] = $true
        Write-Output "== $row passed"
    }
}
finally {
    Stop-Engine $script:Proc
    Write-Output ""
    Write-Output "Passed: $(@($script:Passed.Keys) -join ', ')"
    Write-Output "Engine log: $(Hide $script:EngineLog)"
    if (-not $Keep) {
        Remove-Item -LiteralPath $downloads -Recurse -Force -ErrorAction SilentlyContinue
    }
}
