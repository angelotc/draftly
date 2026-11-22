[CmdletBinding()]
param(
    # Remove the scheduled task instead of registering it.
    [switch]$Unregister
)

# Registers a weekly Windows scheduled task (Sunday 03:00, current user) that runs
# scripts/prune-target.ps1. Re-running replaces the task.

$ErrorActionPreference = 'Stop'
$name = 'league-analysis prune target'

if ($Unregister) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false
    Write-Host "Removed scheduled task '$name'."
    return
}

$script = Join-Path $PSScriptRoot 'prune-target.ps1'
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$script`"" `
    -WorkingDirectory (Split-Path -Parent $PSScriptRoot)
$trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Sunday -At 3am
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -DontStopIfGoingOnBatteries -AllowStartIfOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 30)

Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Settings $settings `
    -Description 'Trim league-analysis target/ incremental caches; cargo clean when over the size cap.' -Force | Out-Null
Write-Host "Registered scheduled task '$name' (weekly, Sunday 03:00)."
