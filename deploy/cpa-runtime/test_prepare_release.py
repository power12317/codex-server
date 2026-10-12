"""Verify release stamping keeps third-party packages and main metadata intact."""

from pathlib import Path
import runpy
import tempfile
import tomllib
import unittest

prepare_release = runpy.run_path(str(Path(__file__).with_name("prepare-release.py")))["prepare_release"]


class PrepareReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        crate = self.root / "codex-rs/app"
        crate.mkdir(parents=True)
        (crate / "Cargo.toml").write_text('[package]\nname = "codex-app"\nversion.workspace = true\n')
        self.manifest = crate.parent / "Cargo.toml"
        self.manifest.write_text('[workspace]\nmembers = ["app"]\n[workspace.package]\nversion = "0.0.0"\n')
        self.lock = crate.parent / "Cargo.lock"
        self.lock.write_text('''version = 4
[[package]]
name = "codex-app"
version = "0.0.0"
dependencies = ["external 0.0.0"]
[[package]]
name = "external"
version = "0.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "unchanged"
[[package]]
name = "fixture"
version = "1.0.0"
dependencies = ["codex-app 0.0.0"]
''')

    def test_stamps_only_workspace_packages_and_is_idempotent(self):
        prepare_release(self.root, "0.161.0")
        workspace = tomllib.loads(self.manifest.read_text())
        self.assertEqual(workspace["workspace"]["package"]["version"], "0.161.0")
        packages = tomllib.loads(self.lock.read_text())["package"]
        self.assertEqual(packages[0]["version"], "0.161.0")
        self.assertEqual(packages[1]["version"], "0.0.0")
        self.assertEqual(packages[1]["checksum"], "unchanged")
        self.assertEqual(packages[2]["dependencies"], ["codex-app 0.161.0"])
        before = self.manifest.read_bytes(), self.lock.read_bytes()
        prepare_release(self.root, "0.161.0")
        self.assertEqual(before, (self.manifest.read_bytes(), self.lock.read_bytes()))

    def test_rejects_non_release_versions_without_writing(self):
        before = self.manifest.read_bytes(), self.lock.read_bytes()
        for version in ("0.0.0", "0.162.0-alpha.20", "rust-v0.161.0", "latest"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                prepare_release(self.root, version)
        self.assertEqual(before, (self.manifest.read_bytes(), self.lock.read_bytes()))

    def test_stamps_implicit_path_members(self):
        implicit = self.root / "codex-rs/app/tests/common"
        implicit.mkdir(parents=True)
        (implicit / "Cargo.toml").write_text('[package]\nname = "app_test_support"\nversion.workspace = true\n')
        with self.lock.open("a") as lock:
            lock.write('[[package]]\nname = "app_test_support"\nversion = "0.0.0"\n')
        prepare_release(self.root, "0.161.0")
        packages = tomllib.loads(self.lock.read_text())["package"]
        self.assertEqual(packages[-1]["version"], "0.161.0")

    def test_stable_tag_manifest_with_development_lockfile(self):
        self.manifest.write_text(self.manifest.read_text().replace('"0.0.0"', '"0.162.1"'))
        implicit = self.root / "codex-rs/app/tests/common"
        implicit.mkdir(parents=True)
        (implicit / "Cargo.toml").write_text('[package]\nname = "app_test_support"\nversion.workspace = true\n')
        with self.lock.open("a") as lock:
            lock.write('[[package]]\nname = "app_test_support"\nversion = "0.0.0"\ndependencies = ["codex-app 0.0.0"]\n')
        prepare_release(self.root, "0.162.1")
        packages = tomllib.loads(self.lock.read_text())["package"]
        self.assertEqual(packages[0]["version"], "0.162.1")
        self.assertEqual(packages[1]["version"], "0.0.0")
        self.assertEqual(packages[1]["checksum"], "unchanged")
        self.assertEqual(packages[2]["dependencies"], ["codex-app 0.162.1"])
        self.assertEqual(packages[-1]["version"], "0.162.1")
        self.assertEqual(packages[-1]["dependencies"], ["codex-app 0.162.1"])
        before = self.manifest.read_bytes(), self.lock.read_bytes()
        prepare_release(self.root, "0.162.1")
        self.assertEqual(before, (self.manifest.read_bytes(), self.lock.read_bytes()))

    def test_stable_tag_rejects_unexpected_lockfile_version_without_writing(self):
        self.manifest.write_text(self.manifest.read_text().replace('"0.0.0"', '"0.162.1"'))
        self.lock.write_text(self.lock.read_text().replace('"0.0.0"', '"0.160.0"', 1))
        before = self.manifest.read_bytes(), self.lock.read_bytes()
        with self.assertRaises(ValueError):
            prepare_release(self.root, "0.162.1")
        self.assertEqual(before, (self.manifest.read_bytes(), self.lock.read_bytes()))


if __name__ == "__main__":
    unittest.main()
