$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Invoke-SceWin($Arguments, $Operation) {
    $process = Start-Process -FilePath $tool.FullName -WorkingDirectory $tool.DirectoryName -ArgumentList $Arguments -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        $detail = @(foreach ($log in @($stderr, $stdout)) {
            if (Test-Path -LiteralPath $log) {
                Get-Content -LiteralPath $log -Tail 8 | Where-Object { $_.Trim() }
            }
        }) -join ' '
        if ($detail.Length -gt 240) { $detail = $detail.Substring(0, 240) }
        $message = "$Operation failed ($toolSource, SCEWIN exit $($process.ExitCode)). $detail"
        $failure = [InvalidOperationException]::new($message.Trim())
        $failure.Data['SceWinExitCode'] = $process.ExitCode
        throw $failure
    }
}

function Read-BiosExport($Path) {
    Invoke-SceWin ('/o /s "{0}" /v' -f $Path) 'BIOS export'
    ,([IO.File]::ReadAllLines($Path))
}

function Find-SceWin([switch]$Optional) {
    $roots = @(
        [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFilesX86)
        [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
    ) | Where-Object { $_ } | Select-Object -Unique
    $scans = @()
    $candidates = foreach ($root in $roots) {
        $path = Join-Path $root 'MSI\MSI Center\Lib\SCEWIN'
        if (!(Test-Path -LiteralPath $path -PathType Container -ErrorAction SilentlyContinue -ErrorVariable +scans)) { continue }
        Get-ChildItem -LiteralPath $path -Filter 'SCEWIN_64.exe' -File -Recurse -Force -ErrorAction SilentlyContinue -ErrorVariable +scans
    }
    $found = $candidates | Sort-Object -Property @{
        Expression = {
            $version = $null
            if ([version]::TryParse($_.Directory.Name, [ref]$version)) { $version }
            else { [version]'0.0' }
        }
        Descending = $true
    }, FullName | Select-Object -First 1
    if ($found) { return $found }
    if ($Optional) { return }
    if ($scans.Count) {
        throw '无法访问 MSI Center 的 SCEWIN 目录。请确认目录权限后刷新。'
    }
    $center = Get-AppxPackage -AllUsers -Name '*MSICenter*' -ErrorAction SilentlyContinue
    if ($center) {
        throw '已检测到 MSI Center，但未找到 SCEWIN。请打开 MSI Center 完成 SDK 初始化后刷新。若仍然缺失，当前机型的 SDK 可能未提供该组件。'
    }
    throw '未找到 SCEWIN。请安装该主板支持的 MSI Center，打开并完成 SDK 初始化后刷新。'
}

function Expand-SceWinBundle([string[]]$Payload, [string]$Folder) {
    for ($index=0; $index -lt $bundleNames.Count; $index++) {
        $path = Join-Path $Folder $bundleNames[$index]
        [IO.File]::WriteAllBytes($path, [Convert]::FromBase64String($Payload[$index]))
    }
    Get-Item -LiteralPath (Join-Path $Folder $bundleNames[0])
}

function Confirm-SceWinSignature($Executable) {
    foreach ($file in @($Executable) + @(Get-ChildItem -LiteralPath $Executable.DirectoryName -Filter '*.sys' -File)) {
        $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'AMI US HOLDINGS|American Megatrends|Microsoft') {
            throw "The MSI Center component failed signature verification: $($file.Name)."
        }
    }
}

function Install-MsiSdk($OfflineInstaller) {
    if (Find-SceWin -Optional) { return 'READY' }
    if ($OfflineInstaller) {
        $file = Get-Item -LiteralPath $OfflineInstaller
        $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
        if ($file.Name -cne 'MSI Center SDK.exe' -or
            $file.VersionInfo.FileDescription.Trim() -cne 'MSI Center SDK Setup' -or
            $signature.Status -ne 'Valid' -or
            $signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName,$false) -ine 'MICRO-STAR INTERNATIONAL CO., LTD.') {
            throw '本地安装包未通过微星官方 SDK 签名检查。'
        }
        $process = Start-Process -FilePath $file.FullName -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "SDK 安装程序退出码为 $($process.ExitCode)。" }
        if (Find-SceWin -Optional) { return 'READY' }
        return 'OFFLINE_MISSING'
    }
    $packageName = '9426MICRO-STARINTERNATION.MSICenter'
    $center = Get-AppxPackage -Name $packageName -ErrorAction SilentlyContinue
    if (!$center) {
        $appInstaller = Get-AppxPackage -Name 'Microsoft.DesktopAppInstaller' -ErrorAction SilentlyContinue |
            Sort-Object Version -Descending | Select-Object -First 1
        if ($appInstaller) {
            $winget = Join-Path $appInstaller.InstallLocation 'winget.exe'
            $signature = Get-AuthenticodeSignature -LiteralPath $winget -ErrorAction SilentlyContinue
            if ($signature.Status -eq 'Valid' -and $signature.SignerCertificate.Subject -match 'Microsoft') {
                $arguments = 'install --id 9NVMNJCR03XV --exact --source msstore --accept-package-agreements --accept-source-agreements --disable-interactivity'
                $process = Start-Process -FilePath $winget -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
                if ($process.ExitCode -eq 0) {
                    $center = Get-AppxPackage -Name $packageName -ErrorAction SilentlyContinue
                }
            }
        }
        if (!$center) {
            Start-Process 'ms-windows-store://pdp/?ProductId=9NVMNJCR03XV'
            return 'STORE'
        }
    }
    $shell = Join-Path $env:WINDIR 'explorer.exe'
    Start-Process -FilePath $shell -ArgumentList "shell:AppsFolder\$($center.PackageFamilyName)!App"
    $deadline = [datetime]::UtcNow.AddSeconds(120)
    do {
        Start-Sleep -Seconds 4
        if (Find-SceWin -Optional) { return 'READY' }
    } while ([datetime]::UtcNow -lt $deadline)
    'OPENED'
}

function Get-Question([string[]]$Lines, [string[]]$Names, $Recommended, [bool]$Required, $Requested) {
    $indices = @(for ($i = 0; $i -lt $Lines.Length; $i++) {
        if ($Lines[$i] -match '^Setup Question\s*=\s*(.+?)\s*$' -and $Matches[1] -cin $Names) { $i }
    })
    if ($indices.Count -eq 0 -and !$Required) {
        if ($Requested) { throw "The BIOS no longer provides $($Names[0])." }
        return [pscustomobject]@{Name=$Names[0]; Current='Unavailable'; Target=''; Recommended=$Recommended; Choices=@(); Patch=@(); Original=@()}
    }
    if ($indices.Count -ne 1) { throw "Could not identify one BIOS setting for $($Names -join ' / ')." }
    $start = $indices[0]
    $name = ($Lines[$start] -split '=', 2)[1].Trim()
    if ($start -lt 2 -or $Lines[$start-2] -notmatch '^// GUID\s*=\s*[0-9a-f-]{36}\s*$' -or
        $Lines[$start-1] -notmatch '^// Variable\s*=\s*\S+\s*$') {
        throw "Missing firmware metadata for $name."
    }
    $end = $start + 1
    while ($end -lt $Lines.Length -and ![string]::IsNullOrWhiteSpace($Lines[$end])) { $end++ }
    $block = @($Lines[($start-2)..($end-1)])
    $original = @($block.Clone())
    if (@($block -match '^Token\s*=\s*[0-9a-f]+(?:\s|$)').Count -ne 1 -or
        @($block -match '^Setup Question\s*=').Count -ne 1) { throw "Invalid setting block for $name." }
    $values = @($block -match '^Value\s*=')
    $options = @($block -match '^(?:Options\s*=\s*|\s+)\*?\[[0-9a-f]+\]')
    $expectedChoices = if ($Recommended -eq 'BIOS') { @('BIOS','OS') } else { @('Enabled','Disabled') }
    if ($values.Count -eq 1 -and $options.Count -eq 0 -and $Recommended -ne 'BIOS' -and
        $values[0] -match '^(Value\s*=\s*)([01])(\s*//\s*Enabled\s*=\s*1,\s*Disabled\s*=\s*0\s*)$') {
        $current = if ($Matches[2] -eq '1') { 'Enabled' } else { 'Disabled' }
        $choices = $expectedChoices
        if ($Requested -and $Requested -cnotin $choices) { throw "Invalid target value for $name." }
        $target = if ($Requested) { $Requested } else { $current }
        $targetNumber = [int]($target -eq 'Enabled')
        $valueIndex = [Array]::IndexOf($block, $values[0])
        $block[$valueIndex] = "$($Matches[1])$targetNumber$($Matches[3])"
    } elseif ($values.Count -eq 0 -and $options.Count -gt 1) {
        $selected = @($options -match '^(?:Options\s*=\s*|\s+)\*\[')
        if ($selected.Count -ne 1 -or
            $selected[0] -notmatch '\*\[[0-9a-f]+\]\s*([^/]+?)(?:\s*//.*)?$') {
            throw "Could not confirm the current value for $name."
        }
        $current = $Matches[1].Trim()
        $choices = @(foreach ($option in $options) {
            if ($option -match '^(?:Options\s*=\s*|\s+)\*?\[[0-9a-f]+\]\s*([^/]+?)(?:\s*//.*)?$') {
                $candidate = $Matches[1].Trim()
                if ($candidate -iin $expectedChoices) { $candidate }
            }
        })
        if ($current -cnotin $choices -or @($choices | Select-Object -Unique).Count -ne $choices.Count) {
            throw "Could not identify unambiguous values for $name."
        }
        if ($Requested -and $Requested -cnotin $choices) { throw "Invalid target value for $name." }
        $target = if ($Requested) { $Requested } else { $current }
        if ($current -ine $Target) {
            $targetLine = @($options | Where-Object {
                $_ -match '^(?:Options\s*=\s*|\s+)\*?\[[0-9a-f]+\]\s*([^/]+?)(?:\s*//.*)?$' -and $Matches[1].Trim() -ceq $target
            })
            $selectedIndex = [Array]::IndexOf($block, $selected[0])
            $targetIndex = [Array]::IndexOf($block, $targetLine[0])
            $block[$selectedIndex] = $selected[0] -replace '\*(?=\[)', ''
            $block[$targetIndex] = $targetLine[0] -replace '(?=\[[0-9a-f]+\])', '*'
        }
    } else { throw "Unsupported value format for $name." }
    $patchLines = if ($current -ine $Target) { $block }
    [pscustomobject]@{Name=$name; Current=$current; Target=$target; Recommended=$Recommended; Choices=$choices; Patch=@($patchLines | Where-Object { $_ }); Original=$original}
}

function Get-WolPlan([string[]]$Lines, [string[]]$Requested = @('','','','')) {
    $first = [Array]::FindIndex($Lines, [Predicate[string]]{param($line) $line -match '^// FORM SET\s*$'})
    if ($first -lt 1 -or $first -gt 32) { throw 'Could not identify the SCEWIN script header.' }
    $header = @($Lines[0..($first-1)])
    # SCEWIN can omit leading zeroes from the 32-bit CRC. Preserve the exported line for import.
    $crc = @($header -match '^\s*HIICrc32\b')
    if ($crc.Count -ne 1 -or $crc[0] -notmatch '^HIICrc32\s*=\s*[0-9a-f]{1,8}\s*$' -or
        @($header -match '^(?!//|HIICrc32\b)\S').Count) { throw 'Invalid SCEWIN script header.' }
    $rows = @(
        Get-Question $Lines @('Wake on LAN Enable') 'Enabled' $false $Requested[0]
        Get-Question $Lines @('Wake Up Event By') 'BIOS' $false $Requested[1]
        Get-Question $Lines @('Resume By PCI-E/Networking Device','Resume By PCI-E Device') 'Enabled' $true $Requested[2]
        Get-Question $Lines @('ErP Ready') 'Disabled' $false $Requested[3]
    )
    $hash = [Security.Cryptography.SHA256]::Create()
    try {
        $text = (@($board.Product, $bios, $toolSource, $toolHash, $crc[0]) + @($rows | ForEach-Object { $_.Original })) -join [char]10
        $token = [BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($text))).Replace('-','')
    } finally { $hash.Dispose() }
    [pscustomobject]@{Header=$header; Rows=$rows; Token=$token}
}

$paths = @()
$bundleNames = 'SCEWIN_64.exe','amifldrv64.sys','amigendrv64.sys'
$bundleFolder = $null
$passwordFolder = $null
$passwordPath = $null
$mutex = $null
$locked = $false
try {
    $operation = [Console]::ReadLine()
    $approvedToken = [Console]::ReadLine()
    $requested = @(for ($i=0; $i -lt 4; $i++) { [string][Console]::ReadLine() })
    $password = [Console]::ReadLine()
    $payload = @(for ($i=0; $i -lt $bundleNames.Count; $i++) { [string][Console]::ReadLine() })
    $hasBundle = @($payload | Where-Object { $_ }).Count -eq $bundleNames.Count
    $board = Get-CimInstance Win32_BaseBoard
    if ($board.Manufacturer -notmatch '^(Micro-Star International|MSI)') { throw 'This tool requires an MSI motherboard.' }
    $bios = (Get-CimInstance Win32_BIOS).SMBIOSBIOSVersion
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this tool as a Windows administrator.' }
    if ($operation -ceq 'setup') {
        if ($hasBundle) { 'SETUP|READY' }
        else { "SETUP|$(Install-MsiSdk $approvedToken)" }
        exit 0
    }
    $mutex = [Threading.Mutex]::new($false, 'Global\MsiWolFirmwareOperation')
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another MSI WoL operation is running. Try again after it finishes.' }
    $tool = $null
    $toolSource = 'MSI Center'
    if (!$hasBundle -and $operation -cne 'read-bundled') { $tool = Find-SceWin }
    if (!$tool -and $hasBundle) {
        $programFiles = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
        $folder = Join-Path $programFiles ('MSI-WoL-' + [guid]::NewGuid().ToString('N'))
        $bundleFolder = (New-Item -ItemType Directory -Path $folder).FullName
        $tool = Expand-SceWinBundle $payload $bundleFolder
        $toolSource = 'bundled'
    }
    if (!$tool) {
        if ($operation -ceq 'read-bundled') { throw '此版本未嵌入 SCEWIN。' }
        $tool = Find-SceWin
    }
    Confirm-SceWinSignature $tool
    $stem = Join-Path "$env:WINDIR\Temp" ('msi-wol-' + [guid]::NewGuid().ToString('N'))
    $dump="$stem.txt"; $stdout="$stem.out"; $stderr="$stem.err"
    $patch="$stem.patch.txt"; $verified="$stem.verify.txt"
    $paths = $dump,$stdout,$stderr,$patch,$verified
    $lines = Read-BiosExport $dump
    $toolHash = (Get-FileHash -LiteralPath $tool.FullName -Algorithm SHA256).Hash
    if ($operation -ceq 'apply') {
        $plan = Get-WolPlan $lines $requested
        if ($approvedToken -cne $plan.Token) { throw 'BIOS settings changed since preview. Open the tool again to review the new values.' }
        $changes = @($plan.Rows | Where-Object { $_.Patch.Count })
        if ($changes.Count) {
            $payload = @($plan.Header) + @(foreach ($row in $changes) { $row.Patch; '' })
            [IO.File]::WriteAllLines($patch, [string[]]$payload, [Text.Encoding]::ASCII)
            $importArguments = '/i /s "{0}" /r' -f $patch
            if ($password) {
                if ($password -notmatch '^[\x20-\x7E]+$') { throw 'SCEWIN BIOS 密码必须使用可打印 ASCII 字符。' }
                $acl = [Security.AccessControl.DirectorySecurity]::new()
                $acl.SetAccessRuleProtection($true, $false)
                foreach ($sid in @($identity.User.Value, 'S-1-5-18', 'S-1-5-32-544') | Select-Object -Unique) {
                    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
                        [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl',
                        'ContainerInherit,ObjectInherit', 'None', 'Allow')
                    $acl.AddAccessRule($rule)
                }
                $directory = Join-Path "$env:WINDIR\Temp" ('msi-wol-auth-' + [guid]::NewGuid().ToString('N'))
                $passwordFolder = [IO.Directory]::CreateDirectory($directory, $acl).FullName
                $passwordPath = Join-Path $passwordFolder 'password.txt'
                [IO.File]::WriteAllText($passwordPath, $password, [Text.Encoding]::ASCII)
                $importArguments += ' /cpwdf "{0}"' -f $passwordPath
            }
            try { Invoke-SceWin $importArguments 'BIOS import' }
            finally { if ($passwordPath) { Remove-Item -LiteralPath $passwordPath -ErrorAction SilentlyContinue } }
            $plan = Get-WolPlan (Read-BiosExport $verified)
            foreach ($change in $changes) {
                $readback = @($plan.Rows | Where-Object Name -CEQ $change.Name)
                if ($readback.Count -ne 1 -or $readback[0].Current -ine $change.Target) {
                    throw "BIOS readback did not confirm $($change.Name). Review all settings before retrying."
                }
            }
        }
    } elseif ($operation -cin @('read','read-bundled')) {
        $plan = Get-WolPlan $lines
    } else { throw 'Unsupported operation.' }

    "BOARD|$($board.Product)|$bios"
    "TOKEN|$($plan.Token)"
    foreach ($row in $plan.Rows) { "ROW|$($row.Name)|$($row.Current)|$($row.Recommended)|$($row.Choices -join ',')" }
} catch {
    $reason = $_.Exception.Message
    if ($password) { $reason = $reason.Replace($password, '[redacted]') }
    "ERROR|$($reason -replace '[|\r\n]+',' ')"
    exit 1
} finally {
    if ($passwordPath) { Remove-Item -LiteralPath $passwordPath -ErrorAction SilentlyContinue }
    if ($passwordFolder) { Remove-Item -LiteralPath $passwordFolder -ErrorAction SilentlyContinue }
    foreach ($path in $paths) {
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue }
    }
    if ($bundleFolder) {
        foreach ($item in (Get-ChildItem -LiteralPath $bundleFolder -File -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $item.FullName -ErrorAction SilentlyContinue
        }
        if (!(Get-ChildItem -LiteralPath $bundleFolder -Force -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $bundleFolder -ErrorAction SilentlyContinue
        }
    }
    if ($locked) { $mutex.ReleaseMutex() }
    if ($mutex) { $mutex.Dispose() }
}
