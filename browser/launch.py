"""Firefox installations are resolved by the external companion, never by the plugin."""

from pathlib import Path
import os
import shutil
import subprocess
import sys


def normalize(path):
    path = Path(path).expanduser()
    # A portal-selected executable must run beside its real installation libraries.
    if hasattr(os, "getxattr"):
        try:
            original = os.fsdecode(os.getxattr(path, "user.document-portal.host-path"))
            if Path(original).is_absolute():
                path = Path(original)
        except OSError:
            pass
    return path / "Contents/MacOS/firefox" if path.suffix.lower() == ".app" else path


def candidates():
    paths = []
    found = shutil.which("firefox")
    if found:
        paths.append(Path(found))
    if sys.platform == "darwin":
        paths += [
            Path("/Applications/Firefox.app/Contents/MacOS/firefox"),
            Path.home() / "Applications/Firefox.app/Contents/MacOS/firefox",
        ]
    elif sys.platform == "win32":
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]:
            if os.environ.get(variable):
                paths.append(Path(os.environ[variable]) / "Mozilla Firefox/firefox.exe")
    else:
        paths += [
            Path("/opt/firefox/firefox"),
            Path("/usr/bin/firefox"),
            Path("/usr/lib/firefox/firefox"),
        ]
    return list(
        dict.fromkeys(str(p) for p in paths if p.is_file() and os.access(p, os.X_OK))
    )


def flatpak_available():
    command = shutil.which("flatpak")
    return bool(
        command
        and subprocess.run(
            [command, "info", "org.mozilla.firefox"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
        ).returncode
        == 0
    )


def snap_firefox(executable):
    """Whether this Firefox runs confined by Snap: the snap itself, its
    /snap/bin launcher, or a distribution wrapper script that execs it."""
    if sys.platform != "linux" or not executable:
        return False
    try:
        resolved = Path(executable).resolve()
    except (OSError, RuntimeError):
        return False
    if resolved.parts[:2] == ("/", "snap") or resolved == Path("/usr/bin/snap"):
        return True
    try:
        with open(resolved, "rb") as script:
            head = script.read(4096)
    except OSError:
        return False
    return head.startswith(b"#!") and b"/snap/bin/firefox" in head


def snap_profile():
    """Snap confinement hides dot folders in the home directory, where the
    profile normally lives; the snap's own common folder stays readable."""
    return Path.home() / "snap/firefox/common/chilpa-qbz-profile"


def profile_for(mode, executable, profile):
    if mode == "native" and snap_firefox(executable):
        return snap_profile()
    return profile


def command(mode, executable, profile, port, headless):
    profile = Path(profile).resolve()
    if mode == "flatpak":
        if sys.platform != "linux" or not shutil.which("flatpak"):
            raise RuntimeError("Firefox Flatpak is not available on this system")
        result = [
            "flatpak",
            "run",
            "--die-with-parent",
            "--filesystem=" + str(profile),
            "org.mozilla.firefox",
        ]
    else:
        selected = normalize(executable)
        if not selected.is_file():
            raise RuntimeError("Choose an installed Firefox executable")
        result = [str(selected)]
    result += [
        "--no-remote",
        "--profile",
        str(profile),
        "--remote-debugging-port",
        str(port),
    ]
    if headless:
        result.append("--headless")
    return result + ["about:blank"]


def storage_paths():
    if sys.platform == "darwin":
        return (
            Path.home() / "Library/Application Support/QBZ Browser",
            Path.home() / "Library/Caches/QBZ Browser",
        )
    if sys.platform == "win32":
        base = (
            Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData/Local")))
            / "QBZ Browser"
        )
        return base / "Data", base / "Cache"
    return (
        Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share")))
        / "qbz-browser",
        Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache")))
        / "qbz-browser",
    )


def external_environment():
    """Do not pass the frozen application's library search path to Firefox/FFmpeg."""
    environment = os.environ.copy()
    if getattr(sys, "frozen", False):
        original = environment.pop("LD_LIBRARY_PATH_ORIG", None)
        if original is None:
            environment.pop("LD_LIBRARY_PATH", None)
        else:
            environment["LD_LIBRARY_PATH"] = original
        for name in ("QT_PLUGIN_PATH", "QML2_IMPORT_PATH"):
            environment.pop(name, None)
    return environment
