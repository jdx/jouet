# Only what can differ on Windows: the shells the harnesses run hooks with.
# Codex runs them with PowerShell, so it gets `commandWindows`; Claude Code
# runs them with Git Bash. Each must pass the hook its stdin, and do nothing at
# all once jouet is not on PATH. Everything else about `jouet setup` is covered
# by test/setup.bats on Linux and macOS.

Describe 'jouet setup hooks on Windows' {
    BeforeAll {
        $script:OriginalDir = Get-Location
        $script:Saved = @{}
        foreach ($name in 'USERPROFILE', 'HOME', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'JOUET_LOG_DIR', 'XDG_STATE_HOME', 'PATH') {
            $script:Saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }
        $script:HookInput = '{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git add -A"},"cwd":"C:\\"}'
        $script:Pwsh = (Get-Process -Id $PID).Path
        $script:GitBash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'

        # Installs the hooks into a scratch home and returns the handler for
        # PreToolUse from the file at PATH under it.
        $script:Handler = {
            param([string]$Harness, [string]$Path)
            & jouet setup $Harness | Out-Null
            $LASTEXITCODE | Should -Be 0
            $doc = Get-Content -Raw (Join-Path $script:TestRoot $Path) | ConvertFrom-Json
            $doc.hooks.PreToolUse[0].hooks[0]
        }
    }

    AfterAll {
        Set-Location $script:OriginalDir
        foreach ($name in $script:Saved.Keys) {
            [Environment]::SetEnvironmentVariable($name, $script:Saved[$name], 'Process')
        }
    }

    BeforeEach {
        $script:TestRoot = Join-Path $TestDrive ([Guid]::NewGuid().ToString())
        New-Item -ItemType Directory -Path $script:TestRoot | Out-Null
        Set-Location $script:TestRoot
        $env:USERPROFILE = $script:TestRoot
        $env:HOME = $script:TestRoot
        $env:JOUET_LOG_DIR = Join-Path $script:TestRoot 'logs'
        $env:XDG_STATE_HOME = Join-Path $script:TestRoot 'state'
        Remove-Item Env:CODEX_HOME, Env:CLAUDE_CONFIG_DIR -ErrorAction Ignore
        $env:PATH = $script:Saved['PATH']
    }

    AfterEach {
        Set-Location $script:OriginalDir
    }

    It 'runs the Codex hook through PowerShell, and skips it silently without jouet' {
        $command = (& $script:Handler 'codex' '.codex\hooks.json').commandWindows

        $output = $script:HookInput | & $script:Pwsh -NoProfile -Command $command 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0
        $output | Should -Match '"permissionDecision":"deny"'

        $env:PATH = Join-Path $env:SystemRoot 'System32'
        $output = $script:HookInput | & $script:Pwsh -NoProfile -Command $command 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0
        $output | Should -BeNullOrEmpty
    }

    It 'runs the Claude Code hook through Git Bash, and skips it silently without jouet' {
        if (-not (Test-Path -LiteralPath $script:GitBash)) {
            Set-ItResult -Skipped -Because 'Git Bash is not installed'
            return
        }
        $command = (& $script:Handler 'claude' '.claude\settings.json').command

        $output = $script:HookInput | & $script:GitBash -c $command 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0
        $output | Should -Match '"permissionDecision":"deny"'

        $env:PATH = Join-Path $env:SystemRoot 'System32'
        $output = $script:HookInput | & $script:GitBash -c $command 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0
        $output | Should -BeNullOrEmpty
    }
}
