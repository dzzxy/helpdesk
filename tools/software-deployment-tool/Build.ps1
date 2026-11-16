param(
    [switch]$Test,
    [string]$PayloadDirectory = (Join-Path $PSScriptRoot 'payloads'),
    [string]$BundleFrom,
    [string]$Output
)

$ErrorActionPreference = 'Stop'
Push-Location -LiteralPath $PSScriptRoot
try {
    $localCargo = Join-Path $PSScriptRoot 'tools/cargo/bin/cargo.exe'
    $cargo = 'cargo'
    if (Test-Path -LiteralPath $localCargo) {
        $env:CARGO_HOME = Join-Path $PSScriptRoot 'tools/cargo'
        $env:RUSTUP_HOME = Join-Path $PSScriptRoot 'tools/rustup'
        $cargo = $localCargo
    }
    if ($Test) {
        & $cargo test --locked --target x86_64-pc-windows-msvc --target-dir target
        if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
    }
    & $cargo build --release --locked --target x86_64-pc-windows-msvc --target-dir target
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }

    if (-not $Output) {
        $version = (Select-String -LiteralPath Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
        $dist = New-Item -ItemType Directory -Path dist -Force
        $Output = Join-Path $dist.FullName "CorpInstaller.Rust-$version.exe"
    }
    & ./Pack-SingleFile.ps1 -Executable 'target/x86_64-pc-windows-msvc/release/software-deployment-rust.exe' `
        -Output $Output -PayloadDirectory $PayloadDirectory -BundleFrom $BundleFrom
} finally { Pop-Location }
