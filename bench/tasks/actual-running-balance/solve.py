from pathlib import Path
import subprocess

subprocess.run(['git', 'apply', str(Path(__file__).with_name('reference.patch'))], check=True)
subprocess.run(['git', 'add', '--',
                'packages/desktop-client/e2e/accounts.test.ts',
                'packages/desktop-client/e2e/page-models/account-page.ts',
                'packages/desktop-client/src/components/accounts/Account.tsx',
                'upcoming-release-notes/refresh-running-balance.md'], check=True)
subprocess.run(['git', 'commit', '-qm', 'fix: refresh running balances after edits'], check=True)
