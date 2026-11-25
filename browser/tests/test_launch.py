import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import launch
from startup_trace import StartupTrace


class SnapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def executable(self, name, text):
        path = self.root / name
        path.write_text(text)
        path.chmod(0o700)
        return path

    def test_a_wrapper_that_execs_the_snap_is_snap(self):
        wrapper = self.executable("firefox", '#!/bin/sh\nexec /snap/bin/firefox "$@"\n')
        with patch("launch.sys.platform", "linux"):
            self.assertTrue(launch.snap_firefox(str(wrapper)))

    def test_a_link_to_the_snap_launcher_is_snap(self):
        link = self.root / "firefox"
        link.symlink_to("/usr/bin/snap")
        with patch("launch.sys.platform", "linux"):
            self.assertTrue(launch.snap_firefox(str(link)))

    def test_other_installations_are_not_snap(self):
        binary = self.executable("firefox-bin", "\x7fELF fixture")
        script = self.executable("firefox-sh", "#!/bin/sh\nexec /usr/lib/firefox/firefox \"$@\"\n")
        with patch("launch.sys.platform", "linux"):
            self.assertFalse(launch.snap_firefox(str(binary)))
            self.assertFalse(launch.snap_firefox(str(script)))
            self.assertFalse(launch.snap_firefox(""))
            self.assertFalse(launch.snap_firefox(str(self.root / "missing")))
        with patch("launch.sys.platform", "darwin"):
            self.assertFalse(launch.snap_firefox("/snap/bin/firefox"))

    def test_only_snap_firefox_moves_its_profile_out_of_hidden_folders(self):
        default = self.root / ".local/share/chilpa/plugin-desktop/firefox"
        wrapper = self.executable("firefox", '#!/bin/sh\nexec /snap/bin/firefox "$@"\n')
        plain = self.executable("firefox-bin", "\x7fELF fixture")
        with patch("launch.sys.platform", "linux"):
            moved = launch.profile_for("native", str(wrapper), default)
            self.assertEqual(moved, Path.home() / "snap/firefox/common/chilpa-qbz-profile")
            self.assertFalse(any(part.startswith(".") for part in moved.relative_to(Path.home()).parts))
            self.assertEqual(launch.profile_for("native", str(plain), default), default)
            self.assertEqual(launch.profile_for("flatpak", "", default), default)


class StartupTraceTests(unittest.TestCase):
    def test_stages_and_a_failure_are_written_without_secrets(self):
        with tempfile.TemporaryDirectory() as directory:
            trace = StartupTrace(Path(directory) / "data")
            trace.note("listening", port=4711)
            try:
                raise RuntimeError("fixture failure")
            except RuntimeError as error:
                trace.failure(error)
            log = (Path(directory) / "data/startup.log").read_text()
            mode = os.stat(Path(directory) / "data/startup.log").st_mode & 0o777
        self.assertIn(" started pid=", log)
        self.assertIn(" listening port=4711", log)
        self.assertIn("failed error=RuntimeError: fixture failure at=test_launch.py:", log)
        self.assertEqual(mode, 0o600)

    def test_an_unwritable_folder_never_stops_the_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            blocker = Path(directory) / "file"
            blocker.write_text("")
            trace = StartupTrace(blocker / "data")
            trace.note("listening", port=1)
            trace.failure(RuntimeError("fixture"))


if __name__ == "__main__":
    unittest.main()
