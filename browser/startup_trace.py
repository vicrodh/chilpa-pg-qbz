"""Startup trace kept beside the helper's data: the host discards its stderr,
so a report needs this file to say where a start stopped. It never holds the
connection key, account data or page content."""

import os
from pathlib import Path
import platform
import sys
import time
import traceback

NAME = "startup.log"


def process_age():
    """Seconds since the process began, interpreter and imports included."""
    try:
        with open("/proc/self/stat") as stat:
            fields = stat.read().rsplit(")", 1)[1].split()
        started = int(fields[19]) / os.sysconf("SC_CLK_TCK")
        with open("/proc/uptime") as uptime:
            return float(uptime.read().split()[0]) - started
    except (OSError, ValueError, IndexError):
        return None


class StartupTrace:
    def __init__(self, directory):
        self.path = Path(directory) / NAME
        self.started = time.monotonic()
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with open(self.path, "w", opener=lambda p, f: os.open(p, f, 0o600)) as output:
                output.write(time.strftime("%Y-%m-%d %H:%M:%S %z") + "\n")
        except OSError:
            self.path = None
        age = process_age()
        self.note("started", pid=os.getpid(), python=platform.python_version(),
                  frozen=bool(getattr(sys, "frozen", False)), system=platform.platform(),
                  age="unknown" if age is None else f"{age:.3f}s")

    def note(self, stage, **details):
        if self.path is None:
            return
        line = f"{time.monotonic() - self.started:8.3f}s {stage}"
        for key, value in details.items():
            line += f" {key}={value}"
        try:
            with open(self.path, "a") as output:
                output.write(line + "\n")
        except OSError:
            pass

    def failure(self, error):
        frames = traceback.extract_tb(error.__traceback__)[-4:]
        where = " < ".join(f"{Path(frame.filename).name}:{frame.lineno}" for frame in reversed(frames))
        self.note("failed", error=f"{type(error).__name__}: {error}", at=where or "unknown")
