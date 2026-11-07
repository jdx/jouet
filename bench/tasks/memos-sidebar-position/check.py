"""Check the Memos sidebar change in jsdom and a separate Chromium checkout."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.environ['BENCH_CHECK_HELPERS'])
from _check import PROBLEMS, UNVERIFIED, expect, finish, new_commit  # noqa: E402

root = Path(__file__).resolve().parent
sys.path.insert(0, str(root))
from evidence import IncompleteEvidence, distinguishes, outcomes  # noqa: E402
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


def safe_test_path(web, name, *, required):
    """Refuse paths through symlinks, including files absent at the base."""
    parts = Path(name).parts
    if not parts or parts[0] != 'tests' or any(part in ('.', '..') for part in parts):
        raise IncompleteEvidence('invalid test path')
    path = web
    for part in parts:
        path /= part
        if path.is_symlink():
            raise IncompleteEvidence('test path contains a symlink')
    if required and not path.is_file():
        raise IncompleteEvidence(f'changed test file missing: {name}')
    if path.exists() and not path.is_file():
        raise IncompleteEvidence(f'test path is not a file: {name}')
    return path


def run_authored_tests(web, files, report, *, show_failures=False):
    """Run exactly the named files; reject incomplete reports and focused tests."""
    command = ['node', 'node_modules/vitest/vitest.mjs', 'run', *sorted(files),
               '--config', 'vitest.config.mts', '--no-allow-only',
               '--reporter=json', '--outputFile', str(report)]
    proc = subprocess.run(command, cwd=web, env={**os.environ, 'CI': '1', 'TZ': 'UTC'},
                          capture_output=True, text=True, timeout=90)
    try:
        document = json.loads(report.read_text())
        result = outcomes(document, web, files)
    except (OSError, ValueError, TypeError) as error:
        print(f'Authored test runner output:\n{proc.stdout}{proc.stderr}')
        raise IncompleteEvidence(f'authored test report unavailable: {error}') from error
    failures = [name for (_, name), status in result.items() if status == 'failed']
    if proc.returncode not in (0, 1) or (proc.returncode == 0) != (not failures):
        print(f'Authored test runner output:\n{proc.stdout}{proc.stderr}')
        raise IncompleteEvidence('authored test exit disagrees with its cases')
    if failures and show_failures:
        print(f'Authored test runner output:\n{proc.stdout}{proc.stderr}')
        for suite in document['testResults']:
            for case in suite['assertionResults']:
                if case['status'] == 'failed':
                    messages = case.get('failureMessages')
                    if isinstance(messages, list):
                        for message in messages:
                            print(message)
    return result


def checkout_state(checkout):
    """Detect commits and worktree changes before reusing a test checkout."""
    def git(*args):
        return subprocess.run(['git', *args], cwd=checkout, capture_output=True, timeout=30, check=True).stdout
    return (git('rev-parse', 'HEAD'), git('diff', '--binary', '--no-ext-diff', 'HEAD'),
            git('status', '--porcelain', '-z', '--untracked-files=all'))


def timeout_output(error):
    """TimeoutExpired may hold bytes even when text mode was requested."""
    def text(value):
        return value.decode(errors='replace') if isinstance(value, bytes) else value or ''
    return text(error.stdout) + text(error.stderr)


def check_authored_regression(checkout, changed, directory):
    """Check changed tests on the fix, the original, and the original tests."""
    test_paths = [path for path in changed if path.startswith('web/tests/')]
    other_tests = [path for path in changed if not path.startswith('web/tests/')
                   and (re.search(r'(?:\.test|\.spec|_test)\.[^/]+$', path)
                        or '/__tests__/' in path)]
    if other_tests:
        raise IncompleteEvidence('changed tests outside web/tests are not covered')
    if any(path.startswith('web/') and
           re.fullmatch(r'(?:vitest|vite)\.config\..+|tsconfig(?:\.[^/]*)?\.json|\.env(?:\..+)?',
                        Path(path).name) for path in changed):
        raise IncompleteEvidence('test runner configuration changed')
    if not test_paths or any(not re.fullmatch(r'web/tests/.+\.(?:test|spec)\.[jt]sx?', path)
                             for path in test_paths):
        raise IncompleteEvidence('changed test paths are absent or include unsupported files')
    # A changed runner or setup can change what gets executed or reported.
    config = subprocess.run(
        ['git', 'diff', '--quiet', os.environ['BENCH_HEAD_SHA'], 'HEAD', '--',
         'web/vitest.config.mts', 'web/vite.config.mts', 'web/tests/setup.ts', 'web/tsconfig.json'],
        capture_output=True, timeout=30, check=False,
    )
    if config.returncode != 0:
        raise IncompleteEvidence('test runner configuration changed or could not be compared')
    names = {path.removeprefix('web/') for path in test_paths}
    fixed_web = checkout / 'web'
    content = {name: safe_test_path(fixed_web, name, required=True).read_bytes() for name in names}
    fixed_state = checkout_state(checkout)
    fixed = run_authored_tests(fixed_web, names, directory / 'authored-fixed.json', show_failures=True)
    if (checkout_state(checkout) != fixed_state
            or any(safe_test_path(fixed_web, name, required=True).read_bytes() != content[name] for name in names)):
        raise IncompleteEvidence('fixed checkout changed during test execution')
    failed = [name for (_, name), status in fixed.items() if status == 'failed']
    if failed:
        expect(False, 'authored tests fail on the fix: ' + ', '.join(failed))
        return

    base = directory / 'authored-base'
    subprocess.run(['git', 'clone', '--quiet', '--local', '--no-hardlinks', str(Path.cwd()), str(base)],
                   capture_output=True, text=True, timeout=60, check=True)
    subprocess.run(['git', 'checkout', '--quiet', '--detach', os.environ['BENCH_HEAD_SHA']],
                   cwd=base, capture_output=True, text=True, timeout=30, check=True)
    install = subprocess.run(['pnpm', '--dir', 'web', '--store-dir', os.environ['PNPM_CONFIG_STORE_DIR'],
                              'install', '--offline', '--frozen-lockfile'],
                             cwd=base, capture_output=True, text=True, timeout=120)
    if install.returncode:
        raise IncompleteEvidence('offline baseline dependencies unavailable')
    base_web = base / 'web'
    if base_web.is_symlink() or (base_web / 'tests').is_symlink():
        raise IncompleteEvidence('baseline test directory contains a symlink')
    existing = {name for name in names if safe_test_path(base_web, name, required=False).is_file()}
    original_state = checkout_state(base)
    pristine = run_authored_tests(base_web, existing, directory / 'authored-pristine.json') if existing else {}
    if checkout_state(base) != original_state:
        raise IncompleteEvidence('original checkout changed during test execution')
    for name in names:
        target = safe_test_path(base_web, name, required=False)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content[name])
    changed_state = checkout_state(base)
    before = run_authored_tests(base_web, names, directory / 'authored-before.json')
    if (checkout_state(base) != changed_state
            or any(safe_test_path(base_web, name, required=True).read_bytes() != content[name] for name in names)):
        raise IncompleteEvidence('baseline checkout changed during test execution')
    if not distinguishes(fixed, before, pristine):
        raise IncompleteEvidence('authored tests did not distinguish the fix from the original')
    print('Authored regression: passed on the fix and newly failed on the original')


def check():
    if not new_commit():
        expect(False, 'no committed change')
        return
    status = subprocess.run(['git', 'status', '--porcelain', '--untracked-files=normal'],
                            capture_output=True, text=True, check=True)
    if status.stdout.strip():
        expect(False, 'working tree changes are not committed')
        return
    changed = subprocess.run(['git', 'diff', '--name-only', '-z', os.environ['BENCH_HEAD_SHA'], 'HEAD', '--'],
                             capture_output=True, text=True, check=True).stdout.split('\0')
    changed = [path for path in changed if path]
    expect(any(name.startswith('web/tests/') and ('test' in name.lower() or 'spec' in name.lower()) for name in changed),
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
        if not PROBLEMS and not UNVERIFIED:
            try:
                check_authored_regression(checkout, changed, Path(directory))
            except subprocess.TimeoutExpired as error:
                print(f'Authored regression timeout output:\n{timeout_output(error)}')
                UNVERIFIED.append(f'authored regression unverified: {error}')
            except (IncompleteEvidence, OSError, subprocess.SubprocessError) as error:
                UNVERIFIED.append(f'authored regression unverified: {error}')


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
