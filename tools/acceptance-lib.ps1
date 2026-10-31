<#
    BARRAGE acceptance helpers, shared by the phase 3 and phase 4 harnesses.
    Dot-source it from a harness's script scope: the functions read the
    harness's own settings from there ($Wifi, $Tether, $Url, $Work, $Port,
    $Engine, $api, $downloads, $timeline, $BudgetMB) and its session state
    ($script:Proc, $script:TetherName, $script:TetherStart, and with a log,
    $script:EngineLog). Every address they print is masked.
#>

# Any IPv4 or IPv6 address hidden, so nothing printed here names a network.
# An IPv6 address here has three colons at least, so a time such as
# 23:38:01 is left as it is.
function Hide([string]$Text) {
    $Text -replace '\b\d{1,3}(\.\d{1,3}){3}\b', 'x.x.x.x' -replace '\b[0-9a-fA-F]{0,4}(:[0-9a-fA-F]{0,4}){3,7}\b', 'x:x::x'
}

function MB([double]$Bytes) { [math]::Round($Bytes / 1e6, 1) }

# Bytes through an adapter both ways: what a phone's data counter counts. An
# adapter gone, its cable pulled, carries nothing more: its last count stands.
# One added again counts from nothing, so the old one's count is carried over.
$script:LastBytes = @{}
$script:BytesBefore = @{}
function Get-AdapterBytes([string]$Name) {
    $s = Get-NetAdapterStatistics -Name $Name -ErrorAction SilentlyContinue
    if ($s) {
        $now = [double]$s.ReceivedBytes + [double]$s.SentBytes
        if ($now -lt [double]$script:LastBytes[$Name]) {
            $script:BytesBefore[$Name] = [double]$script:BytesBefore[$Name] + [double]$script:LastBytes[$Name]
        }
        $script:LastBytes[$Name] = $now
    }
    return [double]$script:BytesBefore[$Name] + [double]$script:LastBytes[$Name]
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
function Test-Range([string]$Name, [string]$Address, [string]$From = $Url) {
    $headers = Join-Path $Work ("range-" + ($Name -replace '\W', '_') + ".txt")
    $out = & curl.exe -s -L -o NUL -D $headers --interface $Address -r 0-1023 --max-time 25 `
        -w "%{http_code} %{size_download}" $From
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

# The engine, in a window of its own, where its own words show. With -Open,
# its page opens in the browser on the Downloads tab.
function Start-Engine([switch]$Open) {
    if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
        throw "port $Port is taken: stop the barrage already running, or pass -Port"
    }
    New-Item -ItemType Directory -Force $downloads | Out-Null
    $arguments = @("--no-open", "--port", "$Port", "--downloads", "`"$downloads`"")
    # A session that counts requests reads them from the engine's log.
    if ($script:EngineLog) { $arguments += @("--log", "`"$($script:EngineLog)`"") }
    $proc = Start-Process -FilePath $Engine -PassThru -ArgumentList $arguments
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

function Add-Download([string[]]$Cables, [string]$From = $Url) {
    $body = @{ url = $From; cables = @($Cables) } | ConvertTo-Json -Compress
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
