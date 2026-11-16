param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Output,
    [string]$PayloadDirectory = (Join-Path $PSScriptRoot 'payloads'),
    [string]$BundleFrom
)

$ErrorActionPreference = 'Stop'
$magic = 'CorpRustBundle01'
$names = @('BCUninstaller.zip', 'CorpInstaller.BcuBridge.exe')
$hashes = @(
    '93F6C3543FDFFA02159967DECFB887E035B3904A7D9087F3E5D806BBF06C7B78',
    'BAE3D7BD1EDD14EFCC4BD0A9C64083225EC8EF82748ED514090143C49536BFFA'
)
$lengths = @([long]79654760, [long]41356405)
$notice = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'THIRD-PARTY-NOTICES.md'))
$corePath = (Resolve-Path -LiteralPath $Executable).ProviderPath
$outputPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
if (Test-Path -LiteralPath $outputPath) { throw "Output already exists: $outputPath" }
$tempOutput = "$outputPath.$([Guid]::NewGuid()).tmp"

function Copy-VerifiedEngine($InputStream, $OutputStream, [long]$Length, [string]$ExpectedHash, [string]$Name) {
    $buffer = [byte[]]::new(1MB)
    $digest = [Security.Cryptography.SHA256]::Create()
    try {
        while ($Length -gt 0) {
            $read = $InputStream.Read($buffer, 0, [int][Math]::Min($buffer.Length, $Length))
            if ($read -le 0) { throw "Truncated component: $Name" }
            [void]$digest.TransformBlock($buffer, 0, $read, $buffer, 0)
            $OutputStream.Write($buffer, 0, $read)
            $Length -= $read
        }
        [void]$digest.TransformFinalBlock([byte[]]::new(0), 0, 0)
        $actual = [BitConverter]::ToString($digest.Hash).Replace('-', '')
        if ($actual -ne $ExpectedHash) { throw "Component SHA-256 mismatch: $Name" }
    } finally { $digest.Dispose() }
}

$core = [IO.File]::OpenRead($corePath)
$prior = $null
try {
    if ($core.Length -lt 64 -or $core.ReadByte() -ne 77 -or $core.ReadByte() -ne 90) {
        throw 'Expected a Windows executable.'
    }
    $reader = [IO.BinaryReader]::new($core, [Text.Encoding]::ASCII, $true)
    try {
        $core.Position = $core.Length - 16
        if ([Text.Encoding]::ASCII.GetString($reader.ReadBytes(16)) -eq $magic) {
            throw 'Executable must be the Cargo output before bundling.'
        }
    } finally { $reader.Dispose() }
    $core.Position = 0
    if ($BundleFrom) {
        $prior = [IO.File]::OpenRead((Resolve-Path -LiteralPath $BundleFrom).ProviderPath)
        if ($prior.Length -lt 40) { throw 'Invalid source bundle.' }
        $prior.Position = $prior.Length - 40
        $reader = [IO.BinaryReader]::new($prior, [Text.Encoding]::ASCII, $true)
        try {
            $archiveSize = $reader.ReadUInt64()
            $bridgeSize = $reader.ReadUInt64()
            $noticeSize = $reader.ReadUInt64()
            if ($archiveSize -ne $lengths[0] -or $bridgeSize -ne $lengths[1] -or
                $noticeSize -lt 1 -or $noticeSize -gt 65536 -or
                [Text.Encoding]::ASCII.GetString($reader.ReadBytes(16)) -ne $magic) {
                throw 'Invalid source bundle.'
            }
            $offset = $prior.Length - 40 - $lengths[0] - $lengths[1] - [long]$noticeSize
            if ($offset -lt 64) { throw 'Invalid source bundle offsets.' }
            $prior.Position = $offset
        } finally { $reader.Dispose() }
    }

    $destination = [IO.File]::Open($tempOutput, [IO.FileMode]::CreateNew)
    try {
        $core.CopyTo($destination)
        for ($i = 0; $i -lt $names.Length; $i++) {
            if ($prior) {
                Copy-VerifiedEngine $prior $destination $lengths[$i] $hashes[$i] $names[$i]
            } else {
                $path = (Resolve-Path -LiteralPath (Join-Path $PayloadDirectory $names[$i])).ProviderPath
                $inputFile = [IO.File]::OpenRead($path)
                try {
                    if ($inputFile.Length -ne $lengths[$i]) { throw "Invalid component size: $path" }
                    Copy-VerifiedEngine $inputFile $destination $lengths[$i] $hashes[$i] $names[$i]
                } finally { $inputFile.Dispose() }
            }
        }
        $writer = [IO.BinaryWriter]::new($destination, [Text.Encoding]::ASCII, $true)
        try {
            $writer.Write($notice)
            foreach ($length in $lengths) { $writer.Write([uint64]$length) }
            $writer.Write([uint64]$notice.Length)
            $writer.Write([Text.Encoding]::ASCII.GetBytes($magic))
        } finally { $writer.Dispose() }
    } finally { $destination.Dispose() }
    [IO.File]::Move($tempOutput, $outputPath)
} finally {
    if ($prior) { $prior.Dispose() }
    $core.Dispose()
    if (Test-Path -LiteralPath $tempOutput) { Remove-Item -LiteralPath $tempOutput }
}
