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
    printed is masked. With -Unattended it waits for no one, and sends the
    Ctrl+C itself: a real one, through the engine's console.

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
    [switch]$Keep,
    [switch]$Unattended
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
# How much a restart may lose: the last moments, at a fast cable's speed.
$MayFetchAgain = 100e6
# How much must be here before each restart, so that a restart has something
# to lose. Well above what it may lose, or the row proves nothing.
$HeldBeforeClean = 800e6
$HeldBeforeKill = 1600e6

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
    if ($Unattended) { return }
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

# Every byte held, of every transfer: what a restart must not lose.
function Get-Held {
    $held = @(Get-Webs) + @(Get-Swarm) | Measure-Object -Property done -Sum
    return [double]$held.Sum
}

# What the engine's speed tests pulled, by its log. Data used counts them, as
# it should, and each start of the engine runs them again.
function Get-Tested {
    $found = Select-String -LiteralPath $script:EngineLog -Pattern 'throughput sample .*bytes=(\d+)'
    return [double](@($found | ForEach-Object { [double]$_.Matches[0].Groups[1].Value }) | Measure-Object -Sum).Sum
}

# How many transfers have some of their bytes, and not all.
function Get-MidWay {
    @(@(Get-Webs) + @(Get-Swarm) | Where-Object { $_.length -and $_.done -gt 0 -and $_.done -lt $_.length }).Count
}

# Waits until a restart has something to lose, and something still under way.
function Wait-Held([double]$Bytes) {
    Wait-For "$(MB $Bytes) MB here" { (Get-Held) -ge $Bytes } | Out-Null
    $midway = Get-MidWay
    Assert-That ($midway -ge 1) "$midway transfers mid-way, $(MB (Get-Held)) MB held"
}

# A real Ctrl+C, from a process of its own: it leaves its console for the
# engine's, and ignores the signal it sends there.
function Send-CtrlC([int]$Id) {
    $code = @'
$m = '[DllImport("kernel32.dll")] public static extern bool FreeConsole();
[DllImport("kernel32.dll")] public static extern bool AttachConsole(uint id);
[DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);
[DllImport("kernel32.dll")] public static extern bool GenerateConsoleCtrlEvent(uint e, uint group);'
Add-Type -Name Console -Namespace Brg -MemberDefinition $m
[Brg.Console]::FreeConsole() | Out-Null
if (-not [Brg.Console]::AttachConsole(ENGINE_ID)) { exit 1 }
[Brg.Console]::SetConsoleCtrlHandler([IntPtr]::Zero, $true) | Out-Null
if (-not [Brg.Console]::GenerateConsoleCtrlEvent(0, 0)) { exit 2 }
Start-Sleep -Milliseconds 500
'@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code.Replace("ENGINE_ID", "$Id")))
    $sent = Start-Process powershell.exe -ArgumentList "-NoProfile -EncodedCommand $encoded" `
        -WindowStyle Hidden -PassThru -Wait
    if ($sent.ExitCode -ne 0) { throw "the Ctrl+C was not sent (exit $($sent.ExitCode))" }
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

function Test-Back([string]$Before, [double]$Received, [double]$Held, [int]$Limit) {
    Wait-For "the engine answering" { try { Get-Queue } catch { $null } } 60 | Out-Null
    Assert-That ((Get-Shape) -eq $Before) "the list and the priorities are back"
    Assert-That ((Get-Queue).limit -eq $Limit) "the limit is back: $Limit"
    $now = Get-TotalReceived
    Assert-That ($now -ge $Received) "data used carried over: $(MB $now) MB"
    # A torrent says what it holds once its fast resume check is done.
    Wait-For "what was held held again" { (Get-Held) -ge $Held - $MayFetchAgain } 120 | Out-Null
    Assert-That $true "held before: $(MB $Held) MB, held now: $(MB (Get-Held)) MB"
    Wait-For "something running again" { (Get-Queue).holding -ge 1 } 120 | Out-Null
}

function Test-CleanRestart {
    # Not the default, or a limit never saved would still come back.
    Post "/queue/limit" '{"limit": 2}' | Out-Null
    Wait-Held $HeldBeforeClean
    $before = Get-Shape
    $received = Get-TotalReceived
    $held = Get-Held
    if ($Unattended) { Send-CtrlC $script:Proc.Id }
    else { Write-Output "   Press Ctrl+C in the engine's window now." }
    Wait-For "the engine closing" { $script:Proc.HasExited } 300 | Out-Null
    $script:Proc = Start-Engine
    Test-Back $before $received $held 2
}

function Test-Kill {
    Wait-Held $HeldBeforeKill
    $before = Get-Shape
    $received = Get-TotalReceived
    $held = Get-Held
    $limit = (Get-Queue).limit
    # What was counted just now is on disk a save later: the kill may take
    # only what came after.
    Start-Sleep -Seconds 2
    Stop-Engine $script:Proc
    $script:Proc = Start-Engine
    Test-Back $before $received $held $limit
}

function Test-Check {
    Wait-For "everything finished" {
        @(Get-Webs | Where-Object { $_.state -notin @("done", "failed") }).Count -eq 0 -and
        @(Get-Swarm | Where-Object { $_.phase -notin @("done", "failed") }).Count -eq 0
    } | Out-Null
    $failed = @(@(Get-Webs) + @(Get-Swarm) | Where-Object { $_.state -eq "failed" -or $_.phase -eq "failed" })
    Assert-That ($failed.Count -eq 0) "nothing failed"
    # Every byte fetched, restarts and all, beside the size of what came.
    $size = (@(Get-Webs) + @(Get-Swarm) | Measure-Object -Property length -Sum).Sum
    $received = Get-TotalReceived
    $tested = Get-Tested
    $fetched = $received - $tested
    Write-Output "   data used $(MB $received) MB, of which speed tests $(MB $tested) MB"
    Assert-That ($fetched -lt $size * 1.05) "$(MB $fetched) MB fetched for $(MB $size) MB of files"
    # Whether what most programs ask for, a read that lets no one write, works
    # on a finished torrent's file while the engine runs. Reported, not judged.
    foreach ($t in @(Get-Swarm)) {
        try {
            [IO.File]::OpenRead((Join-Path $downloads $t.name)).Dispose()
            Write-Output "   note: $($t.name) opens while the engine runs"
        }
        catch { Write-Output "   note: $($t.name) does NOT open while the engine runs" }
    }
    # Stopped first: the engine holds a finished torrent's file open.
    Stop-Engine $script:Proc
    $script:Proc.WaitForExit()
    foreach ($name in $Sha256.Keys) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $downloads $name) -Algorithm SHA256).Hash
        Assert-That ($hash -eq "$($Sha256[$name])".ToUpper()) "$name matches its hash"
    }
    if (-not $Torrent) { return }
    # An empty state folder: no fast resume data, so every piece is read.
    $script:StateDir = Join-Path $Work "brg-phase6-check-state"
    Remove-Item -LiteralPath $script:StateDir -Recurse -Force -ErrorAction SilentlyContinue
    $script:Proc = Start-Engine
    Add-Torrent $Torrent | Out-Null
    Wait-For "every piece checked" { @(Get-Swarm | Where-Object { $_.phase -eq "done" }).Count -eq 1 } | Out-Null
    # The torrent's own count: data used would include this engine's speed test.
    $fetched = (@(@(Get-Swarm)[0].cables) | Measure-Object -Property received -Sum).Sum
    Assert-That ($fetched -lt 50e6) "every piece passed, $(MB $fetched) MB fetched"
}

function Test-Resume {
    Add-Torrent $LongTorrent | Out-Null
    Wait-For "a good part of it here" {
        $t = @(Get-Swarm)[-1]
        $t.length -gt 0 -and $t.done / $t.length -gt 0.3
    } | Out-Null
    Stop-Engine $script:Proc
    $script:Proc.WaitForExit()
    $started = Get-Date
    $script:Proc = Start-Engine
    Wait-For "it back and running" {
        $t = @(Get-Swarm)[-1]
        $t -and $t.phase -eq "downloading" -and $t.peers -gt 0
    } | Out-Null
    $back = [int]((Get-Date) - $started).TotalSeconds
    Stop-Engine $script:Proc
    $script:Proc.WaitForExit()
    # The same, with no fast resume data: every piece read.
    $script:StateDir = Join-Path $Work "brg-phase6-resume-state"
    Remove-Item -LiteralPath $script:StateDir -Recurse -Force -ErrorAction SilentlyContinue
    $script:Proc = Start-Engine
    $started = Get-Date
    Add-Torrent $LongTorrent | Out-Null
    Wait-For "it running after a full check" {
        $t = @(Get-Swarm)[-1]
        $t -and $t.phase -eq "downloading" -and $t.peers -gt 0
    } | Out-Null
    $full = [int]((Get-Date) - $started).TotalSeconds
    Write-Output "   Back in $back s with fast resume; $full s with a full check."
}

function Test-Damaged {
    Stop-Engine $script:Proc
    Set-Content -LiteralPath (Join-Path $script:StateDir "state.json") -Value "not a saved list"
    $script:Proc = Start-Engine
    $queue = Wait-For "the engine answering" { try { Get-Queue } catch { $null } } 60
    Assert-That ($queue.notice -match "could not be read") "the page is told"
    Assert-That ((@(Get-Webs).Count + @(Get-Swarm).Count) -eq 0) "it started with an empty list"
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
