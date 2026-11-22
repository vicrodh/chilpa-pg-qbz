"""Bounded login lifecycle controlled by the native Chilpa settings wizard."""
import asyncio
import json
import os
from pathlib import Path
import sys
from urllib.parse import unquote, urlsplit
from launch import candidates, flatpak_available, normalize


class Setup:
    def __init__(self, companion):
        self.companion = companion
        self.file = companion.directory / "launcher.json"
        self.phase = "detect"
        self.problem = ""
        self.task = None
        self.found = []

    async def detect(self):
        self.found = [{"mode": "native", "path": p} for p in candidates()]
        try:
            if await asyncio.to_thread(flatpak_available):
                self.found.append({"mode": "flatpak", "path": ""})
        except (OSError, TimeoutError):
            pass
        preference = {}
        try:
            preference = json.loads(self.file.read_text())
        except (OSError, ValueError):
            pass
        if self.valid_path(preference.get("ffmpeg", "")):
            self.companion.args.ffmpeg = preference["ffmpeg"]
        options = [preference] + self.found
        for option in options:
            if option.get("mode") == "flatpak" and any(p["mode"] == "flatpak" for p in self.found):
                self.select("flatpak", "")
                break
            if option.get("mode") == "native" and self.valid_path(option.get("path", "")):
                self.select("native", option["path"])
                break
        if not getattr(self.companion.args, "ffmpeg", None):
            self.phase = "detect"
        if preference.get("verified") and self.phase == "login":
            await self.action({"action": "verified"})
        return self.snapshot()

    @staticmethod
    def valid_path(value):
        return bool(value and normalize(value).is_file() and os.access(normalize(value), os.X_OK))

    def select(self, mode, value):
        if value.startswith("file:"):
            url = urlsplit(value)
            if url.scheme != "file" or url.netloc not in ("", "localhost"):
                raise ValueError("Choose a local Firefox installation")
            value = unquote(url.path)
            if sys.platform == "win32" and value.startswith("/"):
                value = value[1:]
        if mode == "native" and not self.valid_path(value):
            raise ValueError("Choose the Firefox executable or application")
        if mode == "flatpak" and not any(p["mode"] == "flatpak" for p in self.found):
            raise ValueError("Install Firefox Flatpak and detect again")
        if mode not in ("native", "flatpak"):
            raise ValueError("Unsupported Firefox installation")
        self.companion.args.mode = mode
        self.companion.args.firefox = str(normalize(value)) if value else ""
        with open(self.file, "w", opener=lambda p, f: os.open(p, f, 0o600)) as output:
            json.dump({"mode": mode, "path": self.companion.args.firefox, "ffmpeg": self.companion.args.ffmpeg}, output)
        self.phase = "login"
        self.problem = ""

    def snapshot(self):
        return {"phase": self.phase, "problem": self.problem, "playback_error": getattr(self.companion, "last_error", ""),
                "playback_error_code": getattr(self.companion, "last_error_code", ""), "found": self.found,
                "mode": self.companion.args.mode, "path": self.companion.args.firefox,
                "directory": sys.platform == "darwin", "missing_decoder": not bool(getattr(self.companion.args, "ffmpeg", None))}

    async def action(self, body):
        action = body.get("action", "status")
        if action == "verified":
            with open(self.file, "w", opener=lambda p, f: os.open(p, f, 0o600)) as output:
                json.dump({"mode": self.companion.args.mode, "path": self.companion.args.firefox, "verified": True, "ffmpeg": self.companion.args.ffmpeg}, output)
            self.phase = "ready"
        elif action == "disconnect":
            await self.cancel()
            self.select(self.companion.args.mode, self.companion.args.firefox)
            self.companion.session = None
        elif action == "cancel":
            await self.cancel()
            self.phase = "login"
        elif action == "detect":
            await self.cancel()
            self.phase = "detect"
            return await self.detect()
        elif action == "select":
            await self.cancel()
            decoder = str(body.get("ffmpeg", ""))
            if decoder:
                parsed = urlsplit(decoder)
                if parsed.scheme == "file" and parsed.netloc in ("", "localhost"):
                    decoder = unquote(parsed.path)
                if not self.valid_path(decoder):
                    raise ValueError("Choose an installed FFmpeg executable")
                self.companion.args.ffmpeg = str(normalize(decoder))
            if not self.companion.args.ffmpeg:
                raise ValueError("Choose an installed FFmpeg executable")
            self.select(body.get("mode", self.companion.args.mode), str(body.get("path") or self.companion.args.firefox))
        elif action == "login" and self.phase not in ("waiting", "checking"):
            if any(not job.finished.is_set() for job in self.companion.jobs.values()):
                raise ValueError("Wait for audio preparation to finish before signing in")
            self.phase, self.problem = "waiting", ""
            self.task = asyncio.create_task(self.login())
        elif action == "change":
            await self.cancel()
            self.phase = "detect"
        return self.snapshot()

    async def login(self):
        companion = self.companion
        try:
            async with asyncio.timeout(600):
                if companion.idle_task:
                    companion.idle_task.cancel()
                async with companion.browser_lock:
                    if companion.browser:
                        await companion.browser.close()
                        companion.browser = None
                companion.session = None
                await companion.start_browser(visible=True)
                while True:
                    if companion.browser.process.returncode is not None:
                        raise RuntimeError("Firefox was closed before signing in")
                    try:
                        await companion.account(refresh=True)
                        break
                    except RuntimeError:
                        await asyncio.sleep(1.5)
                await companion.browser.remember_session()
                companion.last_error = ""
                self.phase = "checking"
                async with companion.browser_lock:
                    await companion.browser.close()
                    companion.browser = None
                # Verify the retained account in the same cold start used for playback.
                companion.session = None
                await companion.start_browser()
                await companion.account(refresh=True)
                self.phase = "verify"
        except asyncio.CancelledError:
            raise
        except (RuntimeError, TimeoutError, OSError):
            self.phase = "login"
            self.problem = "Sign-in did not complete. Try again in Firefox."
        finally:
            async with companion.browser_lock:
                if companion.browser:
                    await companion.browser.close()
                    companion.browser = None

    async def cancel(self):
        if self.task and not self.task.done():
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
        self.task = None
