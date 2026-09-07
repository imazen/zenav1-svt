"""Regression checks for differences hidden by the tree diagnostic."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class PaletteComparison(unittest.TestCase):
    def compare(self, c_size, port_size):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "c").write_text(f"CTREE mi=(6,0) pal={c_size}\n")
            (root / "rs").write_text(f"PTREE mi=(6,0) pal={port_size}\n")
            return subprocess.run(
                [sys.executable, str(Path(__file__).with_name("tree_diff.py")),
                 str(root / "c"), str(root / "rs")],
                capture_output=True, text=True, check=False,
            )

    def test_native_lossless_palette_size_flip(self):
        # screen 64x64, QP 0, preset 4, native 10-bit: the old diagnostic
        # returned success despite C selecting four colors and Rust two.
        result = self.compare(4, 2)
        self.assertEqual(result.returncode, 1)
        self.assertIn("pal: C=4 port=2", result.stdout)

    def test_palette_presence_flip(self):
        for c_size, port_size in [(0, 2), (2, 0)]:
            with self.subTest(c_size=c_size, port_size=port_size):
                self.assertEqual(self.compare(c_size, port_size).returncode, 1)

    def test_matching_palette(self):
        self.assertEqual(self.compare(4, 4).returncode, 0)


if __name__ == "__main__":
    unittest.main()
