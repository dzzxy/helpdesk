$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$result = [ordered]@{
    taskId = '__TASK_ID__'
    success = $false
    changed = $false
    enabledRuleCount = 0
    runningServices = @()
    stage = 'Administrator'
    message = ''
}
try {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrator token required.' }
    $result.stage = 'DomainNetwork'
    if (-not (Get-NetConnectionProfile | Where-Object NetworkCategory -eq 'DomainAuthenticated')) {
        throw 'No active domain network. No changes made.'
    }
    $result.stage = 'Rules'
    $rules = @(Get-NetFirewallRule -Name 'FPS-*','NETDIS-*' | Where-Object { $_.Profile.ToString() -eq 'Domain' })
    if ($rules.Count -eq 0) { throw 'No matching domain rules. No changes made.' }
    $services = @(Get-Service -Name 'LanmanServer','fdPHost','FDResPub')
    $result.changed = $true
    $rules | Enable-NetFirewallRule
    $result.enabledRuleCount = $rules.Count
    $result.stage = 'Services'
    foreach ($service in $services) {
        if ($service.StartType -eq 'Disabled') { Set-Service -Name $service.Name -StartupType Manual }
        Start-Service -Name $service.Name
    }
    $result.stage = 'VerifyServices'
    $result.runningServices = @(Get-Service -Name 'LanmanServer','fdPHost','FDResPub' |
        Where-Object Status -eq 'Running' | ForEach-Object Name)
    if ($result.runningServices.Count -ne 3) { throw 'Required services are not all running.' }
    $result.success = $true
    $result.stage = 'Completed'
    $result.message = 'Domain sharing rules and services are ready.'
} catch { $result.message = $_.Exception.Message }
Write-Output ('CORP_CONNECTION_REPAIR:' + ($result | ConvertTo-Json -Compress -Depth 4))
