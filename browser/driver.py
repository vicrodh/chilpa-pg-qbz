"""Own one Firefox process and one serialized BiDi connection."""

import asyncio
import json
import os
import re
from pathlib import Path
import socket
from websockets.asyncio.client import connect
from launch import command as launch_command, external_environment

ROOT = Path(__file__).resolve().parent
STARTUP_SECONDS = 30


class BrowserSessionExpired(RuntimeError):
    """The official web player returned to sign-in after a browser restart."""


class BrowserPageOperationError(RuntimeError):
    """Classify a page error without retaining its text, URLs or account data."""
    def __init__(self, expression, details):
        super().__init__("Firefox page operation failed")
        self.operation = next((name for name in ("drain", "prepare", "metadata", "stop", "local", "seek", "ready")
                               if name in expression), "other")
        text = str(details.get("text", ""))
        self.terms = [name for name in ("TypeError", "InvalidStateError", "NotSupportedError", "QuotaExceededError",
                                       "ReferenceError", "currentTime", "buffered", "getState", "null", "undefined", "finite", "SourceBuffer")
                      if name in text]


class Browser:
    def __init__(self, profile, firefox, headless=False, mode="native"):
        self.profile = profile
        self.firefox = firefox
        self.headless = headless
        self.mode = mode
        self.lock = asyncio.Lock()
        self.sequence = 0
        self.process = None
        self.instance = None
        self.ws = None

    async def command(self, method, params):
        async with self.lock:
            self.sequence += 1
            ident = self.sequence
            await self.ws.send(
                json.dumps({"id": ident, "method": method, "params": params})
            )
            while True:
                result = json.loads(await asyncio.wait_for(self.ws.recv(), 25))
                if result.get("id") != ident:
                    continue
                if result.get("type") == "error":
                    raise RuntimeError("Firefox control command failed")
                return result.get("result", {})

    async def evaluate(self, expression, *, user_activation=False):
        result = await self.command(
            "script.evaluate",
            {
                "expression": expression,
                "target": {"context": self.context},
                "awaitPromise": True,
                "resultOwnership": "none",
                "userActivation": user_activation,
            },
        )
        if result.get("type") == "exception":
            raise BrowserPageOperationError(expression, result.get("exceptionDetails", {}))
        return result.get("result", {}).get("value")

    async def start(self, landing):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        self.profile.mkdir(parents=True, exist_ok=True)
        self.profile.chmod(0o700)
        args = launch_command(
            self.mode, self.firefox, self.profile, port, self.headless
        )
        descriptor = None
        reader = None
        options = {}
        if self.mode == "flatpak":
            reader, descriptor = os.pipe()
            os.set_blocking(reader, False)
            args.insert(2, "--instance-id-fd=" + str(descriptor))
            options["pass_fds"] = (descriptor,)
        try:
            self.process = await asyncio.create_subprocess_exec(
                *args, env=external_environment(), stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL, **options,
            )
            if descriptor is not None:
                os.close(descriptor)
                descriptor = None
                value = b""
                for _ in range(100):
                    try:
                        chunk = os.read(reader, 128)
                        if not chunk:
                            break
                        value += chunk
                        if b"\n" in value:
                            break
                    except BlockingIOError:
                        pass
                    await asyncio.sleep(0.02)
                identity = value.decode("ascii").strip()
                if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", identity):
                    raise RuntimeError("Firefox Flatpak did not report its instance")
                self.instance = identity
        finally:
            if descriptor is not None:
                os.close(descriptor)
            if reader is not None:
                os.close(reader)
        # A first Snap or Flatpak start on a cold disk takes well over ten seconds.
        deadline = asyncio.get_running_loop().time() + STARTUP_SECONDS
        while True:
            if self.process.returncode is not None:
                raise RuntimeError(
                    "Firefox could not open the Qobuz profile; close any other window using it"
                )
            try:
                self.ws = await connect(
                    f"ws://127.0.0.1:{port}/session",
                    open_timeout=1,
                    max_size=2 * 1024 * 1024,
                    close_timeout=1,
                )
                break
            except OSError:
                if asyncio.get_running_loop().time() >= deadline:
                    raise RuntimeError("Firefox startup timed out")
                await asyncio.sleep(0.1)
        await self.command("session.new", {"capabilities": {}})
        tree = await self.command("browsingContext.getTree", {})
        self.context = tree["contexts"][0]["context"]
        early = (ROOT / "vendor/early.js").read_text()
        bridge = (
            (ROOT / "vendor/bridge.js")
            .read_text()
            .replace("s.outputs || {}", "s.audioOutputs || s.outputs || {}")
        )
        capture = (ROOT / "capture.js").read_text()
        code = (
            '()=>{if(location.origin!=="https://play.qobuz.com")return;'
            + early
            + "\n"
            + capture
            + '\ndocument.addEventListener("DOMContentLoaded",()=>{'
            + bridge
            + "\n},{once:true});}"
        )
        await self.command("script.addPreloadScript", {"functionDeclaration": code})
        await self.command(
            "browsingContext.navigate",
            {
                "context": self.context,
                "url": "https://play.qobuz.com/",
                "wait": "complete",
            },
        )
        if not self.headless and landing:
            tab = await self.command("browsingContext.create", {"type": "tab"})
            self.ui = tab["context"]
            await self.command(
                "browsingContext.navigate",
                {"context": self.ui, "url": landing, "wait": "complete"},
            )

    async def reset_player(self):
        # A stopped MSE stream may retain only media fragments, without its init
        # segment. Each preparation starts with a fresh page and capture hooks.
        await self.command(
            "browsingContext.navigate",
            {
                "context": self.context,
                "url": "https://play.qobuz.com/",
                "wait": "complete",
            },
        )
        await self.wait_ready()

    async def wait_ready(self):
        for _ in range(60):
            if await self.evaluate("!!window.__relay?.ready()"):
                return
            await asyncio.sleep(0.1)
        if await self.evaluate("!!window.__zbq?.ready() && !window.__relay?.ready()"):
            raise BrowserSessionExpired("Sign in again with Firefox in the Qobuz settings")
        raise RuntimeError("Firefox player did not become ready")

    async def account(self):
        for _ in range(60):
            if await self.evaluate("!!window.__relay?.ready()"):
                break
            await asyncio.sleep(0.1)
        else:
            raise BrowserSessionExpired("Sign in again with Firefox in the Qobuz settings")
        return json.loads(
            await self.evaluate(
                "(async()=>JSON.stringify(await window.__relay.account()))()"
            )
        )

    async def remember_session(self):
        if not await self.evaluate("window.__relay.rememberSession()"):
            raise RuntimeError("The browser could not retain this Qobuz session")

    async def local(self):
        await self.wait_ready()
        for _ in range(40):
            if await self.evaluate("window.__relay.local()", user_activation=True):
                return
            await asyncio.sleep(0.1)
        raise RuntimeError("The official player did not select its local output")

    async def prepare_capture(self, track, quality, *, metadata=None):
        try:
            already_started = metadata is None
            if metadata is None:
                metadata = json.loads(await self.evaluate(
                    "(async()=>JSON.stringify(await window.__relay.prepare("
                    + json.dumps(track) + "," + str(quality) + ")))()",
                    user_activation=True,
                ))
            if not metadata.get("available"):
                raise RuntimeError("This track is not available for full streaming")
            duration = metadata.get("duration_ms")
            if type(duration) not in (int, float) or not 0 < duration < float("inf"):
                raise RuntimeError("Track duration is unavailable")
            media = await self.start_capture(track, quality, duration_ms=duration, already_started=already_started)
            return metadata, media
        except BaseException:
            try:
                await self.evaluate("window.__relay.stop()")
            except Exception:
                pass
            raise

    async def start_capture(self, track, quality, *, duration_ms=None, already_started=False):
        # Retry only before any media bytes were accepted, never during playback.
        mismatched_stream = False
        for attempt in range(2):
            if attempt:
                await self.reset_player()
                await self.local()
            if attempt or not already_started:
                await self.evaluate("window.__relay.begin(" + json.dumps(track) + "," + str(quality) + ")", user_activation=True)
                # The official local player can restore the previous queue position.
                # Capture a new track from its beginning, including repeated plays.
                await self.evaluate("window.__zbq.seek(0)", user_activation=True)
            corrections = 0
            for _ in range(60):
                if not await self.evaluate("window.__relay.localSelected()"):
                    # QConnect's asynchronous favorite-output restoration can
                    # override the selection made before Play. Reloading here
                    # would restore that favorite again, so finish selecting the
                    # local player on this page before accepting media.
                    if corrections >= 2:
                        break
                    corrections += 1
                    await self.local()
                    await self.evaluate("window.__zbq.seek(0)", user_activation=True)
                    await self.evaluate("window.__zbq.resume()", user_activation=True)
                value = json.loads(await self.evaluate("JSON.stringify(window.__relay.drain())"))
                if value.get("format") or value.get("error") or value.get("data"):
                    if duration_ms is not None and value.get("format"):
                        original = value["format"]
                        rate = original.get("sample_rate", 0)
                        if not rate or abs(original.get("frames", 0) / rate - duration_ms / 1000) > 2:
                            # A restored web-player queue can still deliver the
                            # previous track while the new request is applied.
                            # Discard it before exposing any bytes to Chilpa.
                            mismatched_stream = True
                            break
                    return value
                await asyncio.sleep(0.1)
        if mismatched_stream:
            raise RuntimeError("The browser returned an incomplete preview")
        raise RuntimeError("The player delivered no initial media after one retry")

    async def close(self):
        requested_close = self.ws is not None
        if self.ws:
            try:
                await asyncio.wait_for(self.command("browser.close", {}), 2)
            except Exception:
                pass
            await self.ws.close()
            self.ws = None
        # BiDi acknowledges shutdown before Firefox has flushed its profile.
        # Killing the process here loses a newly remembered login on disk.
        if requested_close and self.process and self.process.returncode is None:
            try:
                await asyncio.wait_for(self.process.wait(), 8)
            except asyncio.TimeoutError:
                pass
        if self.instance:
            process = await asyncio.create_subprocess_exec(
                "flatpak", "kill", self.instance, env=external_environment(),
                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL,
            )
            try:
                await asyncio.wait_for(process.wait(), 3)
            except asyncio.TimeoutError:
                process.kill()
                await process.wait()
            self.instance = None
        if self.process and self.process.returncode is None:
            try:
                self.process.terminate()
            except ProcessLookupError:
                pass
            try:
                await asyncio.wait_for(self.process.wait(), 3)
            except asyncio.TimeoutError:
                self.process.kill()
                await self.process.wait()
