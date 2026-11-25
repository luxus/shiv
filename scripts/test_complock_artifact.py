"""Unit tests for scripts/complock_artifact.py."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import pathlib
import subprocess
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "complock_artifact", pathlib.Path(__file__).parent / "complock_artifact.py"
)
complock_artifact = importlib.util.module_from_spec(_SPEC)
assert _SPEC.loader is not None
_SPEC.loader.exec_module(complock_artifact)


def _git(repo: pathlib.Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )


class ModelAndTagParsing(unittest.TestCase):
    def test_reads_model_level_constant(self) -> None:
        self.assertEqual(
            complock_artifact.read_model_level("pub const MODEL_LEVEL: u64 = 3;\n"), 3
        )
        with self.assertRaises(complock_artifact.ArtifactError):
            complock_artifact.read_model_level("pub const OTHER: u64 = 1;")
        with self.assertRaises(complock_artifact.ArtifactError):
            complock_artifact.read_model_level("pub const MODEL_LEVEL: u64 = 0;")

    def test_parses_only_canonical_semver_tags(self) -> None:
        self.assertEqual(complock_artifact.parse_semver_tag("v0.7.0"), (0, 7, 0))
        self.assertEqual(complock_artifact.parse_semver_tag("v1.12.3"), (1, 12, 3))
        for ignored in ("complock-data", "v0.7.0-rc1", "v01.2.3", "0.7.0", "v1.2"):
            self.assertIsNone(complock_artifact.parse_semver_tag(ignored), ignored)

    def test_resolves_lowest_compatible_tag(self) -> None:
        levels = [
            ("complock-data", 99),
            ("v0.6.0", 1),
            ("v0.10.0", 2),
            ("v0.7.0", 2),
            ("v0.6.1", 1),
        ]
        self.assertEqual(complock_artifact.resolve_requires_shiv(1, levels), "0.6.0")
        self.assertEqual(complock_artifact.resolve_requires_shiv(2, levels), "0.7.0")
        self.assertIsNone(complock_artifact.resolve_requires_shiv(3, levels))


class GitTagScanAndAssembly(unittest.TestCase):
    def _init_repo(self, root: pathlib.Path) -> pathlib.Path:
        repo = root / "repo"
        repo.mkdir()
        _git(repo, "init")
        _git(repo, "config", "user.name", "Test")
        _git(repo, "config", "user.email", "test@example.com")
        complock_rs = repo / "crates/shiv-core/src/complock.rs"
        complock_rs.parent.mkdir(parents=True)

        # Older tag predating MODEL_LEVEL must be skipped gracefully.
        complock_rs.write_text("pub const PACK_APP_VERSION: u64 = 1;\n", encoding="utf-8")
        _git(repo, "add", ".")
        _git(repo, "commit", "-m", "v0.6.1")
        _git(repo, "tag", "v0.6.1")

        # Tag supporting model 1.
        complock_rs.write_text("pub const MODEL_LEVEL: u64 = 1;\n", encoding="utf-8")
        _git(repo, "add", ".")
        _git(repo, "commit", "-m", "v0.7.0")
        _git(repo, "tag", "v0.7.0")

        # Mutating prerelease tag that must never count as a Shiv app tag.
        _git(repo, "tag", "complock-data")
        return repo

    def test_scan_and_assemble_lifecycle(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            repo = self._init_repo(root)
            head_complock = repo / "crates/shiv-core/src/complock.rs"

            pack_bytes = b'{"game":"6860","base":[],"option":[{"id":"a"}],"files":{},"gameinfo":"g"}'
            pack_path = root / "fresh-pack-data.json"
            pack_path.write_bytes(pack_bytes)
            sha = hashlib.sha256(pack_bytes).hexdigest()

            # 1. Extract options baseline.
            extracted = root / "options.json"
            self.assertEqual(
                complock_artifact.main(["extract-options", str(pack_path), str(extracted)]), 0
            )
            self.assertNotIn("gameinfo", json.loads(extracted.read_text(encoding="utf-8")))

            # 2. Initial assemble at model 1 finds v0.7.0.
            out1 = root / "out1"
            self.assertEqual(
                complock_artifact.main(
                    [
                        "assemble",
                        "--pack-data",
                        str(pack_path),
                        "--game",
                        "6860",
                        "--fetched",
                        "2026-11-25",
                        "--head-complock",
                        str(head_complock),
                        "--repo",
                        str(repo),
                        "--model-bumped",
                        "false",
                        "--out-dir",
                        str(out1),
                    ]
                ),
                0,
            )
            meta1 = json.loads((out1 / "meta.json").read_text(encoding="utf-8"))
            self.assertEqual(meta1["model"], 1)
            self.assertEqual(meta1["requires_shiv"], "0.7.0")
            self.assertEqual(meta1["source_sha256"], sha)
            self.assertEqual((out1 / "pack-data.json").read_bytes(), pack_bytes)
            sums1 = (out1 / "SHA256SUMS").read_text(encoding="utf-8")
            self.assertIn(f"{sha}  pack-data.json", sums1)
            self.assertIn("meta.json", sums1)

            # 3. Same pack again -> publish_needed is false, fetched preserved even with new date.
            out2 = root / "out2"
            self.assertEqual(
                complock_artifact.main(
                    [
                        "assemble",
                        "--pack-data",
                        str(pack_path),
                        "--game",
                        "6860",
                        "--fetched",
                        "2026-11-26",
                        "--head-complock",
                        str(head_complock),
                        "--repo",
                        str(repo),
                        "--model-bumped",
                        "false",
                        "--prev-meta",
                        str(out1 / "meta.json"),
                        "--out-dir",
                        str(out2),
                    ]
                ),
                0,
            )
            meta2 = json.loads((out2 / "meta.json").read_text(encoding="utf-8"))
            self.assertEqual(meta2["fetched"], "2026-11-25")
            self.assertFalse(complock_artifact.publish_needed(meta1, meta2))

            # 4. Upstream introduces a new key (model bump to 2): requires_shiv is null.
            pack_m2 = b'{"game":"6861","base":[],"option":[{"id":"a","new_k":1}],"files":{},"gameinfo":"g"}'
            pack_m2_path = root / "m2.json"
            pack_m2_path.write_bytes(pack_m2)
            out3 = root / "out3"
            self.assertEqual(
                complock_artifact.main(
                    [
                        "assemble",
                        "--pack-data",
                        str(pack_m2_path),
                        "--game",
                        "6861",
                        "--fetched",
                        "2026-11-26",
                        "--head-complock",
                        str(head_complock),
                        "--repo",
                        str(repo),
                        "--model-bumped",
                        "true",
                        "--prev-meta",
                        str(out1 / "meta.json"),
                        "--out-dir",
                        str(out3),
                    ]
                ),
                0,
            )
            meta3 = json.loads((out3 / "meta.json").read_text(encoding="utf-8"))
            self.assertEqual(meta3["model"], 2)
            self.assertIsNone(meta3["requires_shiv"])
            self.assertTrue(complock_artifact.publish_needed(meta1, meta3))

            # 5. Later Shiv v0.8.0 tags MODEL_LEVEL = 2; same pack re-run resolves requires_shiv without bumping model.
            head_complock.write_text("pub const MODEL_LEVEL: u64 = 2;\n", encoding="utf-8")
            _git(repo, "add", ".")
            _git(repo, "commit", "-m", "v0.8.0")
            _git(repo, "tag", "v0.8.0")

            out4 = root / "out4"
            self.assertEqual(
                complock_artifact.main(
                    [
                        "assemble",
                        "--pack-data",
                        str(pack_m2_path),
                        "--game",
                        "6861",
                        "--fetched",
                        "2026-11-27",
                        "--head-complock",
                        str(head_complock),
                        "--repo",
                        str(repo),
                        "--model-bumped",
                        "false",
                        "--prev-meta",
                        str(out3 / "meta.json"),
                        "--out-dir",
                        str(out4),
                    ]
                ),
                0,
            )
            meta4 = json.loads((out4 / "meta.json").read_text(encoding="utf-8"))
            self.assertEqual(meta4["model"], 2)
            self.assertEqual(meta4["requires_shiv"], "0.8.0")
            self.assertEqual(meta4["fetched"], "2026-11-26")
            self.assertTrue(complock_artifact.publish_needed(meta3, meta4))


if __name__ == "__main__":
    unittest.main()
