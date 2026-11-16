$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Read-WakeOnLan {
    $settings = @(Get-CimInstance -Namespace root/wmi -ClassName Lenovo_BiosSetting |
        Where-Object CurrentSetting -match '^Wake[ -]*On[ -]*LAN,')
    if ($settings.Count -ne 1 -or $settings[0].CurrentSetting -notmatch '^([^,]+),([^;|\r\n]+)(?:;(.*))?$') {
        throw 'UNSUPPORTED_SETTING'
    }
    [pscustomobject]@{
        Name = $Matches[1]
        Value = $Matches[2]
        Details = $Matches[3]
        ReadOnly = $settings[0].IsReadOnly -eq $true
    }
}

function Get-WakeOnLanChoices($Setting) {
    $selection = $null
    try {
        $provider = Get-CimInstance -Namespace root/wmi -ClassName Lenovo_GetBiosSelections
        $result = Invoke-CimMethod -InputObject $provider -MethodName GetBiosSelections -Arguments @{ Item = $Setting.Name }
        $selection = $result.Selections
    } catch {
        # Older ThinkCentre models include their choices in CurrentSetting.
    }
    if (!$selection -and $Setting.Details -match '\[Optional:([^\]]+)\]') {
        $selection = $Matches[1]
    }
    $selection -split ',' | ForEach-Object { $_.Trim() } |
        Where-Object { $_ -match '^[A-Za-z0-9][A-Za-z0-9 _()./-]*$' } |
        Select-Object -Unique
}

function Test-BiosMethod($Class, $Method) {
    $definition = Get-CimClass -Namespace root/wmi -ClassName $Class -ErrorAction SilentlyContinue
    $null -ne $definition -and $null -ne $definition.CimClassMethods[$Method]
}

function Invoke-BiosMethod($Class, $Method, $Parameter) {
    $instance = Get-CimInstance -Namespace root/wmi -ClassName $Class
    $arguments = @{}
    if ($null -ne $Parameter) { $arguments.parameter = $Parameter }
    (Invoke-CimMethod -InputObject $instance -MethodName $Method -Arguments $arguments).return
}

$staged = $false
$stage = 'READ'
try {
    $board = Get-CimInstance Win32_BaseBoard
    if ($board.Manufacturer -ine 'LENOVO') { throw 'WRONG_DEVICE' }
    $boardModel = ([string]$board.Product).ToUpperInvariant()
    if ($boardModel -notmatch '^[A-Z0-9][A-Z0-9 _.\-]*$') { $boardModel = 'Unknown' }
    $setting = Read-WakeOnLan
    $current = $setting.Value
    $choices = @(Get-WakeOnLanChoices $setting)
    $passwordState = (Get-CimInstance -Namespace root/wmi -ClassName Lenovo_BiosPasswordSettings -ErrorAction SilentlyContinue).PasswordState
    $needsPassword = $null -ne $passwordState -and ($passwordState -band 2) -ne 0
    $canAuthenticate = $needsPassword -and (Test-BiosMethod Lenovo_WmiOpcodeInterface WmiOpcodeInterface)
    $availability = 'WRITABLE'
    if ($setting.ReadOnly -or !(Test-BiosMethod Lenovo_SetBiosSetting SetBiosSetting) -or
        !(Test-BiosMethod Lenovo_SaveBiosSettings SaveBiosSettings)) {
        $availability = 'NO_WRITE_INTERFACE'
    } elseif ($choices.Count -lt 2 -or $current -cnotin $choices) {
        $availability = 'NO_CHOICES'
    } elseif ($null -eq $passwordState -or ($passwordState -band 128) -or
        (!$needsPassword -and ($passwordState -band 64))) {
        $availability = 'AUTH_METHOD'
    } elseif ($needsPassword -and !$canAuthenticate) {
        $availability = 'AUTH_METHOD'
    }
    $options = $choices -join ','
    $state = "${current}|${needsPassword}|${boardModel}|${canAuthenticate}|${availability}|$($setting.Name)|$options"
    $mode = [Console]::ReadLine()
    if ($mode -ceq 'read') {
        "OK|$state"
        exit 0
    }
    $password = [Console]::ReadLine()
    $expected = [Console]::ReadLine()
    if (![string]::IsNullOrEmpty($expected) -and $expected -cne $state) {
        throw 'STATE_CHANGED'
    }
    if ($availability -cne 'WRITABLE') { throw $availability }
    if ($mode -cnotin $choices) { throw 'INVALID_MODE' }
    if ($mode -ceq $current) {
        "UNCHANGED|$state"
        exit 0
    }
    if ($needsPassword -and (!$canAuthenticate -or [string]::IsNullOrEmpty($password) -or $password.Contains(';'))) {
        throw 'INVALID_PASSWORD_INPUT'
    }
    if ($needsPassword) {
        # Some ThinkCentre firmware rejects SetBiosSetting until the password opcode has run.
        $stage = 'AUTH'
        $reply = Invoke-BiosMethod Lenovo_WmiOpcodeInterface WmiOpcodeInterface "WmiOpcodePasswordAdmin:${password};"
        if ($reply -ne 'Success') { throw 'AUTH_FAILED' }
    }
    $stage = 'SET'
    $request = "$($setting.Name),$mode"
    $saveParameter = $null
    $reply = Invoke-BiosMethod Lenovo_SetBiosSetting SetBiosSetting $request
    if ($reply -eq 'Invalid Parameter') {
        # Lenovo's desktop deployment guide also uses semicolon-terminated requests.
        $reply = Invoke-BiosMethod Lenovo_SetBiosSetting SetBiosSetting "$request;"
        $saveParameter = ';'
    }
    if ($reply -ne 'Success') { throw 'SET_FAILED' }
    $staged = $true
    $stage = 'SAVE'
    $reply = Invoke-BiosMethod Lenovo_SaveBiosSettings SaveBiosSettings $saveParameter
    if ($reply -eq 'Invalid Parameter' -and $null -eq $saveParameter) {
        $reply = Invoke-BiosMethod Lenovo_SaveBiosSettings SaveBiosSettings ';'
    }
    if ($reply -ne 'Success') { throw 'SAVE_FAILED' }
    $staged = $false
    $stage = 'VERIFY'
    $verified = Read-WakeOnLan
    if ($verified.Name -cne $setting.Name) { throw 'VERIFY_FAILED' }
    $current = $verified.Value
    $state = "${current}|${needsPassword}|${boardModel}|${canAuthenticate}|${availability}|$($setting.Name)|$options"
    if ($current -ceq $mode) {
        "SAVED|$state"
    } else {
        "PENDING|$state"
    }
} catch {
    $message = $_.Exception.Message
    if ($staged) {
        try {
            $discard = Invoke-BiosMethod Lenovo_DiscardBiosSettings DiscardBiosSettings $saveParameter
        } catch { $discard = $null }
        if ($discard -ne 'Success') {
            'ERROR|DISCARD'
            exit 1
        }
    }
    if ($message -cin @('WRONG_DEVICE', 'UNSUPPORTED_SETTING', 'INVALID_MODE', 'STATE_CHANGED', 'INVALID_PASSWORD_INPUT',
        'NO_WRITE_INTERFACE', 'NO_CHOICES', 'AUTH_METHOD')) {
        "ERROR|$message"
    } else {
        # WMI exceptions can include input arguments. Only return known status values.
        if ($reply -in @('Access Denied', 'Invalid Parameter', 'Not Supported', 'System Busy', 'Item is show only, inaccessible')) {
            "ERROR|$stage|$reply"
        } else {
            "ERROR|$stage"
        }
    }
    exit 1
}
