# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Protect pinned input identity and published/uncertain release state."""
import copy
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

SPEC = importlib.util.spec_from_file_location('refresh_draft', Path(__file__).with_name('refresh-draft.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class DraftTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.commit = 'a' * 40
        self.name = 'mailcraft-0.11.2-windows-x64-content-runtime.zip'
        data = b'qualified runtime'
        sha = hashlib.sha256(data).hexdigest()
        self.binding = dict(repository='Darkaxt/mailcraft', tag='v0.11.2', releaseId=10, assetId=20, name=self.name, bytes=len(data), sha256=sha)
        (self.directory / self.name).write_bytes(data)
        (self.directory / 'mailcraft-portable.zip').write_bytes(b'new package')
        (self.directory / 'SHA256SUMS.txt').write_text(sha + '  ' + self.name + '\n', encoding='utf-8')
        self.release = dict(id=10, draft=True, prerelease=False, tag_name='v0.11.2', target_commitish=self.commit,
                            name='MailCraft v0.11.2', body='final notes',
                            upload_url='https://uploads.github.com/repos/Darkaxt/mailcraft/releases/10/assets{?name,label}')
        self.remote = [dict(id=20, name=self.name, size=len(data), digest='sha256:' + sha, state='uploaded'),
                       dict(id=21, name='mailcraft-portable.zip', size=3, digest='sha256:' + '0' * 64, state='uploaded')]
        self.original = copy.deepcopy(self.remote[0])
        self.calls = []
        self.fail_upload = False

    def api(self, endpoint, *, method='GET', upload=None, pages=False):
        self.calls.append((method, endpoint))
        if method == 'GET':
            return [[copy.deepcopy(item)] for item in self.remote] if pages else copy.deepcopy(self.release)
        if method == 'DELETE':
            self.remote = [item for item in self.remote if item['id'] != int(endpoint.rsplit('/', 1)[-1])]
            return None
        self.assertEqual(method, 'POST')
        if self.fail_upload:
            raise RuntimeError('original upload reply unavailable')
        name = parse_qs(urlsplit(endpoint).query)['name'][0]
        item = dict(id=100 + len(self.calls), name=name, size=upload.stat().st_size,
                    digest='sha256:' + MODULE.digest(upload), state='uploaded')
        self.remote.append(item)
        return copy.deepcopy(item)

    def run_refresh(self):
        with patch.object(MODULE, 'api', self.api):
            return MODULE.refresh(self.binding, 'Darkaxt/mailcraft', '0.11.2', self.commit, self.directory, 'final notes', False)

    def test_refresh_preserves_exact_input_and_uses_numeric_draft_without_tag_lookup(self):
        result = self.run_refresh()
        self.assertEqual(next(item for item in self.remote if item['id'] == 20), self.original)
        self.assertEqual(result['preservedRuntimeAssetId'], 20)
        self.assertFalse(result['published'])
        self.assertFalse(any('/tags/' in endpoint for _, endpoint in self.calls))
        self.assertFalse(any(method == 'DELETE' and endpoint.endswith('/20') for method, endpoint in self.calls))
        self.assertFalse(any(method == 'POST' and parse_qs(urlsplit(endpoint).query)['name'][0] == self.name for method, endpoint in self.calls))
        self.assertEqual({item['name'] for item in self.remote}, {path.name for path in self.directory.iterdir()})

    def test_published_or_changed_source_is_never_mutated(self):
        for field, value in [('draft', False), ('target_commitish', 'b' * 40), ('body', 'owner changed notes')]:
            with self.subTest(field=field):
                original = self.release[field]
                self.release[field] = value
                with self.assertRaises(ValueError):
                    self.run_refresh()
                self.assertTrue(all(method == 'GET' for method, _ in self.calls))
                self.release[field] = original
                self.calls.clear()

    def test_changed_numeric_input_or_digest_is_preserved(self):
        for field, value in [('id', 30), ('digest', 'sha256:' + 'f' * 64), ('size', 99)]:
            with self.subTest(field=field):
                original = self.remote[0][field]
                self.remote[0][field] = value
                with self.assertRaises(ValueError):
                    self.run_refresh()
                self.assertTrue(all(method == 'GET' for method, _ in self.calls))
                self.remote[0][field] = original
                self.calls.clear()

    def test_changed_local_input_fails_before_remote_mutation(self):
        (self.directory / self.name).write_bytes(b'changed')
        with self.assertRaises(ValueError):
            self.run_refresh()
        self.assertEqual(self.calls, [])

    def test_unrelated_owner_asset_is_not_swept(self):
        self.remote.append(dict(id=90, name='owner-note.txt'))
        with self.assertRaises(ValueError):
            self.run_refresh()
        self.assertTrue(all(method == 'GET' for method, _ in self.calls))
        self.assertIn('owner-note.txt', [item['name'] for item in self.remote])

    def test_unknown_upload_is_not_retried_and_input_survives(self):
        self.fail_upload = True
        with self.assertRaisesRegex(RuntimeError, 'reply unavailable'):
            self.run_refresh()
        self.assertEqual(len([method for method, _ in self.calls if method == 'POST']), 1)
        self.assertEqual(next(item for item in self.remote if item['id'] == 20), self.original)

    def test_upload_transport_streams_the_exact_file_length(self):
        payload = self.directory / self.name
        endpoint = 'https://uploads.github.com/repos/Darkaxt/mailcraft/releases/10/assets?name=runtime.zip'
        with patch.object(MODULE.subprocess, 'check_output', return_value='{"id":100}') as invoke:
            self.assertEqual(MODULE.api(endpoint, method='POST', upload=payload), {'id': 100})
        arguments = invoke.call_args.args[0]
        self.assertIn(endpoint, arguments)
        self.assertIn('Content-Length: ' + str(payload.stat().st_size), arguments)
        self.assertEqual(arguments[arguments.index('--input') + 1], str(payload))
        self.assertEqual(invoke.call_count, 1)


if __name__ == '__main__':
    unittest.main()
