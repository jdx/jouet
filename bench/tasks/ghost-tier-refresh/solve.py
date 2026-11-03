from pathlib import Path
import subprocess

path = Path('apps/admin/src/automations/components/canvas/automation-canvas.tsx')
old = '    defaultErrorHandler: false,\n    enabled: tiersEnabled,\n  });'
new = '    defaultErrorHandler: false,\n    enabled: tiersEnabled,\n    staleTime: 0,\n  });'
text = path.read_text()
assert text.count(old) == 1
path.write_text(text.replace(old, new))
test = Path('apps/admin/src/automations/jouet-reference.acceptance.test.tsx')
test.write_text(Path(__file__).with_name('check.test.tsx').read_text())
subprocess.run(['git', 'add', str(path), str(test)], check=True)
subprocess.run(['git', 'commit', '-qm', 'fix stale automation tiers'], check=True)
