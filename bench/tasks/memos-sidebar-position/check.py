"""Check the Memos sidebar change in jsdom and a separate Chromium checkout."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.environ['BENCH_CHECK_HELPERS'])
from _check import UNVERIFIED, expect, finish, new_commit  # noqa: E402

root = Path(__file__).resolve().parent
expected = (
    'keeps the sidebar scope popup and its Space selector fixed after positioning',
    'keeps the /setting.preference.theme/ submenu fixed after positioning',
    'keeps the /common.language/ submenu fixed after positioning',
)
browser_names = ('keeps a fixed sidebar popup in place when the page scrolls',
                 'preserves the default positioning for other popovers',
                 'preserves the default positioning for other dropdown menus',
                 'preserves the default positioning for other selects',
                 'keeps sidebar Timeline settings fixed while scrolling',
                 'keeps sidebar Timeline memo.order-by selector fixed while scrolling',
                 'keeps sidebar Timeline memo.direction selector fixed while scrolling',
                 'keeps sidebar memo Share fixed while scrolling',
                 'keeps sidebar Tags fixed while scrolling',
                 'keeps sidebar Views fixed while scrolling')


def check():
    if not new_commit():
        expect(False, 'no committed change')
        return
    status = subprocess.run(['git', 'status', '--porcelain', '--untracked-files=normal'],
                            capture_output=True, text=True, check=True)
    if status.stdout.strip():
        expect(False, 'working tree changes are not committed')
        return
    changed = subprocess.run(['git', 'diff', '--name-only', os.environ['BENCH_HEAD_SHA'], 'HEAD', '--', 'web/tests'],
                             capture_output=True, text=True, check=True).stdout.splitlines()
    expect(any('test' in name.lower() or 'spec' in name.lower() for name in changed),
           'no committed frontend regression test')
    dependencies = subprocess.run(['git', 'diff', '--quiet', os.environ['BENCH_HEAD_SHA'], 'HEAD', '--',
                                   'web/package.json', 'web/pnpm-lock.yaml'], check=False)
    if dependencies.returncode:
        UNVERIFIED.append('frontend dependency files changed; checker tooling cannot be applied safely')
        return
    with tempfile.TemporaryDirectory(prefix='memos-check-') as directory:
        checkout = Path(directory) / 'work'
        clone = subprocess.run(['git', 'clone', '--quiet', '--local', '--no-hardlinks', str(Path.cwd()), str(checkout)],
                               capture_output=True, text=True, timeout=60)
        if clone.returncode:
            UNVERIFIED.append('cannot create checker checkout: ' + clone.stderr.strip())
            return
        patch = subprocess.run(['git', 'apply', '--unidiff-zero', str(root / 'checker-tools.patch')], cwd=checkout,
                               capture_output=True, text=True, timeout=30)
        if patch.returncode:
            UNVERIFIED.append('cannot apply isolated browser tools: ' + patch.stderr.strip())
            return
        install = subprocess.run(['pnpm', '--dir', 'web', '--store-dir', os.environ['PNPM_CONFIG_STORE_DIR'], 'install', '--offline', '--frozen-lockfile'],
                                 cwd=checkout, capture_output=True, text=True, timeout=120)
        if install.returncode:
            print(f'Checker dependency setup output:\n{install.stdout}{install.stderr}')
            UNVERIFIED.append('offline checker dependencies unavailable: ' + install.stderr.strip())
            return
        web = checkout / 'web'
        if web.is_symlink() or (web / 'tests').is_symlink():
            UNVERIFIED.append('checker paths contain a symlink')
            return
        files = {
            'tests/jouet-hidden-sidebar-space.test.tsx': 'check-space.test.tsx',
            'tests/jouet-hidden-sidebar-menu.test.tsx': 'check-menu.test.tsx',
            'tests/jouet-hidden-sidebar-browser.test.tsx': 'check-browser.test.tsx',
            'tests/jouet-hidden-sidebar-integration.test.tsx': 'check-sidebar-integration.test.tsx',
            'vitest.jouet-browser.config.mts': 'browser-config.mts',
        }
        for dest, src in files.items():
            path = web / dest
            if path.exists() or path.is_symlink():
                UNVERIFIED.append(f'checker file already exists: {dest}')
                return
            shutil.copyfile(root / src, path)
        commands = [
            ('DOM', ['node', 'node_modules/vitest/vitest.mjs', 'run',
                     'tests/jouet-hidden-sidebar-space.test.tsx', 'tests/jouet-hidden-sidebar-menu.test.tsx',
                     '--reporter=json', '--outputFile', str(Path(directory) / 'dom.json')], expected, 'dom.json'),
            ('Chromium', ['node', 'node_modules/vitest/vitest.mjs', 'run', '--config', 'vitest.jouet-browser.config.mts',
                          'tests/jouet-hidden-sidebar-browser.test.tsx', 'tests/jouet-hidden-sidebar-integration.test.tsx',
                          '--reporter=json', '--outputFile',
                          str(Path(directory) / 'browser.json')], browser_names, 'browser.json'),
        ]
        for label, command, names, result_file in commands:
            proc = None
            try:
                proc = subprocess.run(command, cwd=web, capture_output=True, text=True, timeout=90)
                results = json.loads((Path(directory) / result_file).read_text())
                rows = [row for suite in results['testResults'] for row in suite['assertionResults']]
            except subprocess.TimeoutExpired as error:
                parts = (error.stdout, error.stderr)
                output = ''.join(part.decode(errors='replace') if isinstance(part, bytes) else part or '' for part in parts)
                print(f'{label} output before timeout:\n{output}')
                UNVERIFIED.append(f'{label} check timed out')
                continue
            except (OSError, ValueError, KeyError) as error:
                if proc is not None:
                    print(f'{label} output:\n{proc.stdout}{proc.stderr}')
                UNVERIFIED.append(f'{label} check could not report results: {error}')
                continue
            failures = [row.get('fullName', '<unnamed>') for row in rows if row.get('status') == 'failed']
            expect(not failures, f'{label} tests failed: ' + ', '.join(failures))
            for row in rows:
                if row.get('status') == 'failed':
                    for detail in row.get('failureMessages', []):
                        print(detail)
            for name in names:
                matches = [row for row in rows if name in row.get('fullName', '')]
                if len(matches) != 1 or matches[0].get('status') not in ('passed', 'failed'):
                    UNVERIFIED.append(f'{label} required test not verified: {name}')
            if proc.returncode and not failures:
                UNVERIFIED.append(f'{label} exited {proc.returncode} without a named failure')
            if proc.returncode:
                print(f'{label} output:\n{proc.stdout}{proc.stderr}')
            print(f'{label}: {sum(row.get("status") == "passed" for row in rows)} of {len(rows)} tests passed')


if __name__ == '__main__':
    try:
        check()
    except subprocess.TimeoutExpired as error:
        parts = (error.stdout, error.stderr)
        output = ''.join(part.decode(errors='replace') if isinstance(part, bytes) else part or '' for part in parts)
        print(f'Checker setup output before timeout:\n{output}')
        UNVERIFIED.append(f'checker setup timed out: {error}')
    except (OSError, subprocess.CalledProcessError) as error:
        UNVERIFIED.append(f'checker setup could not be completed: {error}')
    finish()
