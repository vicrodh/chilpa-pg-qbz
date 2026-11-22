import asyncio
import json
from pathlib import Path
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch, AsyncMock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from aiohttp import web
from aiohttp.test_utils import TestClient, TestServer
from companion import Companion, Job
from driver import BrowserSessionExpired
from launch import command, normalize, storage_paths
from pcm import header


class FormatTests(unittest.TestCase):
    def test_wave_preserves_rate_depth_and_frame_count(self):
        prefix, size = header(
            dict(bits=24, channels=2, sample_rate=192000, frames=97994240)
        )
        self.assertEqual(size, 587965440)
        self.assertEqual(len(prefix), 44)
        self.assertEqual(struct.unpack_from("<I", prefix, 24)[0], 192000)
        self.assertEqual(struct.unpack_from("<H", prefix, 34)[0], 24)
        self.assertEqual(struct.unpack_from("<I", prefix, 40)[0], size)

    def test_rejects_unsupported_or_impossible_formats(self):
        for changes in [
            dict(bits=32),
            dict(channels=0),
            dict(frames=0),
            dict(frames=2**40),
        ]:
            with self.assertRaises(ValueError):
                header(
                    dict(bits=24, channels=2, sample_rate=192000, frames=100, **{})
                    | changes
                )

    def test_flatpak_grant_is_scoped_and_paths_are_arguments(self):
        with patch("launch.sys.platform", "linux"), patch(
            "launch.shutil.which", return_value="/usr/bin/flatpak"
        ):
            result = command("flatpak", "", Path("/tmp/QBZ profile"), 12345, True)
        profile = str(Path("/tmp/QBZ profile").resolve())
        self.assertIn("--filesystem=" + profile, result)
        self.assertEqual(result[result.index("--profile") + 1], profile)
        self.assertNotIn("override", result)
        self.assertIn("--headless", result)
        self.assertEqual(
            normalize("/Applications/Firefox.app"),
            Path("/Applications/Firefox.app/Contents/MacOS/firefox"),
        )


class RelayTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        args = SimpleNamespace(data=root / "data", cache=root / "cache", port=0)
        self.companion = Companion(args)
        app = web.Application(client_max_size=65536)
        app.router.add_post("/session", self.companion.rpc)
        app.router.add_post("/stop", self.companion.rpc)
        app.router.add_post("/warm", self.companion.rpc)
        app.router.add_get("/audio/{ticket}", self.companion.media)
        self.client = TestClient(TestServer(app))
        await self.client.start_server()

    async def asyncTearDown(self):
        await self.client.close()
        await self.companion.close()
        self.temp.cleanup()

    async def test_rpc_rejects_missing_key_before_launching_browser(self):
        result = await self.client.post("/session", json={})
        self.assertEqual(result.status, 403)
        self.assertIsNone(self.companion.browser)

    async def test_warming_requires_authorization_and_completed_login(self):
        self.companion.warm_browser = AsyncMock()
        response = await self.client.post("/warm", json={})
        self.assertEqual(response.status, 403)
        self.companion.setup = SimpleNamespace(phase="waiting", cancel=AsyncMock())
        response = await self.client.post("/warm", headers={"Authorization": "Bearer " + self.companion.key}, json={})
        self.assertFalse((await response.json())["accepted"])
        self.companion.warm_browser.assert_not_awaited()

    async def test_browsing_does_not_wait_for_firefox_or_start_duplicate_warming(self):
        entered, release = asyncio.Event(), asyncio.Event()
        async def warm():
            entered.set()
            await release.wait()
        self.companion.setup = SimpleNamespace(phase="ready", cancel=AsyncMock())
        self.companion.warm_browser = AsyncMock(side_effect=warm)
        for _ in range(2):
            response = await asyncio.wait_for(self.client.post("/warm", headers={"Authorization": "Bearer " + self.companion.key}, json={}), 1)
            self.assertTrue((await response.json())["accepted"])
            await entered.wait()
        self.companion.warm_browser.assert_awaited_once()
        release.set()
        await self.companion.warm_task

    async def test_warming_prepares_session_without_selecting_output_or_playing(self):
        browser = SimpleNamespace(wait_ready=AsyncMock(), local=AsyncMock(), start_capture=AsyncMock(), close=AsyncMock())
        self.companion.browser = browser
        self.companion.start_browser = AsyncMock()
        self.companion.account = AsyncMock()
        await self.companion.warm_browser()
        browser.wait_ready.assert_awaited_once()
        self.companion.account.assert_awaited_once()
        browser.local.assert_not_awaited()
        browser.start_capture.assert_not_awaited()
        self.assertIsNotNone(self.companion.idle_task)

    async def test_shutdown_cancels_warming_and_its_idle_timer(self):
        waiting = asyncio.Event()
        self.companion.start_browser = AsyncMock(side_effect=waiting.wait)
        self.companion.warm_task = asyncio.create_task(self.companion.warm_browser())
        await asyncio.sleep(0)
        await self.companion.close()
        self.assertIsNone(self.companion.warm_task)
        self.assertIsNone(self.companion.idle_task)

    async def test_expired_browser_session_returns_an_actionable_error(self):
        self.companion.setup = SimpleNamespace(phase="ready", cancel=AsyncMock())
        self.companion.session = {"fixture": True}
        self.companion.account = AsyncMock(side_effect=BrowserSessionExpired("fixture"))
        response = await self.client.post("/session", headers={"Authorization": "Bearer " + self.companion.key}, json={})
        self.assertEqual(response.status, 401)
        self.assertEqual((await response.json())["code"], "session_expired")
        self.assertEqual(self.companion.setup.phase, "login")
        self.assertIsNone(self.companion.session)

    async def test_expired_session_releases_the_audio_preparation(self):
        self.companion.setup = SimpleNamespace(phase="ready", cancel=AsyncMock())
        self.companion.start_browser = AsyncMock(side_effect=BrowserSessionExpired("fixture"))
        job = Job("42", 27, "fixture", self.companion.cache / "fixture.wav")
        await self.companion.prepare(job)
        self.assertTrue(job.first.is_set())
        self.assertTrue(job.finished.is_set())
        self.assertEqual(job.error_code, "session_expired")
        self.assertEqual(self.companion.setup.phase, "login")

    async def test_ranges_head_and_invalid_ranges(self):
        path = self.companion.cache / "test.wav"
        path.write_bytes(b"0123456789")
        job = Job("1", 27, "test", path)
        job.size = job.available = 10
        job.first.set()
        job.finished.set()
        self.companion.tickets["test"] = job
        self.companion.jobs[("1", 27)] = job
        response = await self.client.get("/audio/test", headers={"Range": "bytes=2-5"})
        self.assertEqual(response.status, 206)
        self.assertEqual(response.headers["Content-Range"], "bytes 2-5/10")
        self.assertEqual(await response.read(), b"2345")
        response = await self.client.head("/audio/test")
        self.assertEqual(response.headers["Content-Length"], "10")
        self.assertEqual(await response.read(), b"")
        for value in ["bytes=20-", "bytes=8-2", "bytes=1-2,4-5"]:
            response = await self.client.get("/audio/test", headers={"Range": value})
            self.assertEqual(response.status, 416)
        response = await self.client.post("/audio/test")
        self.assertEqual(response.status, 405)
        self.assertEqual(job.readers, 0)

    async def test_old_stop_ticket_cannot_cancel_a_new_preparation(self):
        job = Job("42", 27, "new", self.companion.cache / "pending.wav")
        job.task = asyncio.create_task(asyncio.sleep(100))
        self.companion.jobs[("42", 27)] = job
        self.companion.tickets["new"] = job
        response = await self.client.post(
            "/stop",
            headers={"Authorization": "Bearer " + self.companion.key},
            json={"ticket": "old"},
        )
        self.assertEqual(response.status, 200)
        self.assertFalse(job.task.cancelled())
        response = await self.client.post(
            "/stop",
            headers={"Authorization": "Bearer " + self.companion.key},
            json={"ticket": "new"},
        )
        self.assertEqual(response.status, 200)
        await asyncio.sleep(0)
        self.assertTrue(job.task.cancelled())

    async def test_pending_preparation_returns_a_ticket_without_inventing_format(self):
        job = Job("42", 27, "pending", self.companion.cache / "pending.wav")
        job.first.wait = AsyncMock(side_effect=asyncio.TimeoutError)
        self.companion.jobs[("42", 27)] = job
        self.companion.account = AsyncMock(
            return_value={"session": {"entitlements": {"lossless_streaming": True}}}
        )
        response = await self.companion.resolve({"track": "42", "quality": 27})
        result = json.loads(response.text)
        self.assertEqual(response.status, 200)
        self.assertEqual(result["format"], {"codec": "wav"})
        self.assertTrue(result["url"].endswith("/audio/pending"))

    async def test_catalog_metadata_is_bound_to_the_requested_track(self):
        self.companion.account = AsyncMock(return_value={"session": {"entitlements": {"lossless_streaming": True}}})
        async def prepared(job):
            job.first.set()
            job.finished.set()
        self.companion.prepare = AsyncMock(side_effect=prepared)
        for index, metadata in enumerate([
            {"track":"42","duration_ms":180000,"available":True},
            {"track":"other","duration_ms":180000,"available":True},
            {"track":"44","duration_ms":True,"available":True},
            {"track":"45","duration_ms":180000},
        ]):
            track = str(42 + index)
            self.companion.jobs.clear()
            await self.companion.resolve({"track":track,"quality":27,"metadata":metadata})
            supplied = self.companion.jobs[(track,27)].metadata
            if index == 0:
                self.assertEqual(supplied, {"duration_ms":180000,"available":True})
            else:
                self.assertIsNone(supplied)

    async def test_unfinished_range_waits_for_real_bytes(self):
        path = self.companion.cache / "test.wav"
        path.write_bytes(b"01")
        job = Job("1", 27, "test", path)
        job.size = 4
        job.available = 2
        job.first.set()
        self.companion.tickets["test"] = job
        self.companion.jobs[("1", 27)] = job
        response = await self.client.get("/audio/test")
        reader = asyncio.create_task(response.read())
        await asyncio.sleep(0.02)
        self.assertFalse(reader.done())
        with path.open("ab") as out:
            out.write(b"23")
        async with job.changed:
            job.available = 4
            job.first.set()
            job.finished.set()
            job.changed.notify_all()
        self.assertEqual(await asyncio.wait_for(reader, 1), b"0123")


if __name__ == "__main__":
    unittest.main()
