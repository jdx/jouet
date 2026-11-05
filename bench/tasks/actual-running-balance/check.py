"""Check the Actual running-balance task, separating missing evidence from failure."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, expect, finish  # noqa: E402

TEST_NAME = 'running balances refresh after repeated edits'


def print_timeout_output(error):
    for output in (error.stdout, error.stderr):
        if output:
            print(output.decode(errors='replace') if isinstance(output, bytes) else output)


def specs(suites):
    for suite in suites:
        yield from suite.get('specs', [])
        yield from specs(suite.get('suites', []))


def browser_verdict(report, exit_code):
    """Only a completed passing test or a relevant assertion proves an outcome."""
    try:
        matching = [s for s in specs(report['suites']) if s['title'] == TEST_NAME]
        if len(matching) != 1 or len(matching[0]['tests']) != 1:
            return 'unverified'
        results = matching[0]['tests'][0]['results']
        if len(results) != 1:
            return 'unverified'
        result = results[0]
        if result['status'] == 'passed' and exit_code == 0 and not report.get('errors'):
            return 'passed'
        message = result.get('error', {}).get('message', '')
        message = re.sub(r'\x1b\[[0-9;]*m', '', message)
        if (result['status'] == 'failed' and 'expect(' in message and ').toHaveText(' in message
                and ("getByTestId('balance')" in message or "getByTestId('notes')" in message)):
            return 'failed'
    except (KeyError, TypeError, AttributeError):
        pass
    return 'unverified'


def check():
    # Check before Playwright creates reports or other untracked artifacts.
    if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal'], text=True):
        expect(False, 'working tree changes are not committed')
        return

    base = os.environ['BENCH_BASE_SHA']
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    if head == base:
        expect(False, 'no committed change')
        return
    changed = subprocess.check_output(['git', 'diff', '--name-only', base, head], text=True).splitlines()
    expect(any('test' in path.lower() or 'spec' in path.lower() for path in changed),
           'no committed regression test')
    notes = subprocess.check_output([
        'git', 'diff', '--name-only', '--diff-filter=A', base, head, '--', 'upcoming-release-notes/',
    ], text=True).splitlines()
    notes = [path for path in notes if path.endswith('.md') and path != 'upcoming-release-notes/README.md']
    script = Path(__file__).with_name('check-note.mjs').read_text()
    if not notes:
        expect(False, 'no valid committed release note crediting the supplied contributor')
    else:
        try:
            result = subprocess.run(['node', '--input-type=module', '-e', script, '--', *notes],
                                    capture_output=True, text=True, timeout=30)
            if result.returncode:
                print(result.stdout, end='')
                print(result.stderr, end='')
                if 'The release note must follow the project format and credit jouet-bench.' in result.stderr:
                    expect(False, 'no valid committed release note crediting the supplied contributor')
                else:
                    UNVERIFIED.append('release note validation could not be completed')
        except subprocess.TimeoutExpired as error:
            print_timeout_output(error)
            UNVERIFIED.append('release note validation timed out')
        except OSError as error:
            UNVERIFIED.append(f'release note validation could not start: {error}')

    spec = Path('packages/desktop-client/e2e/jouet-running-balance.test.ts')
    created = False
    try:
        if any(path.is_symlink() for path in (spec, *spec.parents)):
            UNVERIFIED.append('checker test path contains a symlink')
            return
        with spec.open('x') as out:
            created = True
            out.write(Path(__file__).with_name('check.test.ts').read_text())
        with tempfile.TemporaryDirectory(prefix='actual-check-') as directory:
            report = Path(directory) / 'playwright.json'
            try:
                result = subprocess.run([
                    'node', 'node_modules/@playwright/test/cli.js', 'test',
                    '--config', 'packages/desktop-client/playwright.config.ts',
                    spec.name, '--retries=0', '--reporter=json',
                ], env={**os.environ, 'PLAYWRIGHT_JSON_OUTPUT_FILE': str(report)},
                    capture_output=True, text=True, timeout=240)
            except subprocess.TimeoutExpired as error:
                print_timeout_output(error)
                UNVERIFIED.append('Playwright check timed out')
                return
            except OSError as error:
                UNVERIFIED.append(f'Playwright could not start: {error}')
                return
            try:
                verdict = browser_verdict(json.loads(report.read_text()), result.returncode)
            except (OSError, ValueError):
                verdict = 'unverified'
            if verdict != 'passed':
                print(result.stdout, end='')
                print(result.stderr, end='')
                if report.exists():
                    print(report.read_text())
            if verdict == 'failed':
                expect(False, 'running balance or transaction editing assertion failed')
            elif verdict == 'unverified':
                UNVERIFIED.append('Playwright did not establish the required browser behavior')
    except OSError as error:
        UNVERIFIED.append(f'could not place checker test: {error}')
    finally:
        if created:
            spec.unlink(missing_ok=True)


if __name__ == '__main__':
    check()
    finish()
