"""Regression for the trusted runtime-input producer's exact numeric API route."""
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("runtime_input", Path(__file__).with_name("prepare-runtime-input.py"))
RUNTIME = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNTIME)
SUPPORT_SPEC = importlib.util.spec_from_file_location("content_runtime", Path(__file__).with_name("content-runtime.py"))
SUPPORT = importlib.util.module_from_spec(SUPPORT_SPEC)
SUPPORT_SPEC.loader.exec_module(SUPPORT)


class RuntimeInputTests(unittest.TestCase):
    def binding(self):
        payload = b"inert owned runtime download fixture"
        binding = dict(repository="Darkaxt/mailcraft", tag="v0.11.2", assetId=123,
            name="mailcraft-0.11.2-windows-x64-content-runtime.zip", bytes=len(payload),
            sha256=hashlib.sha256(payload).hexdigest())
        metadata = dict(id=123, name=binding["name"], state="uploaded", size=len(payload), digest="sha256:" + binding["sha256"])
        return payload, binding, metadata

    def test_unpublished_input_uses_exact_asset_api_without_tag_lookup(self):
        payload, binding, metadata = self.binding()
        def downloaded(argv, stdout, check):
            self.assertEqual(argv[:5], ["gh", "api", "--hostname", "github.com", "repos/Darkaxt/mailcraft/releases/assets/123"])
            self.assertIn("Accept: application/octet-stream", argv)
            self.assertTrue(check)
            stdout.write(payload)
            return subprocess.CompletedProcess(argv, 0)
        with tempfile.TemporaryDirectory(prefix="mailcraft-runtime-api-") as temporary:
            root = Path(temporary) / "download"
            with patch.object(RUNTIME.subprocess, "check_output", return_value=json.dumps(metadata)) as lookup, patch.object(RUNTIME.subprocess, "run", side_effect=downloaded) as download:
                result = RUNTIME.download_runtime({"releaseInput":binding}, "0.11.2", root)
            self.assertEqual(result.read_bytes(), payload)
            self.assertEqual(lookup.call_args.args[0][-1], "repos/Darkaxt/mailcraft/releases/assets/123")
            self.assertEqual(download.call_count, 1)

    def test_changed_remote_identity_is_refused_before_download(self):
        _, binding, metadata = self.binding()
        for key, value in (("id",124),("name","other.zip"),("state","starter"),("size",0),("digest","sha256:"+"0"*64)):
            with self.subTest(key=key), tempfile.TemporaryDirectory(prefix="mailcraft-runtime-api-") as temporary:
                root = Path(temporary)/"download"
                with patch.object(RUNTIME.subprocess,"check_output",return_value=json.dumps({**metadata,key:value})), patch.object(RUNTIME.subprocess,"run") as download:
                    with self.assertRaises(ValueError):
                        RUNTIME.download_runtime({"releaseInput":binding},"0.11.2",root)
                self.assertFalse(root.exists())
                download.assert_not_called()

    def test_wrong_release_input_is_refused_without_network(self):
        _, binding, _ = self.binding()
        for changed in ({**binding,"assetId":True},{**binding,"tag":"v0.11.1"},{**binding,"repository":"other/mailcraft"},{**binding,"sha256":"invalid"}):
            with patch.object(RUNTIME.subprocess,"check_output") as lookup:
                with self.assertRaises(ValueError):
                    RUNTIME.download_runtime({"releaseInput":changed},"0.11.2",Path("unused-runtime-fixture"))
                lookup.assert_not_called()

    def test_changed_payload_is_not_accepted_or_retried(self):
        _, binding, metadata = self.binding()
        def corrupted(argv, stdout, check):
            stdout.write(b"changed payload")
        with tempfile.TemporaryDirectory(prefix="mailcraft-runtime-api-") as temporary:
            root=Path(temporary)/"download"
            with patch.object(RUNTIME.subprocess,"check_output",return_value=json.dumps(metadata)), patch.object(RUNTIME.subprocess,"run",side_effect=corrupted) as download:
                with self.assertRaises(ValueError):
                    RUNTIME.download_runtime({"releaseInput":binding},"0.11.2",root)
            self.assertEqual(download.call_count,1)
            self.assertEqual((root/binding["name"]).read_bytes(),b"changed payload")


class CanonicalSourceBindingTests(unittest.TestCase):
    def fixture(self, folder, mode):
        root=Path(folder)
        config=root/'native/craft-content'
        config.mkdir(parents=True)
        shutil.copyfile(Path(__file__).resolve().parents[2]/'native/craft-content/copy-runtime.py',config/'copy-runtime.py')
        source=b'fn worker() {}\n'
        (root/'worker.rs').write_bytes(source)
        runtime=root/'runtime'
        runtime.mkdir()
        (runtime/'runtime.json').write_bytes(b'{}')
        original=source if mode=='lf' else source.replace(b'\n',b'\r\n')
        selection=dict(version=1,platform='windows-x64',manifestSha256=hashlib.sha256(b'{}').hexdigest(),
            sourceFiles={'worker.rs':hashlib.sha256(source).hexdigest()},
            qualifiedSourceFiles={'worker.rs':hashlib.sha256(original).hexdigest()},
            qualifiedLineEndings={'worker.rs':mode})
        lock=config/'release-runtime.json'
        lock.write_text(json.dumps(selection),encoding='utf-8')
        return root,runtime,lock,selection

    def test_canonical_bytes_reconstruct_both_exact_qualified_forms(self):
        for mode in ('lf','crlf'):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory(prefix='mailcraft-source-binding-') as folder:
                root,runtime,_,_=self.fixture(folder,mode)
                with patch.object(SUPPORT,'ROOT',root):
                    self.assertTrue(callable(SUPPORT.selected(runtime).copy_runtime))

    def test_relabelling_changed_source_does_not_preserve_qualification(self):
        with tempfile.TemporaryDirectory(prefix='mailcraft-source-binding-') as folder:
            root,runtime,lock,selection=self.fixture(folder,'crlf')
            changed=b'fn worker() { changed(); }\n'
            (root/'worker.rs').write_bytes(changed)
            selection['sourceFiles']['worker.rs']=hashlib.sha256(changed).hexdigest()
            lock.write_text(json.dumps(selection),encoding='utf-8')
            with patch.object(SUPPORT,'ROOT',root):
                with self.assertRaisesRegex(ValueError,'exact qualified input'):
                    SUPPORT.selected(runtime)

    def test_noncanonical_checkout_is_not_silently_normalized(self):
        with tempfile.TemporaryDirectory(prefix='mailcraft-source-binding-') as folder:
            root,runtime,_,_=self.fixture(folder,'crlf')
            (root/'worker.rs').write_bytes(b'fn worker() {}\r\n')
            with patch.object(SUPPORT,'ROOT',root):
                with self.assertRaises(ValueError):
                    SUPPORT.selected(runtime)


if __name__ == "__main__":
    unittest.main()
