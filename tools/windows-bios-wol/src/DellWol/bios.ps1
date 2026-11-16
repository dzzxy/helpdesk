$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$namespace = 'root/dcim/sysman/biosattributes'
$definitions = @(
    @{ Name = 'WakeOnLan'; Target = 'LanOnly' },
    @{ Name = 'DeepSleepCtrl'; Target = 'Disabled' }
)

function Stop-BiosOperation([string]$message) {
    $publicError = [InvalidOperationException]::new($message)
    $publicError.Data['DellWolPublic'] = $true
    throw $publicError
}
function Get-Snapshot {
    $board = Get-CimInstance Win32_BaseBoard
    if ($board.Manufacturer -notmatch '(?i)\bDell\b') { Stop-BiosOperation '此工具需要戴尔 BIOS WMI 接口。' }
    $system = Get-CimInstance Win32_ComputerSystem
    $bios = Get-CimInstance Win32_BIOS
    $uuid = (Get-CimInstance Win32_ComputerSystemProduct).UUID
    try {
        $attributes = @(Get-CimInstance -Namespace $namespace -ClassName EnumerationAttribute)
    } catch { Stop-BiosOperation '未发现戴尔 BIOS 属性接口，当前机型或 BIOS 暂不支持。' }
    $interface = $null
    try {
        $interfaceClass = Get-CimClass -Namespace $namespace -ClassName BIOSAttributeInterface
        if ($interfaceClass.CimClassMethods['SetAttribute']) {
            $instances = @(Get-CimInstance -Namespace $namespace -ClassName BIOSAttributeInterface | Where-Object Active)
            if ($instances.Count -eq 1) { $interface = $instances[0] }
        }
    } catch { }
    $auth = 'Unknown'
    try {
        $admin = @(Get-CimInstance -Namespace root/dcim/sysman/wmisecurity -ClassName PasswordObject |
            Where-Object { $_.NameId -eq 'Admin' -and $_.Active })
        if ($admin.Count -eq 1 -and $admin[0].IsPasswordSet -in @(0, 1)) {
            $auth = if ($admin[0].IsPasswordSet -eq 1) { 'Manual' } else { 'None' }
        }
    } catch { }
    $rows = foreach ($definition in $definitions) {
        $found = @($attributes | Where-Object AttributeName -ceq $definition.Name)
        if ($found.Count -gt 1) { Stop-BiosOperation "发现重复 BIOS 设置 $($definition.Name)，请检查固件。" }
        $attribute = if ($found.Count -eq 1) { $found[0] } else { $null }
        $current = if ($attribute) { [string]$attribute.CurrentValue } else { 'Unavailable' }
        $choices = if ($attribute) { @($attribute.PossibleValue | Select-Object -Unique) } else { @() }
        foreach ($value in @($current) + $choices) {
            if ($value -notmatch '^[A-Za-z0-9 _()./-]+$') { Stop-BiosOperation '固件返回了无法识别的 BIOS 设置值。' }
        }
        # Older BIOS may leave ReadOnly unset. The SetAttribute result remains authoritative.
        $writable = $null -ne $interface -and $auth -ne 'Unknown' -and $null -ne $attribute -and
            $attribute.ReadOnly -ne $true -and $choices.Count -gt 1 -and $current -cin $choices
        [pscustomobject]@{
            Name = $definition.Name
            Current = $current
            Choices = @($choices)
            Target = $(if ($definition.Target -cin $choices) { $definition.Target } else { 'Unavailable' })
            Writable = $writable
        }
    }
    $identity = @($uuid, $board.Product, $bios.SMBIOSBIOSVersion, $auth, ($null -ne $interface)) -join '|'
    $payload = $identity + ($rows | ConvertTo-Json -Compress -Depth 4)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { $token = ([BitConverter]::ToString($hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($payload)))).Replace('-', '') }
    finally { $hasher.Dispose() }
    [pscustomobject]@{
        Product = ('DELL ' + $board.Product + ' / ' + $system.Model) -replace '[|\r\n]', ' '
        Bios = ([string]$bios.SMBIOSBIOSVersion) -replace '[|\r\n]', ' '
        Token = $token
        Auth = $auth
        Interface = $interface
        Rows = @($rows)
    }
}

function Write-Preview($snapshot, [string]$message) {
    "BOARD|$($snapshot.Product)|$($snapshot.Bios)"
    "TOKEN|$($snapshot.Token)"
    "AUTH|$($snapshot.Auth)"
    'STATUS|' + [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($message))
    foreach ($row in $snapshot.Rows) {
        $editable = if ($row.Writable) { '1' } else { '0' }
        "ROW|$($row.Name)|$($row.Current)|$($row.Target)|$($row.Choices -join ',')|$editable"
    }
}

$lock = $null
$locked = $false
$attempted = $false
$completed = @()
try {
    $operation = [Console]::ReadLine()
    $expectedToken = [Console]::ReadLine()
    $password = [Console]::ReadLine()
    $targets = @([Console]::ReadLine(), [Console]::ReadLine())
    if ($operation -cnotin @('read', 'apply')) { Stop-BiosOperation '不支持的操作。' }
    if ($operation -ceq 'apply') {
        $lock = [Threading.Mutex]::new($false, 'Global\DellWolBiosWrite')
        try { $locked = $lock.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
        if (!$locked) { Stop-BiosOperation '另一个戴尔工具正在保存，请稍后刷新。' }
    }
    $snapshot = Get-Snapshot
    $authMessage = switch ($snapshot.Auth) {
        'None' { 'BIOS 未设置管理员密码。' }
        'Manual' { '修改后请手动输入 BIOS 管理员密码。' }
        default { '无法确认 BIOS 密码状态，当前仅可查看。' }
    }
    if ($operation -ceq 'read') {
        $message = if (!$snapshot.Interface) { '未检测到 BIOS 写入接口，当前仅可查看。' }
            elseif (!($snapshot.Rows | Where-Object Writable)) { '未检测到可编辑的 WoL 设置。' }
            else { '请选择要修改的值，再点击保存。' }
        Write-Preview $snapshot "$message $authMessage"
        exit 0
    }
    if ($expectedToken -cne $snapshot.Token) { Stop-BiosOperation 'BIOS 状态已变化，请刷新后重新选择。' }
    $changes = @()
    for ($index = 0; $index -lt $snapshot.Rows.Count; $index++) {
        $row = $snapshot.Rows[$index]
        $target = $targets[$index]
        if ([string]::IsNullOrEmpty($target)) { continue }
        if ($target -cnotin $row.Choices) { Stop-BiosOperation "$($row.Name) 的目标值未由固件提供，请刷新。" }
        if ($target -ceq $row.Current) { continue }
        if (!$row.Writable) { Stop-BiosOperation "$($row.Name) 当前不可写，请刷新。" }
        $changes += [pscustomobject]@{ Name = $row.Name; Old = $row.Current; Target = $target }
    }
    if (!$changes.Count) {
        Write-Preview $snapshot "当前没有需要写入的变更。 $authMessage"
        exit 0
    }
    $securityType = [uint32]0
    $securityBytes = [byte[]]@(0)
    $securityCount = [uint32]0
    if ($snapshot.Auth -ceq 'Manual') {
        if ([string]::IsNullOrEmpty($password)) { Stop-BiosOperation '请输入 BIOS 管理员密码。' }
        $securityType = [uint32]1
        $securityBytes = [Text.Encoding]::UTF8.GetBytes($password)
        $securityCount = [uint32]$securityBytes.Length
    }
    $password = $null
    foreach ($change in $changes) {
        # Recheck each attribute immediately before writing, including after earlier writes.
        $fresh = @(Get-CimInstance -Namespace $namespace -ClassName EnumerationAttribute -Filter "AttributeName='$($change.Name)'")
        if ($fresh.Count -ne 1 -or $fresh[0].CurrentValue -cne $change.Old) {
            Stop-BiosOperation "$($change.Name) 已发生变化，请刷新后核对。"
        }
        $attempted = $true
        try {
            $result = Invoke-CimMethod -InputObject $snapshot.Interface -MethodName SetAttribute -Arguments @{
                SecType = $securityType; SecHndCount = $securityCount; SecHandle = $securityBytes
                AttributeName = $change.Name; AttributeValue = $change.Target
            }
        } catch { Stop-BiosOperation "$($change.Name) 写入结果无法确认，请刷新核对。" }
        if ($null -eq $result.Status -or $result.Status -ne 0) {
            $reason = switch ($result.Status) {
                1 { '固件执行失败' }
                2 { '参数无效' }
                3 { '访问被拒绝，请核对管理员权限和 BIOS 密码' }
                4 { '固件不支持此操作' }
                5 { '固件内存错误' }
                6 { '固件协议错误' }
                default { '固件返回未知状态' }
            }
            Stop-BiosOperation "$($change.Name) 保存失败，$reason。"
        }
        $completed += "$($change.Name)=$($change.Target)"
        try { $verified = Get-CimInstance -Namespace $namespace -ClassName EnumerationAttribute -Filter "AttributeName='$($change.Name)'" }
        catch { Stop-BiosOperation "$($change.Name) 已接受写入，但回读失败，请刷新核对。" }
        if ($verified.CurrentValue -cne $change.Target) {
            $snapshot = Get-Snapshot
            Write-Preview $snapshot "固件已接受 $($completed -join '，')，回读暂未一致。请重启后刷新核对，其余选项请重新选择。"
            exit 0
        }
    }
    $snapshot = Get-Snapshot
    Write-Preview $snapshot "已保存并回读确认 $($completed -join '，')。请自行重启，并验证网络唤醒。"
} catch {
    $reason = $_.Exception.Message
    # Only messages authored in this script are suitable for display. Provider errors may contain credentials.
    if (!$_.Exception.Data['DellWolPublic'] -or $reason.Length -gt 250 -or $reason -match '[|\r\n]') {
        $reason = '无法完成 BIOS 操作，请以 Windows 管理员身份运行并检查戴尔 WMI 接口。'
    }
    if ($attempted) {
        $reason = "保存未全部确认，已接受 $($completed -join '，')。$reason 请刷新核对当前值。"
    }
    "ERROR|$reason"
    exit 1
} finally {
    if ($securityBytes) { [Array]::Clear($securityBytes, 0, $securityBytes.Length) }
    if ($locked) { $lock.ReleaseMutex() }
    if ($lock) { $lock.Dispose() }
}
