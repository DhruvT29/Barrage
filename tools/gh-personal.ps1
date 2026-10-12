<#
  gh-personal.ps1 - run the GitHub CLI against PERSONAL repositories only.

  WHY THIS EXISTS
    Two GitHub identities live on this machine. Work repos (keystride, emerja)
    are reached over SSH through the `github.com-work` alias in ~/.ssh/config
    with a separate key. Personal repos are reached over HTTPS.

    `gh auth login` would store one account-wide token in %APPDATA% and make gh
    a global git credential helper, leaving "don't touch work repos" as nothing
    more than a convention. Instead this script:

      1. Reads a FINE-GRAINED token from .github.env in the repo root, a file
         that lives inside Codes\personal and is gitignored. The token itself
         is restricted by GitHub to selected repositories, so the boundary is
         enforced server-side and survives any mistake made here.
      2. Refuses to run if `origin` is not owned by an allow-listed personal
         owner, or if the remote uses the work SSH alias, AND refuses if any
         argument names a repository outside the allow-list, whether through
         `--repo`/`-R` or an API path such as `repos/owner/name`.

         Be precise about what this does and does not buy. It is a local
         sanity check that makes a wrong-repo call fail loudly and instantly
         instead of returning a confusing 404. **It is not a sandbox.** gh has
         many ways to name a target and this inspects the obvious ones, so the
         boundary that actually holds is the token's repository scope, which
         GitHub enforces server-side. Treat this check as a guard rail, never
         as the reason it is safe.

         The first version of this script checked only `origin`, which was
         close to useless: with `origin` pointing at one repository,
         `gh api repos/<owner>/<other-repo>` read a different one and returned
         HTTP 200. The header claimed protection the code did not provide,
         which is worse than having no check at all.
      3. Passes the token via GH_TOKEN for this process only. Nothing is
         written to %APPDATA%, gh never becomes a credential helper, and work
         repos are left exactly as they were.

  SETUP (one time, done by the user, the token must never be pasted into chat)
    1. https://github.com/settings/personal-access-tokens/new
    2. Token name: barrage-claude   Expiration: 90 days
    3. Resource owner: your personal account
    4. Repository access: "Only select repositories" -> pick Barrage
    5. Permissions -> Repository permissions:
         Contents       Read and write    (push branches)
         Pull requests  Read and write    (open and update PRs)
         Metadata       Read-only         (added automatically)
       Everything else: No access. Do NOT grant Account permissions.
    6. Generate, copy, then create .github.env in the repo root:

         GITHUB_TOKEN=github_pat_xxxxxxxxxxxxxxxxxxxx
         GITHUB_OWNERS=DhruvT29

       GITHUB_OWNERS is a comma-separated allow-list of personal account or
       org names. A BOM is tolerated, so it does not matter how you create it.

  USAGE
    .\tools\gh-personal.ps1 -Verify
    .\tools\gh-personal.ps1 auth status
    .\tools\gh-personal.ps1 pr create --title "..." --body-file .\pr.md --base main
    .\tools\gh-personal.ps1 pr list

    If PowerShell mis-parses a flag, pass the arguments as an explicit array:
    .\tools\gh-personal.ps1 -Gh @("pr","view","3","--json","state,url")
#>
[CmdletBinding()]
param(
    [switch]$Verify,
    [string[]]$Gh,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------- locate gh

$ghExe = Join-Path ${env:ProgramFiles} "GitHub CLI\gh.exe"
if (-not (Test-Path $ghExe)) {
    $cmd = Get-Command gh -ErrorAction SilentlyContinue
    if ($null -ne $cmd) { $ghExe = $cmd.Source }
}
if (-not (Test-Path $ghExe)) {
    throw "gh not found. Run: winget install GitHub.cli"
}

# ---------------------------------------------------------------- load config

$repoRoot = Split-Path -Parent $PSScriptRoot
$envFile  = Join-Path $repoRoot ".github.env"

if (-not (Test-Path -LiteralPath $envFile)) {
    throw @"
.github.env not found at $envFile
See the SETUP block at the top of this script. It needs two lines:
  GITHUB_TOKEN=github_pat_...
  GITHUB_OWNERS=DhruvT29
"@
}

$cfg = @{}
foreach ($line in (Get-Content -LiteralPath $envFile)) {
    # Strip a BOM and ignore comments and blanks.
    $l = [string]$line -replace "^\xEF\xBB\xBF", ""
    if ($l -match '^\s*#') { continue }
    if ($l -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+?)\s*$') {
        $cfg[$matches[1]] = $matches[2]
    }
}

$token = $cfg['GITHUB_TOKEN']
if (-not $token) { throw "GITHUB_TOKEN missing from $envFile" }

$owners = @()
if ($cfg['GITHUB_OWNERS']) {
    $owners = @($cfg['GITHUB_OWNERS'].Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
if ($owners.Count -eq 0) { throw "GITHUB_OWNERS missing from $envFile. Refusing to run without an allow-list." }

# ---------------------------------------------------------------- guard rail

$originUrl = & git -C $repoRoot remote get-url origin 2>$null
if (-not $originUrl) { throw "No 'origin' remote on $repoRoot. Nothing to open a PR against." }
$originUrl = [string]$originUrl

# The work identity is an SSH host alias, never a personal HTTPS remote.
if ($originUrl -match 'github\.com-work') {
    throw "REFUSED: origin '$originUrl' uses the work SSH alias. This script is for personal repositories only."
}

$owner = $null
$repo  = $null
if ($originUrl -match '^https://github\.com/([^/]+)/([^/]+?)(\.git)?/?$') {
    $owner = $matches[1]; $repo = $matches[2]
}
elseif ($originUrl -match '^git@github\.com:([^/]+)/([^/]+?)(\.git)?/?$') {
    $owner = $matches[1]; $repo = $matches[2]
}
else {
    throw "REFUSED: cannot parse owner out of origin '$originUrl'. Not running gh against an unrecognised remote."
}

$allowed = $false
foreach ($o in $owners) { if ($o -eq $owner) { $allowed = $true } }
if (-not $allowed) {
    throw "REFUSED: origin owner '$owner' is not in GITHUB_OWNERS ($($owners -join ', ')). This looks like a work or third-party repo."
}

# ---------------------------------------------------------------- invoke

# This process only. Nothing is persisted to %APPDATA% and gh does not become
# a git credential helper, so work repositories are untouched.
$env:GH_TOKEN = $token
$env:GH_PROMPT_DISABLED = "1"
$env:GH_NO_UPDATE_NOTIFIER = "1"

if ($Verify) {
    Write-Output "repo root    $repoRoot"
    Write-Output "origin       $originUrl"
    Write-Output "owner/repo   $owner/$repo  (allowed: $($owners -join ', '))"
    Write-Output "token        [$($token.Length) chars] from .github.env"
    Write-Output ""
    # jq expressions with string interpolation do not survive PowerShell's
    # quoting, so parse the JSON here instead of asking gh to format it.
    Write-Output "---- authenticated as ----"
    $me = & $ghExe api user | ConvertFrom-Json
    Write-Output ("  $($me.login)")

    Write-Output "---- the repo this script is pointed at ----"
    $info = & $ghExe api "repos/$owner/$repo" | ConvertFrom-Json
    Write-Output ("  {0}  private={1}  default_branch={2}" -f $info.full_name, $info.private, $info.default_branch)

    Write-Output "---- EVERY repo this token can reach (the scope proof) ----"
    $all = & $ghExe api "user/repos?per_page=100" | ConvertFrom-Json
    foreach ($r in $all) { Write-Output ("  {0}" -f $r.full_name) }
    Write-Output ("  total: {0}" -f @($all).Count)
    Write-Output ""
    Write-Output "A fine-grained token restricted to selected repositories lists only those"
    Write-Output "repositories above and returns 404 for anything else, so the boundary is"
    Write-Output "enforced by GitHub rather than by this script's good behaviour."
    return
}

$argv = @()
if ($Gh)   { $argv += $Gh }
if ($Rest) { $argv += $Rest }
if ($argv.Count -eq 0) {
    throw "Nothing to run. Try -Verify, or pass gh arguments, e.g. pr list"
}

function Get-NamedOwners {
    <#
      Every repository owner named anywhere in the arguments. Checking only
      `origin` was the original bug: a command can name its own target and
      bypass the remote entirely.
    #>
    param([string[]]$Argv)

    $owners = @()
    for ($i = 0; $i -lt $Argv.Count; $i++) {
        $a = [string]$Argv[$i]

        # --repo owner/name   or   -R owner/name
        if (($a -eq "--repo" -or $a -eq "-R") -and ($i + 1) -lt $Argv.Count) {
            $owners += [string]$Argv[$i + 1]
            continue
        }
        # --repo=owner/name
        if ($a -like "--repo=*") {
            $owners += $a.Substring("--repo=".Length)
            continue
        }
        # REST paths: repos/owner/name, and the same inside a full API URL.
        $m = [regex]::Match($a, '(?:^|/)repos/([^/\s]+)/([^/\s?]+)')
        if ($m.Success) {
            $owners += "$($m.Groups[1].Value)/$($m.Groups[2].Value)"
            continue
        }
        # A github.com URL used as a repo argument.
        $u = [regex]::Match($a, '^(?:https://|git@)github\.com[:/]([^/\s]+)/([^/\s]+?)(?:\.git)?/?$')
        if ($u.Success) {
            $owners += "$($u.Groups[1].Value)/$($u.Groups[2].Value)"
        }
    }
    return $owners
}

foreach ($named in (Get-NamedOwners -Argv $argv)) {
    if ($named -match 'github\.com-work') {
        throw "REFUSED: argument '$named' references the work SSH alias."
    }
    $parts = $named.Split("/")
    if ($parts.Count -lt 2 -or -not $parts[0]) {
        throw "REFUSED: cannot determine the owner of '$named'. Name repositories as owner/name so the allow-list can be applied."
    }
    $namedOwner = $parts[0]
    $okNamed = $false
    foreach ($o in $owners) { if ($o -eq $namedOwner) { $okNamed = $true } }
    if (-not $okNamed) {
        throw "REFUSED: the arguments name '$named', whose owner '$namedOwner' is not in GITHUB_OWNERS ($($owners -join ', ')). This script only operates on allow-listed personal repositories."
    }
}

Write-Output "gh $($argv -join ' ')   [$owner/$repo]"
& $ghExe @argv
exit $LASTEXITCODE
