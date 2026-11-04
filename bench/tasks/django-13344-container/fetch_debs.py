"""Fetch snapshot-selected Debian packages from a mirror, verifying snapshot hashes."""
import concurrent.futures
import hashlib
import os
from pathlib import Path
import shlex
import sys
import urllib.error
import urllib.request


SNAPSHOT = 'https://snapshot.debian.org/archive/'
DATE = '20261103T000000Z/'
MIRRORS = {'debian/': 'https://deb.debian.org/debian/',
           'debian-security/': 'https://deb.debian.org/debian-security/'}
ARCHIVES = Path('/var/cache/apt/archives')


def parse(line):
    if not line.startswith("'"):
        return None
    words = shlex.split(line)
    if len(words) != 4 or not words[3].startswith('SHA256:'):
        raise ValueError('apt did not provide a SHA-256 package digest')
    url, name, size, digest = words
    if Path(name).name != name or not size.isdigit() or len(digest[7:]) != 64:
        raise ValueError('apt provided an invalid package record')
    return url, name, int(size), digest[7:]


def mirrors(url):
    for archive, mirror in MIRRORS.items():
        prefix = SNAPSHOT + archive + DATE
        if url.startswith(prefix):
            return [mirror + url[len(prefix):], url]
    raise ValueError('package URL is not from the pinned snapshot')


def fetch(record):
    url, name, size, expected = record
    target = ARCHIVES / name
    temporary = target.with_suffix(target.suffix + '.partial')
    errors = []
    try:
        for candidate in mirrors(url):
            try:
                digest = hashlib.sha256()
                count = 0
                with urllib.request.urlopen(candidate, timeout=30) as response, temporary.open('wb') as output:
                    for block in iter(lambda: response.read(1024 * 1024), b''):
                        digest.update(block)
                        count += len(block)
                        output.write(block)
                if count != size or digest.hexdigest() != expected:
                    raise ValueError('download does not match the signed snapshot')
                os.replace(temporary, target)
                return
            except (OSError, ValueError, urllib.error.URLError) as error:
                errors.append(f'{candidate}: {error}')
        raise RuntimeError(f'cannot fetch {name}: ' + '; '.join(errors))
    finally:
        temporary.unlink(missing_ok=True)


def main():
    records = [record for line in sys.stdin if (record := parse(line))]
    if not records:
        raise SystemExit('apt listed no packages to install')
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(fetch, records))
    print(f'verified {len(records)} packages against the pinned Debian snapshot')


if __name__ == '__main__':
    main()
