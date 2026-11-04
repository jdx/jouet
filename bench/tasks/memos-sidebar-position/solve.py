import subprocess
from pathlib import Path

root = Path(__file__).resolve().parent
subprocess.run(['git', 'apply', '--unidiff-zero', str(root / 'reference.patch')], check=True)
subprocess.run(['git', 'add', '--', 'web/src/components/AppSidebar/SpaceSwitcher.tsx',
                'web/src/components/AppSidebar/TagsSection.tsx', 'web/src/components/AppSidebar/ViewsSection.tsx',
                'web/src/components/MemoDetailSidebar/MemoDetailSidebar.tsx', 'web/src/components/MemoDisplaySettingMenu.tsx',
                'web/src/components/UserMenu.tsx', 'web/src/components/ui/dropdown-menu.tsx',
                'web/src/components/ui/popover.tsx', 'web/src/components/ui/select.tsx',
                'web/tests/memo-detail-sidebar.test.tsx', 'web/tests/memo-display-setting-menu.test.tsx',
                'web/tests/space-switcher.test.tsx', 'web/tests/user-menu.test.tsx'], check=True)
subprocess.run(['git', 'commit', '-qm', 'Fix sidebar popup positioning'], check=True)
