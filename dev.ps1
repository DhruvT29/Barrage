<#
    BARRAGE, everything at once: the engine (which is also the backend: it
    serves the API and the live socket on 127.0.0.1:7700) and the page, with
    one command from the repository's root. Ctrl+C stops both.

    USAGE
      .\dev.ps1             The engine, and the page with live reload: edits
                            under ui/src show at once. Opens
                            http://127.0.0.1:5173, which passes /api to the
                            engine.
      .\dev.ps1 -App        The page built and served by the engine itself,
                            exactly as `barrage` ships it. Opens
                            http://127.0.0.1:7700.
      .\dev.ps1 -Preview    The page alone, fed made-up data (preview.html):
                            no engine, nothing touches the network. Opens
                            http://127.0.0.1:5173/preview.html.

    OPTIONS
      -Release              Build the engine optimised (slower to build,
                            faster downloads).
      -Downloads DIR        Where downloads go (it must exist); otherwise the
                            Downloads folder.
      -Log FILE             The engine records every change in FILE.
      -NoOpen               Do not open the browser.

    The engine watches this machine's real connections from the moment it
    starts: a tethered phone is speed-tested once (30 to 60 MB of its data).
    -Preview is the way to look at the page without that.

    The first run installs the page's packages and compiles the engine, which
    takes a few minutes; later runs only rebuild what changed.
#>
[CmdletBinding()]
param(
    [switch]$App,
    [switch]$Preview,
    [switch]$Release,
    [string]$Downloads,
    [string]$Log,
    [switch]$NoOpen
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = $PSScriptRoot
$Ui = Join-Path $Root 'ui'
$Engine = Join-Path $Root 'engine'
$EnginePort = 7700
$PagePort = 5173

function Say([string]$text) { Write-Host "[dev] $text" -ForegroundColor Green }
function Fail([string]$text) { Write-Host "[dev] $text" -ForegroundColor Red; exit 1 }

function Need([string]$command, [string]$why) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) { Fail "$command is not installed: $why" }
}

# Whether something on this machine already answers on the port.
function Test-Port([int]$port) {
    $client = New-Object Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1', $port)
        return $true
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

# Waits until the port answers, or the process that should open it ends.
function Wait-Port([int]$port, [Diagnostics.Process]$process, [string]$name, [int]$seconds = 60) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while (-not (Test-Port $port)) {
        if ($process.HasExited) { Fail "$name stopped before it was ready (exit code $($process.ExitCode))." }
        if ((Get-Date) -gt $deadline) { Fail "$name did not answer on port $port within $seconds s." }
        Start-Sleep -Milliseconds 250
    }
}

function Assert-Free([int]$port, [string]$what) {
    if (Test-Port $port) {
        Fail "port $port is already in use: is $what already running (another .\dev.ps1, barrage or pnpm dev)? Stop it first."
    }
}

# Stops a process and everything it started (cmd -> pnpm -> node).
function Stop-Tree([Diagnostics.Process]$process) {
    if ($null -ne $process -and -not $process.HasExited) {
        & taskkill.exe /T /F /PID $process.Id 2>&1 | Out-Null
    }
}

# Runs a tool with its output on the screen and gives its exit code. Its
# stderr is progress, not an error: Windows PowerShell would otherwise stop
# the script on the first line cargo or pnpm writes there.
function Invoke-Native([string]$command, [string[]]$arguments) {
    $ErrorActionPreference = 'Continue'
    & $command @arguments 2>&1 | ForEach-Object { Write-Host $_ }
    return $LASTEXITCODE
}

# Installs the page's packages when they are missing or older than the lockfile.
function Install-Page {
    $stamp = Join-Path $Ui 'node_modules\.modules.yaml'
    $lock = Join-Path $Ui 'pnpm-lock.yaml'
    if (-not (Test-Path $stamp) -or (Get-Item $lock).LastWriteTime -gt (Get-Item $stamp).LastWriteTime) {
        Say 'installing the page''s packages...'
        if ((Invoke-Native 'pnpm' @('--dir', $Ui, 'install')) -ne 0) { Fail 'pnpm install failed.' }
    }
}

# Builds the engine and gives the path of barrage.exe.
function Build-Engine {
    $build = if ($Release) { 'release' } else { 'debug' }
    Say "building the engine ($build)..."
    $cargoArgs = @('build', '--manifest-path', (Join-Path $Engine 'Cargo.toml'), '--bin', 'barrage')
    if ($Release) { $cargoArgs += '--release' }
    if ((Invoke-Native 'cargo' $cargoArgs) -ne 0) { Fail 'the engine did not build.' }
    return Join-Path $Engine "target\$build\barrage.exe"
}

function Start-Engine([string]$exe) {
    $engineArgs = @('--no-open', '--port', "$EnginePort")
    if ($Downloads) { $engineArgs += @('--downloads', "`"$((Resolve-Path $Downloads).Path)`"") }
    if ($Log) { $engineArgs += @('--log', "`"$($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Log))`"") }
    Say 'starting the engine...'
    $process = Start-Process -FilePath $exe -ArgumentList $engineArgs -WorkingDirectory $Engine -NoNewWindow -PassThru
    # Windows PowerShell keeps a process's exit code only once its handle has been read.
    $null = $process.Handle
    Wait-Port $EnginePort $process 'the engine'
    Say "engine ready on http://127.0.0.1:$EnginePort"
    return $process
}

function Start-Page {
    Say 'starting the page with live reload...'
    $process = Start-Process -FilePath 'cmd.exe' `
        -ArgumentList '/c', 'pnpm', 'exec', 'vite', '--port', "$PagePort", '--strictPort', '--host', '127.0.0.1' `
        -WorkingDirectory $Ui -NoNewWindow -PassThru
    $null = $process.Handle
    Wait-Port $PagePort $process 'the page''s dev server'
    return $process
}

if ($App -and $Preview) { Fail 'choose -App or -Preview, not both.' }

Need 'pnpm' 'install Node, then: npm install -g pnpm'
if (-not $Preview) { Need 'cargo' 'install Rust from https://rustup.rs' }

if (-not $Preview) { Assert-Free $EnginePort 'the engine' }
if (-not $App) { Assert-Free $PagePort 'the page''s dev server' }

$engineProcess = $null
$pageProcess = $null
try {
    Install-Page

    if ($App) {
        Say 'building the page...'
        if ((Invoke-Native 'pnpm' @('--dir', $Ui, 'build')) -ne 0) { Fail 'the page did not build.' }
    }

    if (-not $Preview) {
        $exe = Build-Engine
        $engineProcess = Start-Engine $exe
    }
    if (-not $App) {
        $pageProcess = Start-Page
    }

    $url = if ($App) { "http://127.0.0.1:$EnginePort" }
    elseif ($Preview) { "http://127.0.0.1:$PagePort/preview.html" }
    else { "http://127.0.0.1:$PagePort" }
    Say "open $url    (Ctrl+C stops everything)"
    if (-not $NoOpen) { Start-Process $url }

    # Until either one ends: the other is no use alone.
    $running = @($engineProcess, $pageProcess) | Where-Object { $null -ne $_ }
    while (-not ($running | Where-Object { $_.HasExited })) { Start-Sleep -Milliseconds 500 }
    $ended = $running | Where-Object { $_.HasExited } | Select-Object -First 1
    $name = if ($ended -eq $engineProcess) { 'the engine' } else { 'the page''s dev server' }
    Write-Host "[dev] $name stopped (exit code $($ended.ExitCode)); stopping the rest." -ForegroundColor Yellow
} finally {
    # Ctrl+C reaches every process in this window, so the engine has already
    # been asked to stop and saves its downloads' progress; give it a moment
    # before anything left over is ended outright.
    if ($null -ne $engineProcess -and -not $engineProcess.HasExited) { $null = $engineProcess.WaitForExit(3000) }
    Stop-Tree $pageProcess
    Stop-Tree $engineProcess
    Say 'stopped.'
}
