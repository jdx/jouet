# Only what can differ on Windows: line endings, path syntax, cmd.exe as the
# --check shell, and the file locking and read-only attribute that make a
# rename fail. Everything else about `jouet edit` is covered by test/*.bats on
# Linux and macOS.

Describe 'jouet edit on Windows' {
    BeforeAll {
        $script:OriginalDir = Get-Location
        $script:Saved = @{}
        foreach ($name in 'JOUET_DISPLAY', 'JOUET_LOG_DIR', 'AI_AGENT', 'CLAUDECODE') {
            $script:Saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }

        # Runs jouet with a spec on stdin. The spec's lines are joined with
        # CRLF, as PowerShell pipes them to a native command.
        $script:Edit = {
            param([string[]]$Spec, [string[]]$Arguments)
            $output = $Spec | & jouet edit @Arguments 2>&1 | Out-String
            [pscustomobject]@{ Status = $LASTEXITCODE; Output = $output }
        }

        $script:Bytes = {
            param([string]$Path)
            [IO.File]::ReadAllBytes((Join-Path $script:TestRoot $Path))
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
        $env:JOUET_LOG_DIR = Join-Path $script:TestRoot 'logs'
        Remove-Item Env:JOUET_DISPLAY, Env:AI_AGENT, Env:CLAUDECODE -ErrorAction Ignore
    }

    AfterEach {
        Set-Location $script:OriginalDir
    }

    It 'keeps CRLF line endings, and LF ones, whatever the spec uses' {
        [IO.File]::WriteAllText("$script:TestRoot\crlf.txt", "one`r`ntwo`r`n")
        [IO.File]::WriteAllText("$script:TestRoot\lf.txt", "one`ntwo`n")
        $spec = @(
            '=== crlf.txt', '<<<<<<< SEARCH', 'one', '=======', '1', 'uno', '>>>>>>> REPLACE',
            '=== lf.txt', '<<<<<<< SEARCH', 'one', '=======', '1', '>>>>>>> REPLACE'
        )

        $result = & $script:Edit $spec @()

        $result.Status | Should -Be 0 -Because $result.Output
        [Text.Encoding]::ASCII.GetString((& $script:Bytes 'crlf.txt')) | Should -BeExactly "1`r`nuno`r`ntwo`r`n"
        [Text.Encoding]::ASCII.GetString((& $script:Bytes 'lf.txt')) | Should -BeExactly "1`ntwo`n"
    }

    It 'takes drive-letter paths and backslashes, and one file named both ways is edited once' {
        New-Item -ItemType Directory -Path sub | Out-Null
        [IO.File]::WriteAllText("$script:TestRoot\sub\a.txt", "one`n")
        $absolute = Join-Path $script:TestRoot 'sub\a.txt'
        $absolute | Should -Match '^[A-Za-z]:\\'
        $spec = @(
            "=== $absolute", '<<<<<<< SEARCH', 'one', '=======', 'two', '>>>>>>> REPLACE',
            '=== sub\a.txt', '<<<<<<< SEARCH', 'two', '=======', 'three', '>>>>>>> REPLACE',
            '=== sub/a.txt', '<<<<<<< SEARCH', 'three', '=======', 'four', '>>>>>>> REPLACE'
        )

        $result = & $script:Edit $spec @()

        $result.Status | Should -Be 0 -Because $result.Output
        Get-Content -Raw sub\a.txt | Should -BeExactly "four`n"
    }

    It 'creates a file under a backslash path from a Codex patch' {
        $spec = @('*** Begin Patch', '*** Add File: new\dir\c.txt', '+created', '*** End Patch')

        $result = & $script:Edit $spec @()

        $result.Status | Should -Be 0 -Because $result.Output
        Get-Content -Raw new\dir\c.txt | Should -BeExactly "created`n"
    }

    It 'runs --check through cmd.exe with its quoting intact' {
        [IO.File]::WriteAllText("$script:TestRoot\a.txt", "hello world`n")

        $output = & jouet edit replace a.txt --old world --new there --check 'findstr /C:"hello there" a.txt && if exist a.txt (exit /b 0) else (exit /b 1)' 2>&1 | Out-String

        $LASTEXITCODE | Should -Be 0 -Because $output
        $output | Should -Match 'check passed'
    }

    It 'reverts when a cmd.exe --check fails, and keeps its output in the log' {
        [IO.File]::WriteAllText("$script:TestRoot\a.txt", "one`n")

        $json = & jouet edit replace a.txt --old one --new 1 --json --check 'echo broken& exit /b 3' --revert-on-fail | Out-String
        $status = $LASTEXITCODE
        $doc = $json | ConvertFrom-Json

        $status | Should -Be 1 -Because $json
        $doc.check.exit_code | Should -Be 3
        $doc.check.reverted | Should -BeTrue
        Get-Content -Raw $doc.check.log_path | Should -Match 'broken'
        Get-Content -Raw a.txt | Should -BeExactly "one`n"
    }

    # A rename cannot replace a file another process holds open, so jouet
    # refuses with exit 1 and restores what it had written. The file below is
    # opened the way most readers and editors open one (read/write sharing,
    # without delete sharing).
    It 'restores every file when one it must replace is open in another process' {
        [IO.File]::WriteAllText("$script:TestRoot\first.txt", "a`n")
        [IO.File]::WriteAllText("$script:TestRoot\second.txt", "b`n")
        $spec = @(
            '=== first.txt', '<<<<<<< SEARCH', 'a', '=======', 'A', '>>>>>>> REPLACE',
            '=== second.txt', '<<<<<<< SEARCH', 'b', '=======', 'B', '>>>>>>> REPLACE'
        )
        $held = [IO.File]::Open("$script:TestRoot\second.txt", 'Open', 'Read', 'ReadWrite')
        try {
            $result = & $script:Edit $spec @('--json')
        } finally {
            $held.Dispose()
        }

        $result.Status | Should -Be 1 -Because $result.Output
        ($result.Output | ConvertFrom-Json).error.message | Should -Match 'second\.txt'
        Get-Content -Raw first.txt | Should -BeExactly "a`n"
        Get-Content -Raw second.txt | Should -BeExactly "b`n"
        Get-ChildItem -LiteralPath $script:TestRoot -Name -Filter '.tmp*' | Should -BeNullOrEmpty
    }

    # Unlike Unix, where the rename only needs a writable directory, Windows
    # will not replace a read-only file. jouet refuses and leaves it alone.
    It 'leaves a read-only file and its attribute as they were' {
        [IO.File]::WriteAllText("$script:TestRoot\ro.txt", "one`n")
        Set-ItemProperty -LiteralPath ro.txt -Name IsReadOnly -Value $true

        $json = & jouet edit replace ro.txt --old one --new 1 --json | Out-String
        $status = $LASTEXITCODE

        $status | Should -Be 1 -Because $json
        ($json | ConvertFrom-Json).error.message | Should -Match 'ro\.txt'
        Get-Content -Raw ro.txt | Should -BeExactly "one`n"
        (Get-Item -LiteralPath ro.txt).IsReadOnly | Should -BeTrue
        Get-ChildItem -LiteralPath $script:TestRoot -Name -Filter '.tmp*' | Should -BeNullOrEmpty
    }
}
