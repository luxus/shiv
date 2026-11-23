import tempfile
import unittest
from pathlib import Path

import bench_diff_summary as bds


BASE_LOG = """\
move_mod/25             time:   [7.10 ms 7.20 ms 7.30 ms]
"""

CANDIDATE_OK = """\
move_mod/25             time:   [7.20 ms 7.30 ms 7.40 ms]
                        change: [-1.00% +1.39% +3.80%] (p = 0.25 > 0.05)
                        No change in performance detected.
"""

CANDIDATE_REGRESSED = """\
move_mod/25             time:   [10.10 ms 10.50 ms 10.90 ms]
                        change: [+40.0% +45.83% +52.0%] (p = 0.00 < 0.05)
                        Performance has regressed.
"""

CANDIDATE_INF = """\
move_mod/25             time:   [1.00 ms 1.10 ms 1.20 ms]
                        change: [+∞% +∞% +∞%] (p = 0.00 < 0.05)
                        Performance has regressed.
"""


class BenchDiffSummaryTests(unittest.TestCase):
    def _write(self, tmp: Path, name: str, content: str) -> str:
        path = tmp / name
        path.write_text(content, encoding="utf-8")
        return str(path)

    def test_empty_base_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            base = self._write(Path(td), "base.log", "")
            cand = self._write(Path(td), "cand.log", CANDIDATE_OK)
            self.assertEqual(bds.main(["--base", base, "--candidate", cand]), 2)

    def test_empty_candidate_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            base = self._write(Path(td), "base.log", BASE_LOG)
            cand = self._write(Path(td), "cand.log", "")
            self.assertEqual(bds.main(["--base", base, "--candidate", cand]), 2)

    def test_within_threshold_passes(self):
        with tempfile.TemporaryDirectory() as td:
            base = self._write(Path(td), "base.log", BASE_LOG)
            cand = self._write(Path(td), "cand.log", CANDIDATE_OK)
            self.assertEqual(bds.main(["--base", base, "--candidate", cand]), 0)

    def test_regression_beyond_threshold_fails(self):
        with tempfile.TemporaryDirectory() as td:
            base = self._write(Path(td), "base.log", BASE_LOG)
            cand = self._write(Path(td), "cand.log", CANDIDATE_REGRESSED)
            self.assertEqual(bds.main(["--base", base, "--candidate", cand]), 1)

    def test_infinite_regression_does_not_crash_and_fails_gate(self):
        with tempfile.TemporaryDirectory() as td:
            base = self._write(Path(td), "base.log", BASE_LOG)
            cand = self._write(Path(td), "cand.log", CANDIDATE_INF)
            self.assertEqual(bds.main(["--base", base, "--candidate", cand]), 1)

    def test_visible_preserves_content_before_trailing_cr(self):
        self.assertEqual(bds.visible("progress\rmove_mod/25\r\n"), "move_mod/25")
        self.assertEqual(bds.visible("move_mod/25\r"), "move_mod/25")


if __name__ == "__main__":
    unittest.main()
