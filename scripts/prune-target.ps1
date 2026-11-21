[CmdletBinding(SupportsShouldProcess)]
param(
    # Run `cargo clean` when target/ is still larger than this after trimming.
    [double]$MaxGB = 6,
    # Delete incremental caches for crates that have not been rebuilt in this many days.
    [int]$StaleDays = 14
)

# Weekly prune of target/ (registered by scripts/register-prune-task.ps1).
# 1. Incremental trim: keep only the newest session per crate and drop stale crates.
# 2. Size cap: `cargo clean` if target/ is still over -MaxGB.
# Skips the run while cargo/rustc or the app built in target/ is running.

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$target = Join-Path $repo 'target'
$logDir = Join-Path $env:LOCALAPPDATA 'league-analysis'
$log = Join-Path $logDir 'prune-target.log'

function Get-DirBytes([string]$Path) {
    $total = 0L
    if (-not (Test-Path -LiteralPath $Path)) { return $total }
    foreach ($f in [System.IO.Directory]::EnumerateFiles($Path, '*', 'AllDirectories')) {
        try { $total += (New-Object System.IO.FileInfo $f).Length } catch {}
    }
    $total
}

function Write-Log([string]$Message) {
    $line = '{0:yyyy-MM-dd HH:mm:ss} {1}' -f (Get-Date), $Message
    Write-Host $line
    if (-not $WhatIfPreference) {
        New-Item -ItemType Directory -Force -Path $logDir | Out-Null
        Add-Content -LiteralPath $log -Value $line -Encoding utf8
    }
}

if (-not (Test-Path -LiteralPath $target)) {
    Write-Log 'target/ missing; nothing to prune'
    return
}

$busy = @(Get-Process cargo, rustc -ErrorAction SilentlyContinue)
$busy += @(Get-Process draftly -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($target, [System.StringComparison]::OrdinalIgnoreCase) })
if ($busy.Count -gt 0) {
    Write-Log ('skipped: running {0}' -f (($busy | ForEach-Object Name | Sort-Object -Unique) -join ', '))
    return
}

$before = Get-DirBytes $target
$actions = @()
$cutoff = (Get-Date).AddDays(-$StaleDays)

# Step 1: incremental trim (target/<profile>/incremental/<crate-hash>/s-*).
foreach ($inc in Get-ChildItem -LiteralPath $target -Directory | ForEach-Object { Join-Path $_.FullName 'incremental' }) {
    if (-not (Test-Path -LiteralPath $inc)) { continue }
    foreach ($crate in Get-ChildItem -LiteralPath $inc -Directory) {
        $sessions = @(Get-ChildItem -LiteralPath $crate.FullName -Directory -Filter 's-*' | Sort-Object LastWriteTime -Descending)
        $newest = if ($sessions.Count -gt 0) { $sessions[0].LastWriteTime } else { $crate.LastWriteTime }
        if ($newest -lt $cutoff) {
            if ($PSCmdlet.ShouldProcess($crate.FullName, 'remove stale incremental cache')) {
                Remove-Item -LiteralPath $crate.FullName -Recurse -Force
            }
            $actions += "stale $($crate.Name)"
            continue
        }
        foreach ($old in $sessions | Select-Object -Skip 1) {
            if ($PSCmdlet.ShouldProcess($old.FullName, 'remove old incremental session')) {
                Remove-Item -LiteralPath $old.FullName -Recurse -Force
                $lock = "$($old.FullName).lock"
                if (Test-Path -LiteralPath $lock) { Remove-Item -LiteralPath $lock -Force }
            }
        }
        if ($sessions.Count -gt 1) { $actions += "trim $($crate.Name) x$($sessions.Count - 1)" }
    }
}

# Step 2: size cap.
$afterTrim = if ($WhatIfPreference) { $before } else { Get-DirBytes $target }
if ($afterTrim -gt $MaxGB * 1GB) {
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
    if ($PSCmdlet.ShouldProcess($target, "cargo clean (over $MaxGB GB)")) {
        & $cargo clean --manifest-path (Join-Path $repo 'Cargo.toml')
        if ($LASTEXITCODE -ne 0) { Write-Log "cargo clean failed ($LASTEXITCODE)"; exit $LASTEXITCODE }
    }
    $actions += "cargo clean (over $MaxGB GB)"
}

$after = if ($WhatIfPreference) { $before } else { Get-DirBytes $target }
$summary = if ($actions.Count -gt 0) { $actions -join '; ' } else { 'no action' }
$prefix = if ($WhatIfPreference) { 'whatif ' } else { '' }
Write-Log ('{0}target {1:N2} GB -> {2:N2} GB: {3}' -f $prefix, ($before / 1GB), ($after / 1GB), $summary)
