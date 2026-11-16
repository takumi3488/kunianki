import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class AndroidSmokeCliTests(unittest.TestCase):
    def test_missing_adb_exits_nonzero(self):
        source = Path(__file__).with_name("android_smoke.py")
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            script_directory = root / "scripts"
            script_directory.mkdir()
            script = script_directory / "android_smoke.py"
            shutil.copyfile(source, script)

            result = subprocess.run(
                [sys.executable, str(script)],
                cwd=root,
                env={"PATH": ""},
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertEqual(result.returncode, 1)


if __name__ == "__main__":
    unittest.main()
