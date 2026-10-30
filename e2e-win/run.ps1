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

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$PSScriptRoot\..\target" }
$env:PATH = "$targetDir\debug;$env:PATH"

Invoke-Pester -Configuration $config
