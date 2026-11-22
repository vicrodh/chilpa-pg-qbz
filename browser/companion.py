"""Qobuz browser: explicit Firefox owner and bounded lossless PCM audio relay."""

import argparse
import asyncio
import base64
from dataclasses import dataclass, field
import hashlib
import hmac
import html
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import sys
import time
from aiohttp import web
from driver import Browser, BrowserSessionExpired, BrowserPageOperationError
from pcm import header as wave_header
from launch import external_environment

MAX_TRACK = 480 * 1024 * 1024
MAX_CACHE = 2 * 1024 * 1024 * 1024
MAX_PCM = 1024 * 1024 * 1024
MAX_JOBS = 2


@dataclass
class Job:
    track: str
    quality: int
    ticket: str
    path: Path
    created: float = field(default_factory=time.monotonic)
    first: asyncio.Event = field(default_factory=asyncio.Event)
    finished: asyncio.Event = field(default_factory=asyncio.Event)
    format: dict | None = None
    metadata: dict | None = None
    error: str = ""
    error_code: str = ""
    task: asyncio.Task | None = None
    size: int = 0
    readers: int = 0
    available: int = 0
    changed: asyncio.Condition = field(default_factory=asyncio.Condition)
    checksum: object = field(default_factory=hashlib.md5)


class Companion:
    def __init__(self, args):
        self.args = args
        self.directory = args.data.resolve()
        self.directory.mkdir(parents=True, exist_ok=True)
        self.directory.chmod(0o700)
        self.cache = args.cache.resolve()
        self.cache.mkdir(parents=True, exist_ok=True)
        self.cache.chmod(0o700)
        keyfile = self.directory / "key"
        if keyfile.exists():
            self.key = keyfile.read_text().strip()
        else:
            self.key = secrets.token_urlsafe(32)
            with open(keyfile, "x", opener=lambda p, f: os.open(p, f, 0o600)) as f:
                f.write(self.key)
        if not re.fullmatch(r"[A-Za-z0-9_-]{32,128}", self.key):
            raise RuntimeError("Invalid Qobuz browser key file")
        self.browser = None
        self.player_dirty = False
        self.last_track = None
        self.session = None
        self.session_at = 0
        self.browser_lock = asyncio.Lock()
        self.jobs = {}
        self.tickets = {}
        self.preparing = asyncio.Semaphore(1)
        self.ui_key = secrets.token_urlsafe(24)
        self.last_error = ""
        self.last_error_code = ""
        self.stopping = asyncio.Event()
        self.idle_task = None
        self.warm_task = None
        self.port = args.port
        self.setup = None
        self.metrics = {
            "prepared": 0,
            "bytes": 0,
            "range_requests": 0,
            "browser_starts": 0,
            "warm_requests": 0,
            "warm_ready": 0,
            "catalog_metadata": 0,
            "browser_metadata": 0,
            "startup": [],
            "page_errors": [],
        }

    @property
    def origin(self):
        return f"http://127.0.0.1:{self.port}"

    @property
    def landing(self):
        return self.origin + "/ui/" + self.ui_key

    async def start_browser(self, visible=False):
        async with self.browser_lock:
            if self.browser and self.browser.process.returncode is None:
                return
            if self.browser:
                await self.browser.close()
            self.browser = Browser(
                self.args.profile,
                self.args.firefox,
                headless=not visible,
                mode=self.args.mode,
            )
            try:
                await self.browser.start(None if getattr(self.args, "managed", False) else self.landing)
                self.player_dirty = False
                self.last_track = None
                self.metrics["browser_starts"] += 1
            except BaseException:
                await self.browser.close()
                self.browser = None
                raise

    async def account(self, refresh=False):
        if self.session and not refresh and time.monotonic() - self.session_at < 300:
            return self.session
        await self.start_browser()
        self.session = await self.browser.account()
        self.session_at = time.monotonic()
        return self.session

    def authorized(self, request):
        return hmac.compare_digest(
            request.headers.get("Authorization", ""), "Bearer " + self.key
        )

    async def rpc(self, request):
        if not self.authorized(request):
            return web.json_response({"error": "Connection code required"}, status=403)
        try:
            body = await request.json()
        except (ValueError, TypeError):
            return web.json_response({"error": "Invalid JSON"}, status=400)
        if not isinstance(body, dict):
            return web.json_response({"error": "Expected object"}, status=400)
        try:
            if request.path == "/setup" and self.setup:
                try:
                    if body.get("action") in ("login", "disconnect", "cancel", "detect", "select", "change"):
                        await self.cancel_warm()
                    return web.json_response(await self.setup.action(body))
                except ValueError as error:
                    return web.json_response({"error": str(error)}, status=400)
            if request.path == "/warm":
                if not self.setup or self.setup.phase != "ready" or self.stopping.is_set():
                    return web.json_response({"accepted": False})
                self.metrics["warm_requests"] += 1
                if self.idle_task:
                    self.idle_task.cancel()
                if not self.warm_task or self.warm_task.done():
                    self.warm_task = asyncio.create_task(self.warm_browser())
                return web.json_response({"accepted": True})
            if request.path == "/stop":
                job = self.tickets.get(str(body.get("ticket", "")))
                if job and not job.finished.is_set() and job.task:
                    await asyncio.sleep(0.2)
                    if not job.readers:
                        job.task.cancel()
                return web.json_response({"accepted": True})
            if request.path == "/open":
                if any(not job.finished.is_set() for job in self.jobs.values()):
                    return web.json_response(
                        {"error": "Wait for audio preparation to finish"}, status=409
                    )
                if self.idle_task:
                    self.idle_task.cancel()
                async with self.browser_lock:
                    if self.browser:
                        await self.browser.close()
                        self.browser = None
                self.session = None
                await self.start_browser(visible=True)
                return web.json_response({"opened": True})
            if request.path == "/session":
                account = await self.account()
                if self.idle_task:
                    self.idle_task.cancel()
                self.idle_task = asyncio.create_task(self.close_idle_browser())
                return web.json_response(account)
            if request.path == "/resolve":
                return await self.resolve(body)
            return web.json_response({"error": "Unknown operation"}, status=404)
        except BrowserSessionExpired:
            self.session = None
            if self.setup:
                self.setup.phase = "login"
            self.last_error = "The Firefox session expired. Sign in again in the Qobuz settings."
            return web.json_response({"code": "session_expired", "error": self.last_error}, status=401)
        except (RuntimeError, asyncio.TimeoutError):
            self.last_error = "Firefox is not ready. Open the QBZ settings in Chilpa and check sign-in."
            return web.json_response({"error": self.last_error}, status=409)

    async def warm_browser(self):
        # Browsing can prepare the page, but must never select an output or play.
        try:
            async with asyncio.timeout(20):
                if any(not job.finished.is_set() for job in self.jobs.values()):
                    return
                await self.start_browser()
                await self.browser.wait_ready()
                await self.account()
                self.metrics["warm_ready"] += 1
        except Exception:
            pass  # Play reports any actual failure; warming is only an optimization.
        finally:
            if not self.stopping.is_set():
                if self.idle_task:
                    self.idle_task.cancel()
                self.idle_task = asyncio.create_task(self.close_idle_browser())

    async def cancel_warm(self):
        if self.warm_task and not self.warm_task.done():
            self.warm_task.cancel()
            await asyncio.gather(self.warm_task, return_exceptions=True)
        self.warm_task = None

    async def resolve(self, body):
        if self.setup and self.setup.phase in ("waiting", "checking"):
            return web.json_response({"error": "Finish signing in first"}, status=409)
        track = str(body.get("track", ""))
        quality = body.get("quality", 27)
        if not track.isdigit() or len(track) > 20 or quality not in (6, 7, 27):
            return web.json_response({"error": "Invalid track or quality"}, status=400)
        identity = await self.account()
        entitlement = identity["session"].get("entitlements", {})
        if not (
            entitlement.get("lossless_streaming") or entitlement.get("hires_streaming")
        ):
            return web.json_response(
                {"error": "A lossless streaming subscription is required"}, status=403
            )
        key = (track, quality)
        job = self.jobs.get(key)
        if job and job.error:
            self.jobs.pop(key)
            self.tickets.pop(job.ticket, None)
            job.path.unlink(missing_ok=True)
            job = None
        if job is None:
            completed = sorted(
                (
                    j
                    for j in self.jobs.values()
                    if j.finished.is_set() and not j.readers
                ),
                key=lambda j: j.created,
            )
            while completed and (
                len(self.jobs) >= MAX_JOBS
                or sum(j.size for j in self.jobs.values()) > MAX_CACHE - MAX_PCM
            ):
                old = completed.pop(0)
                self.jobs.pop((old.track, old.quality), None)
                self.tickets.pop(old.ticket, None)
                old.path.unlink(missing_ok=True)
            if len(self.jobs) >= MAX_JOBS:
                return web.json_response(
                    {"error": "Preparation queue full"}, status=429
                )
            ticket = secrets.token_urlsafe(24)
            job = Job(track, quality, ticket, self.cache / (ticket + ".wav"))
            metadata = body.get("metadata")
            if (isinstance(metadata, dict) and metadata.get("track") == track
                    and type(metadata.get("duration_ms")) is int and 0 < metadata["duration_ms"] < 2**53
                    and type(metadata.get("available")) is bool):
                job.metadata = {key: metadata[key] for key in ("duration_ms", "available")}
            self.jobs[key] = job
            self.tickets[ticket] = job
            job.task = asyncio.create_task(self.prepare(job))
        job.created = time.monotonic()
        try:
            await asyncio.wait_for(job.first.wait(), 17)
        except asyncio.TimeoutError:
            pass
        if job.error:
            return web.json_response(
                {"code": job.error_code, "error": job.error}, status=502
            )
        return web.json_response(
            {
                "url": self.origin + "/audio/" + job.ticket,
                "format": job.format or {"codec": "wav"},
                "delivery": "direct",
                "preview": False,
            }
        )

    async def collect_pcm(self, job, process, started, timings):
        first_chunk = True
        with job.path.open("ab", buffering=0) as output:
            while True:
                chunk = await process.stdout.read(65536)
                if not chunk:
                    break
                if job.available + len(chunk) > job.size:
                    raise RuntimeError(
                        "Decoded audio exceeded the declared frame count"
                    )
                output.write(chunk)
                job.checksum.update(chunk)
                async with job.changed:
                    job.available += len(chunk)
                    job.changed.notify_all()
                if first_chunk:
                    first_chunk = False
                    timings["first_pcm"] = round((time.monotonic() - started) * 1000)
                    if len(self.metrics["startup"]) < 16:
                        self.metrics["startup"].append(dict(timings))
                    print(json.dumps({"event": "playback_start", "elapsed_ms": timings}),
                          file=sys.stderr, flush=True)

    async def prepare(self, job):
        started = time.monotonic()
        process = None
        collector = None
        timings = {}
        def mark(stage):
            timings[stage] = round((time.monotonic() - started) * 1000)
        try:
            async with self.preparing:
                mark("preparation_slot")
                await self.start_browser()
                mark("browser")
                if self.player_dirty or self.last_track == job.track:
                    await self.browser.reset_player()
                self.player_dirty = True
                await self.browser.local()
                mark("local_player")
                self.metrics["catalog_metadata" if job.metadata is not None else "browser_metadata"] += 1
                metadata, first_media = await self.browser.prepare_capture(job.track, job.quality, metadata=job.metadata)
                mark("metadata_and_media")
                compressed = 0
                try:
                    while True:
                        value = first_media
                        first_media = None
                        if value is None:
                            value = json.loads(await self.browser.evaluate(
                                "JSON.stringify(window.__relay.drain())"))
                        if value["error"]:
                            raise RuntimeError(
                                "Firefox capture exceeded its transfer budget"
                            )
                        if value.get("format") and process is None:
                            original = value["format"]
                            prefix, length = wave_header(original)
                            if len(prefix) + length > MAX_PCM:
                                raise RuntimeError(
                                    "Track exceeds the 1 GiB PCM cache limit"
                                )
                            if (
                                abs(
                                    original["frames"] / original["sample_rate"]
                                    - metadata["duration_ms"] / 1000
                                )
                                > 2
                            ):
                                raise RuntimeError(
                                    "The browser returned an incomplete preview"
                                )
                            job.format = {**original, "codec": "wav"}
                            job.size = len(prefix) + length
                            job.path.write_bytes(prefix)
                            job.available = len(prefix)
                            kind = "s16le" if original["bits"] == 16 else "s24le"
                            process = await asyncio.create_subprocess_exec(
                                self.args.ffmpeg,
                                "-v",
                                "error",
                                "-xerror",
                                "-i",
                                "pipe:0",
                                "-map",
                                "0:a:0",
                                "-c:a",
                                "pcm_" + kind,
                                "-f",
                                kind,
                                "pipe:1",
                                env=external_environment(),
                                stdin=asyncio.subprocess.PIPE,
                                stdout=asyncio.subprocess.PIPE,
                                stderr=asyncio.subprocess.DEVNULL,
                            )
                            collector = asyncio.create_task(
                                self.collect_pcm(job, process, started, timings)
                            )
                            job.first.set()
                        if value["data"]:
                            if process is None:
                                raise RuntimeError(
                                    "Media arrived without its FLAC stream information"
                                )
                            chunk = base64.b64decode(value["data"], validate=True)
                            compressed += len(chunk)
                            if compressed > MAX_TRACK:
                                raise RuntimeError(
                                    "Track exceeds the compressed capture budget"
                                )
                            process.stdin.write(chunk)
                            await process.stdin.drain()
                        if value["ended"]:
                            break
                        if time.monotonic() - started > 180:
                            raise RuntimeError(
                                "Track preparation exceeded three minutes"
                            )
                        await asyncio.sleep(0.002 if value["data"] else 0.04)
                finally:
                    await self.browser.evaluate("window.__relay.stop()")
                if process is None:
                    raise RuntimeError("The player delivered no lossless audio")
                process.stdin.close()
                await asyncio.wait_for(collector, 15)
                if (
                    await asyncio.wait_for(process.wait(), 5)
                    or job.available != job.size
                ):
                    raise RuntimeError(
                        "Decoded samples did not match the complete track length"
                    )
                expected = job.format.get("md5", "")
                if (
                    expected
                    and expected != "0" * 32
                    and job.checksum.hexdigest() != expected
                ):
                    raise RuntimeError(
                        "Decoded PCM did not match the FLAC source checksum"
                    )
                self.player_dirty = False
                self.last_track = job.track
                self.metrics["prepared"] += 1
                self.metrics["bytes"] += job.size
                self.last_error = ""
                self.last_error_code = ""
                print(
                    json.dumps(
                        {
                            "event": "track_ready",
                            "seconds": round(time.monotonic() - started, 2),
                            "bytes": job.size,
                            "format": job.format or {"codec": "wav"},
                        }
                    ),
                    file=sys.stderr, flush=True,
                )
        except asyncio.CancelledError:
            job.error = "Preparation cancelled"
            raise
        except Exception as error:
            if isinstance(error, BrowserPageOperationError) and len(self.metrics["page_errors"]) < 8:
                self.metrics["page_errors"].append({"operation": error.operation, "terms": error.terms})
            print(
                json.dumps(
                    {"event": "preparation_failed", "type": type(error).__name__}
                ),
                file=sys.stderr, flush=True,
            )
            if isinstance(error, BrowserSessionExpired):
                self.session = None
                if self.setup:
                    self.setup.phase = "login"
                job.error_code = "session_expired"
                job.error = "The Firefox session expired. Sign in again in the Qobuz settings."
            else:
                job.error_code = {
                    "The player delivered no initial media after one retry": "capture_start_timeout",
                    "Firefox capture exceeded its transfer budget": "capture_overflow",
                    "Decoded samples did not match the complete track length": "incomplete_pcm",
                    "Decoded PCM did not match the FLAC source checksum": "pcm_checksum",
                    "The official player did not select its local output": "local_output_unavailable",
                    "Firefox page operation failed": "browser_page_operation",
                    "Media arrived without its FLAC stream information": "missing_stream_info",
                    "The browser returned an incomplete preview": "track_stream_mismatch",
                }.get(str(error), "preparation_failed")
                job.error = "Firefox could not prepare this track. Retry playback or reconnect in Qobuz settings."
            self.last_error = job.error
            self.last_error_code = job.error_code
        finally:
            if process and process.returncode is None:
                process.kill()
                await process.wait()
            if collector and not collector.done():
                collector.cancel()
                await asyncio.gather(collector, return_exceptions=True)
            job.first.set()
            job.finished.set()
            async with job.changed:
                job.changed.notify_all()
            if self.idle_task:
                self.idle_task.cancel()
            self.idle_task = asyncio.create_task(self.close_idle_browser())

    async def close_idle_browser(self):
        await asyncio.sleep(45)
        if any(not j.finished.is_set() for j in self.jobs.values()):
            return
        async with self.browser_lock:
            if self.browser:
                await self.browser.close()
                self.browser = None

    async def media(self, request):
        job = self.tickets.get(request.match_info["ticket"])
        if not job or time.monotonic() - job.created > 3600:
            return web.Response(status=404)
        job.readers += 1
        try:
            try:
                await asyncio.wait_for(job.first.wait(), 60)
            except asyncio.TimeoutError:
                return web.Response(status=503)
            if job.error:
                return web.Response(status=502)
            size = job.size
            start = 0
            end = size - 1
            value = request.headers.get("Range")
            if value:
                match = re.fullmatch(r"bytes=(\d+)-(\d*)", value)
                if not match:
                    return web.Response(
                        status=416, headers={"Content-Range": f"bytes */{size}"}
                    )
                start = int(match[1])
                end = min(size - 1, int(match[2])) if match[2] else size - 1
                if start >= size or end < start:
                    return web.Response(
                        status=416, headers={"Content-Range": f"bytes */{size}"}
                    )
                self.metrics["range_requests"] += 1

            async def available(position):
                async with job.changed:
                    await asyncio.wait_for(
                        job.changed.wait_for(
                            lambda: job.available > position or job.finished.is_set()
                        ),
                        40,
                    )
                if job.error or job.available <= position:
                    raise RuntimeError("Audio preparation ended before this range")

            try:
                await available(start)
            except (RuntimeError, asyncio.TimeoutError):
                return web.Response(status=503)
            headers = {
                "Content-Type": "audio/wav",
                "Content-Length": str(end - start + 1),
                "Accept-Ranges": "bytes",
                "Cache-Control": "no-store",
            }
            if value:
                headers["Content-Range"] = f"bytes {start}-{end}/{size}"
            response = web.StreamResponse(status=206 if value else 200, headers=headers)
            await response.prepare(request)
            if request.method != "HEAD":
                with job.path.open("rb") as source:
                    source.seek(start)
                    remaining = end - start + 1
                    while remaining:
                        position = source.tell()
                        await available(position)
                        chunk = source.read(
                            min(65536, remaining, job.available - position)
                        )
                        if not chunk:
                            raise RuntimeError("Prepared audio range was unavailable")
                        await response.write(chunk)
                        remaining -= len(chunk)
            await response.write_eof()
            return response
        except (RuntimeError, asyncio.TimeoutError):
            # Cancelled preparation must terminate an incomplete response.
            # Sending a successful short body would invent a complete stream.
            if request.transport:
                request.transport.close()
            raise asyncio.CancelledError()
        except ConnectionResetError:
            raise asyncio.CancelledError()
        except asyncio.CancelledError:
            raise
        finally:
            job.readers -= 1

    async def ui(self, request):
        if not hmac.compare_digest(request.match_info["key"], self.ui_key):
            return web.Response(status=404)
        if request.method == "POST":
            if request.headers.get("Origin") not in (None, self.origin):
                return web.Response(status=403)
            try:
                await self.account(refresh=True)
                self.last_error = ""
            except Exception:
                self.last_error = (
                    "Inicia sesión en la pestaña de Qobuz y vuelve a pulsar Continuar."
                )
        signed = self.session is not None
        status = "Firefox conectado" if signed else "Inicia sesión en Qobuz"
        code = f"qbz1:{self.port}:{self.key}" if signed else ""
        body = f"""<!doctype html><html lang="es"><meta charset="utf-8"><title>Qobuz browser</title>
+<style>body{{font:17px system-ui;background:#101419;color:#e8edf2;max-width:640px;margin:8vh auto;padding:28px}}h1{{font-size:42px}}input{{width:100%;box-sizing:border-box;padding:12px;background:#1b242d;color:inherit;border:1px solid #637683;border-radius:6px}}button{{padding:12px 28px;margin:20px 0;background:#95d7a5;border:0;border-radius:6px;font:inherit}}small{{color:#a7b5c1}}</style>
+<h1>QBZ</h1><h2>{status}</h2><p>Usa la pestaña de Qobuz para iniciar sesión. Si ya lo hiciste, pulsa Continuar.</p>
+<form method="post"><button>Continuar</button></form><p>{html.escape(self.last_error)}</p>
+<p>Copia este código en los ajustes del plugin QBZ:</p><input aria-label="Código de conexión" readonly value="{html.escape(code)}">
+<p><small>Firefox prepara el audio; Chilpa lo reproduce. Este perfil pertenece a QBZ. El navegador se cierra tras 45 segundos sin preparar pistas y se abre sin ventana cuando vuelve a hacer falta.</small></p></html>""".replace(
            "\n+", "\n"
        )
        return web.Response(
            text=body,
            content_type="text/html",
            headers={
                "Cache-Control": "no-store",
                "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
            },
        )

    async def close(self):
        self.stopping.set()
        await self.cancel_warm()
        if self.setup:
            await self.setup.cancel()
        if self.idle_task:
            self.idle_task.cancel()
        tasks = [j.task for j in self.jobs.values() if j.task and not j.task.done()]
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        if self.idle_task:
            self.idle_task.cancel()
        if self.browser:
            await self.browser.close()
        for job in self.jobs.values():
            job.path.unlink(missing_ok=True)
        (self.directory / "metrics.json").write_text(
            json.dumps(self.metrics, indent=2) + "\n"
        )
        (self.directory / "connection.json").unlink(missing_ok=True)


async def run(args):
    companion = Companion(args)
    app = web.Application(client_max_size=65536)
    app.router.add_post("/session", companion.rpc)
    app.router.add_post("/setup", companion.rpc)
    app.router.add_post("/open", companion.rpc)
    app.router.add_post("/stop", companion.rpc)
    app.router.add_post("/resolve", companion.rpc)
    app.router.add_post("/warm", companion.rpc)
    app.router.add_get("/audio/{ticket}", companion.media)
    app.router.add_get("/ui/{key}", companion.ui)
    app.router.add_post("/ui/{key}", companion.ui)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", args.port)
    await site.start()
    companion.port = site._server.sockets[0].getsockname()[1]
    config = companion.directory / "connection.json"
    with open(config, "w", opener=lambda p, f: os.open(p, f, 0o600)) as out:
        json.dump({"port": companion.port, "key": companion.key}, out)
    try:
        if args.managed:
            from setup import Setup
            companion.setup = Setup(companion)
            await companion.setup.detect()
            loop = asyncio.get_running_loop()
            def input_closed():
                if not os.read(sys.stdin.fileno(), 4096):
                    loop.remove_reader(sys.stdin.fileno())
                    companion.stopping.set()
            loop.add_reader(sys.stdin.fileno(), input_closed)
            print(json.dumps({"protocol": 1, "port": companion.port, "key": companion.key}), flush=True)
        else:
            await companion.start_browser(visible=not args.headless)
        if args.headless:
            for _ in range(80):
                try:
                    await companion.account()
                    break
                except RuntimeError:
                    await asyncio.sleep(0.25)
            else:
                raise RuntimeError(
                    "Sign in with Qobuz browser before starting without a window"
                )
        if not args.managed:
            print("Qobuz browser ready", flush=True)
        for sig in (signal.SIGINT, signal.SIGTERM):
            asyncio.get_running_loop().add_signal_handler(sig, companion.stopping.set)
        await companion.stopping.wait()
    finally:
        await companion.close()
        await runner.cleanup()


if __name__ == "__main__":
    from launch import storage_paths

    data, cache = storage_paths()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, default=data)
    parser.add_argument("--cache", type=Path, default=cache)
    parser.add_argument("--profile", type=Path)
    parser.add_argument(
        "--firefox", default=shutil.which("firefox") or "/opt/firefox/firefox"
    )
    parser.add_argument("--mode", choices=["native", "flatpak"], default="native")
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--managed", action="store_true")
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg"))
    parser.add_argument("--headless", action="store_true")
    parser.add_argument("--port", type=int, default=48639)
    args = parser.parse_args()
    args.profile = args.profile or args.data / "firefox"
    if not args.worker and not args.headless and not args.managed:
        from wizard import show

        raise SystemExit(show(args))
    if not args.ffmpeg and not args.managed:
        parser.error("Install FFmpeg before starting Qobuz browser")
    if not 0 <= args.port <= 65535:
        parser.error("Invalid port")
    try:
        asyncio.run(run(args))
    except KeyboardInterrupt:
        pass
    except Exception:
        raise SystemExit(
            "Qobuz browser could not start. Check that Firefox is installed and its Qobuz profile is not already open."
        )
