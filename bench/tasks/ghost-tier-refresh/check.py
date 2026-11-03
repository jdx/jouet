import os
from pathlib import Path
import subprocess
import sys

if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal'], text=True):
    sys.exit('working tree changes are not committed')

spec = Path('apps/admin/src/automations/jouet-tier-refresh.acceptance.test.tsx')
created = False
try:
    with spec.open('x') as output:
        created = True
        output.write(Path(__file__).with_name('check.test.tsx').read_text())
    result = subprocess.run(['pnpm', '--filter', '@tryghost/admin', 'test:acceptance', str(spec.relative_to('apps/admin'))],
                            timeout=240)
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
