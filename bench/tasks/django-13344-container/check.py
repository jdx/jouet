"""Check the historical Django task in an optional, isolated container."""
import os
import re
import shutil
import stat
import subprocess
import sys
import uuid
from pathlib import Path

root = Path(__file__).resolve().parent
sys.path.insert(0, str(root.parent))
from _check import UNVERIFIED, expect, finish, new_commit  # noqa: E402
from _swebench import FAIL, PASS, load, parse_django, save_log, verdict  # noqa: E402

SOURCE = root.parent / 'swebench-django-13344'
IMAGE = 'jouet-bench-django-13344:1'
WITNESSES = (
    'test_add (cache.tests.MemcachedCacheTests)',
    'test_add (cache.tests.PyLibMCCacheTests)',
)


def safe_path(rel):
    """Do not follow an agent-created symlink while placing hidden tests."""
    path = Path(rel)
    if path.is_absolute() or '..' in path.parts:
        raise OSError(f'unsafe checker path: {rel}')
    current = Path.cwd()
    for part in path.parts:
        current /= part
        if current.is_symlink():
            raise OSError(f'symlink in checker path: {rel}')
    return current


def run_check():
    spec = load(SOURCE)
    if not new_commit():
        expect(False, 'no committed change')
        return
    docker = os.environ.get('BENCH_DOCKER')
    if not docker:
        UNVERIFIED.append('Docker is unavailable to the benchmark checker')
        return
    try:
        image = subprocess.run([docker, 'image', 'inspect', IMAGE, '--format',
                                '{{.Id}} {{index .Config.Labels "dev.jouet.bench.django13344"}}'],
                               capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f'cannot inspect the checker image: {error}')
        return
    fields = image.stdout.split()
    if image.returncode or len(fields) != 2 or not re.fullmatch(r'sha256:[0-9a-f]{64}', fields[0]) or fields[1] != '1':
        detail = image.stderr.strip()
        UNVERIFIED.append(f'cannot use checker image {IMAGE}: {detail or "missing or unexpected image"}; build it as documented')
        return
    image_id = fields[0]

    originals = {}
    try:
        for rel in dict.fromkeys(spec['test_files'] + spec.get('deleted_test_files', [])):
            path = safe_path(rel)
            if path.exists() and not path.is_file():
                raise OSError(f'cannot preserve non-file {rel}')
            originals[rel] = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) if path.exists() else None
    except OSError as error:
        UNVERIFIED.append(f'cannot preserve checker inputs: {error}')
        return

    container = 'jouet-django-check-' + uuid.uuid4().hex
    try:
        for rel in spec['test_files']:
            target = safe_path(rel)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(SOURCE / 'hidden' / rel, target)
        for rel in spec.get('deleted_test_files', []):
            safe_path(rel).unlink(missing_ok=True)
        command = [
            docker, 'run', '--rm', '--name', container, '--network', 'none',
            '--mount', f'type=bind,src={Path.cwd().resolve()},dst=/workspace,readonly',
            '--mount', f'type=bind,src={root},dst=/config,readonly',
            '--workdir', '/workspace',
            '--env', 'PYTHONPATH=/workspace:/workspace/tests:/config',
            '--env', 'PYTHONDONTWRITEBYTECODE=1', image_id,
            'sh', '-c', 'memcached -u nobody -d && python tests/runtests.py --verbosity 2 '
            '--settings=probe_settings --parallel 1 cache.tests deprecation.test_middleware_mixin',
        ]
        try:
            proc = subprocess.run(command, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired as error:
            try:
                cleanup = subprocess.run([docker, 'rm', '--force', container],
                                         capture_output=True, timeout=30, check=False)
                if cleanup.returncode:
                    UNVERIFIED.append('container cleanup could not be confirmed')
            except (OSError, subprocess.TimeoutExpired) as cleanup_error:
                UNVERIFIED.append(f'container cleanup could not be confirmed: {cleanup_error}')
            log = ''.join(part.decode(errors='replace') if isinstance(part, bytes) else part or ''
                          for part in (error.stdout, error.stderr))
            save_log(spec, log)
            failed = [name for name, status in parse_django(log).items() if status == FAIL]
            expect(not failed, 'tests failed before timeout: ' + ', '.join(failed))
            UNVERIFIED.append('containerized tests timed out')
            return
        except OSError as error:
            UNVERIFIED.append(f'container could not start: {error}')
            return
        log = proc.stdout + proc.stderr
        save_log(spec, log)
        statuses = parse_django(log)
        failed = [name for name, status in statuses.items() if status == FAIL]
        expect(not failed, 'tests reported failures: ' + ', '.join(failed))
        if not statuses:
            UNVERIFIED.append('container produced no named test results')
            return
        problems, unverified = verdict(spec, statuses)
        expect(not problems, 'required tests failed: ' + ', '.join(problems))
        UNVERIFIED.extend(unverified)
        for witness in WITNESSES:
            if statuses.get(witness) != PASS:
                UNVERIFIED.append(f'cache backend was not verified: {witness}')
        if proc.returncode and not failed and not problems:
            UNVERIFIED.append(f'container exited {proc.returncode} without a named failure')
        print(f'checker image: {image_id}; {sum(s == PASS for s in statuses.values())} '
              f'of {len(statuses)} named results passed')
        print(f'full log: {Path(os.environ.get("TMPDIR", "/tmp")) / (spec["instance_id"] + ".test.log")}')
    except OSError as error:
        UNVERIFIED.append(f'could not place checker inputs: {error}')
    finally:
        for rel, original in originals.items():
            try:
                path = safe_path(rel)
                if original is None:
                    path.unlink(missing_ok=True)
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(original[0])
                    os.chmod(path, original[1])
            except OSError as error:
                UNVERIFIED.append(f'could not restore checker input {rel}: {error}')


if __name__ == '__main__':
    run_check()
    finish()
