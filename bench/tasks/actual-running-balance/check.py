import os
from pathlib import Path
import subprocess
import sys

spec = Path('packages/desktop-client/e2e/jouet-running-balance.test.ts')
created = False
try:
    with spec.open('x') as out:
        created = True
        out.write(Path(__file__).with_name('check.test.ts').read_text())
    result = subprocess.run([
        'node', 'node_modules/@playwright/test/cli.js', 'test',
        '--config', 'packages/desktop-client/playwright.config.ts',
        spec.name, '--retries=0',
    ], timeout=240)
    if result.returncode:
        sys.exit(result.returncode)
finally:
    if created:
        spec.unlink(missing_ok=True)

base = os.environ['BENCH_BASE_SHA']
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
if head == base:
    sys.exit('no committed change')
changed = subprocess.check_output(['git', 'diff', '--name-only', base, head], text=True).splitlines()
if not any('test' in path.lower() or 'spec' in path.lower() for path in changed):
    sys.exit('no committed regression test')
notes = subprocess.check_output([
    'git', 'diff', '--name-only', '--diff-filter=A', base, head, '--', 'upcoming-release-notes/',
], text=True).splitlines()
notes = [path for path in notes if path.endswith('.md') and path != 'upcoming-release-notes/README.md']
script = Path(__file__).with_name('check-note.mjs').read_text()
result = subprocess.run(['node', '--input-type=module', '-e', script, '--', *notes], check=False)
if result.returncode:
    sys.exit('no valid committed release note crediting the supplied contributor')
if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal'], text=True):
    sys.exit('working tree changes are not committed')
