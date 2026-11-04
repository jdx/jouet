import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / 'tasks/django-13344-container/fetch_debs.py'
spec = importlib.util.spec_from_file_location('django_fetch_debs', SCRIPT)
fetch_debs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch_debs)


class SnapshotFetchTest(unittest.TestCase):
    def test_uses_only_pinned_snapshot_urls_and_sha256_records(self):
        url = fetch_debs.SNAPSHOT + 'debian/' + fetch_debs.DATE + 'pool/main/example.deb'
        line = f"'{url}' example.deb 3 SHA256:{'a' * 64}"
        self.assertEqual(fetch_debs.parse(line), (url, 'example.deb', 3, 'a' * 64))
        self.assertEqual(fetch_debs.mirrors(url)[0], 'https://deb.debian.org/debian/pool/main/example.deb')
        with self.assertRaises(ValueError):
            fetch_debs.parse(f"'{url}' ../escape 3 SHA256:{'a' * 64}")
        with self.assertRaises(ValueError):
            fetch_debs.mirrors('https://example.invalid/example.deb')
        with self.assertRaises(ValueError):
            fetch_debs.parse(f"'{url}' example.deb 3 MD5Sum:{'a' * 32}")

    def test_rejects_a_bad_mirror_and_verifies_the_snapshot_fallback(self):
        good = b'correct package'
        url = fetch_debs.SNAPSHOT + 'debian/' + fetch_debs.DATE + 'pool/main/example.deb'
        record = (url, 'example.deb', len(good), hashlib.sha256(good).hexdigest())
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(fetch_debs, 'ARCHIVES', Path(directory)), \
                patch.object(fetch_debs.urllib.request, 'urlopen', side_effect=[io.BytesIO(b'bad'), io.BytesIO(good)]):
            fetch_debs.fetch(record)
            self.assertEqual((Path(directory) / 'example.deb').read_bytes(), good)
            self.assertFalse((Path(directory) / 'example.deb.partial').exists())

    def test_refuses_to_install_when_both_downloads_have_wrong_contents(self):
        url = fetch_debs.SNAPSHOT + 'debian/' + fetch_debs.DATE + 'pool/main/example.deb'
        record = (url, 'example.deb', 3, hashlib.sha256(b'yes').hexdigest())
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(fetch_debs, 'ARCHIVES', Path(directory)), \
                patch.object(fetch_debs.urllib.request, 'urlopen', side_effect=[io.BytesIO(b'bad'), io.BytesIO(b'bad')]):
            with self.assertRaises(RuntimeError):
                fetch_debs.fetch(record)
            self.assertFalse((Path(directory) / 'example.deb').exists())
            self.assertFalse((Path(directory) / 'example.deb.partial').exists())
