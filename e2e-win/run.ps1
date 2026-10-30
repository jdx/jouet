param(
    [string]$TestName
)

# Pester 5. Windows ships Pester 3, which finds no tests in this layout.
Import-Module Pester -MinimumVersion 5.0 -ErrorAction Stop

$config = New-PesterConfiguration
$config.Run.Path = $PSScriptRoot
$config.Run.Exit = $true
$config.TestResult.Enabled = $true
$config.Output.Verbosity = 'Detailed'

if ($TestName) {
    $config.Filter.FullName = $TestName
}

# Cargo resolves a relative CARGO_TARGET_DIR against the working directory.
# Resolve it here, because every test changes into a directory of its own.
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot '..\target' }
$targetDir = [IO.Path]::GetFullPath($targetDir, (Get-Location).ProviderPath)
$jouet = Join-Path $targetDir 'debug\jouet.exe'
if (-not (Test-Path -LiteralPath $jouet)) {
    throw "jouet binary not found; run 'cargo build' first: $jouet"
}
$env:PATH = "$(Split-Path -Parent $jouet);$env:PATH"

Invoke-Pester -Configuration $config
