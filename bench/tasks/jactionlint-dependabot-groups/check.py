import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import (
    UNVERIFIED,
    committed_files,
    dirty,
    expect,
    finish,
    head_sha,
    new_commit,
)


def timeout_output(error):
    def text(value):
        return value.decode(errors='replace') if isinstance(value, bytes) else value or ''
    return (text(error.stdout) + text(error.stderr))[-3000:]


def check_untracked():
    try:
        run = subprocess.run(['git', 'ls-files', '--others', '-z'], capture_output=True, text=True,
                             timeout=30, check=True)
    except (OSError, subprocess.SubprocessError) as error:
        UNVERIFIED.append(f'could not check untracked files: {error}')
        return
    files = [path for path in run.stdout.split('\0') if path]
    if files:
        UNVERIFIED.append(f'{len(files)} untracked or ignored files may affect the checks')


def check_dependabot_fixtures(files):
    """Recognize the pinned repository's fixture test without guessing other tests."""
    fixtures = []
    for path in files:
        match = re.fullmatch(r'testdata/(ok|err|examples)/(dependabot_[A-Za-z0-9_]+)\.yaml', path)
        if match and Path(path).is_file() and not any(part.is_symlink() for part in (Path(path), *Path(path).parents)):
            fixtures.append(f'TestDependabotFixtures/{match[1]}/{match[2]}')
    test_files = {p for p in files if p.endswith('_test.go')}
    if not fixtures or test_files:
        return False
    names = ['TestDependabotFixtures']
    try:
        run = subprocess.run(['go', 'test', '-json', '-count=1', '-run', '^(' + '|'.join(names) + ')$', '.'],
                             capture_output=True, text=True, timeout=300, check=False)
        events = [json.loads(line) for line in run.stdout.splitlines()]
    except subprocess.TimeoutExpired as error:
        UNVERIFIED.append(f'Dependabot fixture tests timed out:\n{timeout_output(error)}')
        return True
    except (OSError, json.JSONDecodeError) as error:
        UNVERIFIED.append(f'could not run Dependabot fixture tests: {error}')
        return True
    outcomes = {event.get('Test'): event.get('Action') for event in events
                if event.get('Action') in ('pass', 'fail', 'skip') and isinstance(event.get('Test'), str)}
    targets = fixtures
    failures = [name for name, outcome in outcomes.items() if outcome == 'fail'
                and any(name == target or name.startswith(target + '/') for target in targets)]
    if failures:
        expect(False, f'Dependabot fixture tests failed: {", ".join(failures)}\n{run.stdout[-3000:]}')
    elif run.returncode != 0 or any(outcomes.get(name) != 'pass' for name in [*names, *fixtures]) \
            or any(outcome == 'skip' for outcome in outcomes.values()):
        UNVERIFIED.append('Dependabot fixture tests did not all run and pass')
    return True


def check_committed_tests():
    committed = committed_files()
    if check_dependabot_fixtures(committed):
        return
    files = [p for p in committed if p.endswith('_test.go') and Path(p).is_file()
             and not any(part.is_symlink() for part in (Path(p), *Path(p).parents))]
    expect(bool(files), 'no committed Go test file was changed')
    added_test = False
    for path in sorted(files):
        diff = subprocess.run(['git', 'diff', '--unified=0', head_sha(), 'HEAD', '--', path],
                              capture_output=True, text=True, check=True).stdout
        names = re.findall(r'^\+func\s+(Test[A-Za-z0-9_]+)\s*\(', diff, re.MULTILINE)
        added_test = added_test or bool(names)
        package = Path(path).parent.as_posix()
        package = '.' if package == '.' else f'./{package}'
        try:
            existed = bool(subprocess.run(['git', 'ls-tree', '-z', head_sha(), '--', path],
                                          capture_output=True, timeout=30, check=True).stdout)
            listed = subprocess.run(['go', 'list', '-json', package], capture_output=True, text=True, timeout=60,
                                    check=False)
            package_info = json.loads(listed.stdout)
            available = package_info.get('TestGoFiles', []) + package_info.get('XTestGoFiles', [])
            if listed.returncode != 0 or Path(path).name not in available:
                UNVERIFIED.append(f'edited test file was not selected for {path}')
                continue
            args = ['go', 'test', '-json', '-count=1']
            if names:
                args.extend(['-run', '^(' + '|'.join(names) + ')$'])
            run = subprocess.run([*args, package],
                                 capture_output=True, text=True, timeout=300, check=False)
            events = [json.loads(line) for line in run.stdout.splitlines()]
        except subprocess.TimeoutExpired as error:
            UNVERIFIED.append(f'committed tests timed out in {path}:\n{timeout_output(error)}')
            continue
        except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
            UNVERIFIED.append(f'could not run committed tests in {path}: {error}')
            continue
        outcomes = {event.get('Test'): event.get('Action') for event in events
                    if event.get('Action') in ('pass', 'fail', 'skip') and event.get('Test') in names}
        failures = [event['Test'] for event in events if event.get('Action') == 'fail'
                    and isinstance(event.get('Test'), str)
                    and any(event['Test'] == name or event['Test'].startswith(name + '/') for name in names)]
        if failures:
            expect(False, f'committed tests failed in {path}: {", ".join(failures)}\n{run.stdout[-3000:]}')
        elif run.returncode != 0:
            UNVERIFIED.append(f'committed tests did not pass in {path}:\n{run.stdout[-3000:]}\n{run.stderr[-1000:]}')
        elif ((names and any(outcomes.get(name) != 'pass' for name in names))
              or (not names and not any(event.get('Action') == 'pass' and event.get('Test') for event in events))
              or any(event.get('Action') == 'skip' and isinstance(event.get('Test'), str)
                     and (not names or any(event['Test'].startswith(name + '/') for name in names)) for event in events)):
            UNVERIFIED.append(f'committed tests did not all run and pass in {path}')
        # A new function in an existing file can accompany changes to other
        # tests or helpers. Running only the new function cannot cover those.
        if names and existed:
            try:
                whole = subprocess.run(['go', 'test', '-json', '-count=1', package], capture_output=True,
                                       text=True, timeout=300, check=False)
                all_events = [json.loads(line) for line in whole.stdout.splitlines()]
                if (whole.returncode != 0
                        or not any(e.get('Action') == 'pass' and e.get('Test') for e in all_events)
                        or any(e.get('Action') == 'skip' and e.get('Test') for e in all_events)):
                    UNVERIFIED.append(f'package for edited test file did not all run and pass: {path}')
            except subprocess.TimeoutExpired as error:
                UNVERIFIED.append(f'package for edited test file timed out in {path}:\n{timeout_output(error)}')
            except (OSError, json.JSONDecodeError) as error:
                UNVERIFIED.append(f'could not check package for edited test file {path}: {error}')
    if files and not added_test:
        UNVERIFIED.append('no newly added Go test function could be identified')

# Exercise the public parser without replacing any of the agent's tests.
def main():
    check_untracked()
    try:
        run = subprocess.run(["go", "run", str(Path(__file__).parent / "check.go")],
                             capture_output=True, text=True, timeout=540, check=False)
        if run.returncode:
            output = run.stdout + run.stderr
            if re.search(r'(?m)^(regular and group|two groups|duplicate regular|duplicate group): '
                         r'(got \d+ duplicate diagnostics|unexpected diagnostic:)', output):
                expect(False, f"Dependabot cases failed:\n{output}")
            else:
                UNVERIFIED.append(f"Dependabot cases could not complete:\n{output[-3000:]}")
    except subprocess.TimeoutExpired as error:
        UNVERIFIED.append(f'Dependabot cases timed out:\n{timeout_output(error)}')
    except OSError as error:
        UNVERIFIED.append(f"Dependabot cases could not run: {error}")
    expect(new_commit(), 'nothing was committed')
    expect(not dirty(), f'tracked changes were left uncommitted: {dirty()}')
    check_committed_tests()
    finish()


if __name__ == '__main__':
    main()
