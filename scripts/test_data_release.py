"""Unit tests for scripts/data_release.py.

CI discovers this file (`python3 -m unittest discover -s scripts`).
The cover-next-version and cmp-version tables mirror the ones in
docs/perf-results.md's spirit: every rule and refusal named.
"""

from __future__ import annotations

import importlib.util
import pathlib
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "data_release", pathlib.Path(__file__).parent / "data_release.py"
)
data_release = importlib.util.module_from_spec(_SPEC)
assert _SPEC.loader is not None
_SPEC.loader.exec_module(data_release)


class NextVersion(unittest.TestCase):
    def assert_next(self, current: str, subjects: list[str], expected: str) -> None:
        got = data_release.next_version(current=current, subjects=subjects)
        self.assertEqual(got, expected)

    def test_a_feature_bumps_the_minor(self) -> None:
        self.assert_next("0.6.0", ["fix(ui): one", "feat(ui,core): two"], "0.7.0")

    def test_only_fixes_bump_the_patch(self) -> None:
        self.assert_next("0.6.0", ["fix(ui): one", "perf(ui): two"], "0.6.1")

    def test_a_breaking_subject_bumps_the_major(self) -> None:
        self.assert_next("0.6.0", ["feat!: drop the old format"], "1.0.0")
        self.assert_next("0.6.0", ["fix(api)!: refuse the old url", "feat(ui): x"], "1.0.0")

    def test_a_breaking_footer_line_counts(self) -> None:
        self.assert_next(
            "0.6.0",
            ["feat(ui): one", "BREAKING CHANGE: the pack format grew a field"],
            "1.0.0",
        )

    def test_internal_types_and_prose_release_nothing(self) -> None:
        self.assert_next(
            "0.6.0",
            [
                "chore: tidy",
                "chore(complock): gameinfo build 6861 (deadlockconfig.com)",
                "ci(release): generate the notes",
                "test(core): more cover",
                "docs(readme): shorter",
                "release 0.6.0",
            ],
            "none",
        )
        self.assert_next("0.6.0", ["one line with no type"], "none")
        self.assert_next("0.6.0", [], "none")

    def test_complock_watch_data_release_plan_removed(self) -> None:
        self.assertFalse(hasattr(data_release, "plan"))
        self.assertFalse(hasattr(data_release, "marker"))
        self.assertFalse(hasattr(data_release, "MARKER"))

    def test_a_scope_with_a_bang_before_the_colon_is_breaking(self) -> None:
        self.assert_next("0.6.0", ["feat(core)!: one"], "1.0.0")

    def test_a_malformed_current_refuses(self) -> None:
        with self.assertRaises(data_release.ReleaseError):
            data_release.next_version(current="0.6", subjects=["feat: one"])


class CmpVersion(unittest.TestCase):
    def cmp(self, current: str, tag: str) -> str:
        return data_release.cmp_version(current=current, tag=tag)

    def test_same_ahead_and_behind(self) -> None:
        self.assertEqual(self.cmp("0.6.0", "v0.6.0"), "same")
        self.assertEqual(self.cmp("0.7.0", "v0.6.0"), "ahead")
        self.assertEqual(self.cmp("0.6.0", "v0.7.0"), "behind")

    def test_versions_compare_numerically(self) -> None:
        self.assertEqual(self.cmp("0.10.0", "v0.9.9"), "ahead")

    def test_the_tag_v_is_optional(self) -> None:
        self.assertEqual(self.cmp("0.6.0", "0.6.0"), "same")


if __name__ == "__main__":
    unittest.main()
