import subprocess
import sys
import json
import re
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, head_sha, new_commit  # noqa: E402


def timeout_output(error):
    def text(value):
        return value.decode(errors='replace') if isinstance(value, bytes) else value or ''
    return (text(error.stdout) + text(error.stderr))[-3000:]


def check_committed_tests():
    files = [p for p in committed_files() if p.endswith('_test.go') and Path(p).is_file()
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
            if not names:
                listed = subprocess.run(['go', 'list', '-json', package], capture_output=True, text=True, timeout=60)
                package_info = json.loads(listed.stdout)
                available = package_info.get('TestGoFiles', []) + package_info.get('XTestGoFiles', [])
                if listed.returncode != 0 or Path(path).name not in available:
                    UNVERIFIED.append(f'edited test file was not selected for {path}')
                    continue
            args = ['go', 'test', '-json', '-count=1']
            if names:
                args.extend(['-run', '^(' + '|'.join(names) + ')$'])
            run = subprocess.run([*args, package],
                                 capture_output=True, text=True, timeout=300)
            events = [json.loads(line) for line in run.stdout.splitlines()]
        except subprocess.TimeoutExpired as error:
            UNVERIFIED.append(f'committed tests timed out in {path}:\n{timeout_output(error)}')
            continue
        except (OSError, json.JSONDecodeError) as error:
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
    if files and not added_test:
        UNVERIFIED.append('no newly added Go test function could be identified')

# Exercise the public parser without replacing any of the agent's tests.
def main():
    try:
        run = subprocess.run(["go", "run", str(Path(__file__).parent / "check.go")],
                             capture_output=True, text=True, timeout=540)
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
