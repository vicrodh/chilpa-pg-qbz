"""Settings do not start a browser before the login action."""
import asyncio
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from setup import Setup
from driver import Browser, BrowserPageOperationError


class SetupTests(unittest.IsolatedAsyncioTestCase):
    async def test_page_errors_never_retain_exception_text_or_stream_addresses(self):
        browser = Browser(Path("unused"), "fixture")
        browser.context = "fixture"
        private_text = "TypeError: currentTime undefined at https://example.invalid/private-stream?token=fixture-secret"
        browser.command = AsyncMock(return_value={"type":"exception", "exceptionDetails":{"text":private_text}})
        with self.assertRaises(BrowserPageOperationError) as caught:
            await browser.evaluate("JSON.stringify(window.__relay.drain())")
        error = caught.exception
        self.assertEqual(error.operation, "drain")
        self.assertEqual(error.terms, ["TypeError", "currentTime", "undefined"])
        self.assertNotIn("fixture-secret", str(error) + repr(vars(error)))

    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.executable = self.root / "Firefox with spaces"
        self.executable.write_text("fixture")
        self.executable.chmod(0o700)
        self.companion = SimpleNamespace(directory=self.root, args=SimpleNamespace(mode="native", firefox="", ffmpeg="/usr/bin/ffmpeg"),
            browser=None, jobs={}, idle_task=None, browser_lock=asyncio.Lock(), start_browser=AsyncMock(),
            account=AsyncMock(), session=None)
        self.setup = Setup(self.companion)

    async def asyncTearDown(self):
        await self.setup.cancel()
        self.temp.cleanup()

    async def test_detection_and_restart_preserve_verified_state_without_opening_firefox(self):
        with patch("setup.candidates", return_value=[str(self.executable)]), patch("setup.flatpak_available", return_value=False):
            self.assertEqual((await self.setup.detect())["phase"], "login")
            self.companion.start_browser.assert_not_called()
            await self.setup.action({"action": "verified"})
            again = Setup(self.companion)
            self.assertEqual((await again.detect())["phase"], "ready")
            self.assertTrue(json.loads(again.file.read_text())["verified"])

    async def test_portal_picker_resolves_the_original_installation(self):
        with patch("launch.os.getxattr", return_value=os.fsencode(str(self.executable)), create=True):
            await self.setup.action({"action": "select", "path": "file:///run/user/1000/doc/example/browser"})
        self.assertEqual(self.companion.args.firefox, str(self.executable))

    async def test_picker_decodes_spaces_and_refuses_remote_files(self):
        await self.setup.action({"action": "select", "path": self.executable.as_uri()})
        self.assertEqual(self.companion.args.firefox, str(self.executable))
        with self.assertRaises(ValueError):
            self.setup.select("native", "file://remote.example/firefox")

    async def test_missing_decoder_is_recoverable_in_the_detection_step(self):
        self.companion.args.ffmpeg = None
        with patch("setup.candidates", return_value=[str(self.executable)]), patch("setup.flatpak_available", return_value=False):
            state = await self.setup.detect()
        self.assertEqual(state["phase"], "detect")
        self.assertTrue(state["missing_decoder"])
        self.companion.start_browser.assert_not_called()
        state = await self.setup.action({"action": "select", "ffmpeg": self.executable.as_uri()})
        self.assertEqual(state["phase"], "login")
        self.assertFalse(state["missing_decoder"])

    async def test_flatpak_shutdown_targets_only_the_owned_instance(self):
        browser = Browser(self.root, "", mode="flatpak")
        browser.instance = "12345678"
        browser.process = SimpleNamespace(returncode=0)
        killer = SimpleNamespace(wait=AsyncMock(return_value=0))
        with patch("driver.asyncio.create_subprocess_exec", new=AsyncMock(return_value=killer)) as spawn:
            await browser.close()
        self.assertEqual(spawn.call_args.args, ("flatpak", "kill", "12345678"))
        self.assertIsNone(browser.instance)

    async def test_graceful_shutdown_finishes_before_process_termination(self):
        browser = Browser(self.root, "fixture")
        browser.ws = SimpleNamespace(close=AsyncMock())
        browser.command = AsyncMock()
        process = SimpleNamespace(returncode=None, terminate=unittest.mock.Mock(), kill=unittest.mock.Mock())
        async def finish():
            process.returncode = 0
        process.wait = AsyncMock(side_effect=finish)
        browser.process = process
        await browser.close()
        process.wait.assert_awaited_once()
        process.terminate.assert_not_called()
        process.kill.assert_not_called()

    async def test_flatpak_profile_finishes_flushing_before_instance_cleanup(self):
        browser = Browser(self.root, "", mode="flatpak")
        browser.instance = "owned-instance"
        browser.ws = SimpleNamespace(close=AsyncMock())
        browser.command = AsyncMock()
        process = SimpleNamespace(returncode=None)
        async def finish():
            process.returncode = 0
        process.wait = AsyncMock(side_effect=finish)
        browser.process = process
        async def cleanup(*args, **kwargs):
            self.assertEqual(process.returncode, 0)
            self.assertEqual(args, ("flatpak", "kill", "owned-instance"))
            return SimpleNamespace(wait=AsyncMock(return_value=0))
        with patch("driver.asyncio.create_subprocess_exec", new=cleanup):
            await browser.close()
        process.wait.assert_awaited_once()

    async def test_cancel_closes_only_the_owned_browser_and_stops_login(self):
        browser = SimpleNamespace(process=SimpleNamespace(returncode=None), close=AsyncMock(), remember_session=AsyncMock())
        async def start(**_):
            self.companion.browser = browser
        self.companion.start_browser.side_effect = start
        self.companion.account.side_effect = RuntimeError("Not signed in")
        await self.setup.action({"action": "login"})
        await asyncio.sleep(0.01)
        self.assertEqual(self.setup.phase, "waiting")
        await self.setup.action({"action": "cancel"})
        browser.close.assert_awaited_once()
        self.assertEqual(self.setup.phase, "login")
        self.assertIsNone(self.setup.task)

    async def test_sign_in_does_not_close_an_active_preparation(self):
        browser = SimpleNamespace(close=AsyncMock())
        self.companion.browser = browser
        self.companion.jobs = {"active": SimpleNamespace(finished=asyncio.Event())}
        with self.assertRaises(ValueError):
            await self.setup.action({"action": "login"})
        browser.close.assert_not_called()
        self.assertIsNone(self.setup.task)

    async def test_success_returns_to_credentials_step_and_closes_firefox(self):
        browser = SimpleNamespace(process=SimpleNamespace(returncode=None), close=AsyncMock(), remember_session=AsyncMock())
        async def start(**_):
            self.companion.browser = browser
        self.companion.start_browser.side_effect = start
        await self.setup.action({"action": "login"})
        await self.setup.task
        self.assertEqual(self.setup.phase, "verify")
        browser.remember_session.assert_awaited_once()
        self.assertEqual(browser.close.await_count, 2)
        self.assertEqual(self.companion.start_browser.await_count, 2)
        self.assertEqual(self.companion.account.await_count, 2)

    async def test_cold_playback_waits_for_player_before_selecting_local_output(self):
        browser = Browser(self.root, "fixture")
        browser.evaluate = AsyncMock(side_effect=[False, False, True, True])
        with patch("driver.asyncio.sleep", new=AsyncMock()):
            await browser.local()
        self.assertEqual([call.args[0] for call in browser.evaluate.await_args_list],
                         ["!!window.__relay?.ready()"] * 3 + ["window.__relay.local()"])

    async def test_unready_player_times_out_without_selecting_output(self):
        browser = Browser(self.root, "fixture")
        browser.evaluate = AsyncMock(return_value=False)
        with patch("driver.asyncio.sleep", new=AsyncMock()):
            with self.assertRaises(RuntimeError):
                await browser.local()
        self.assertEqual(browser.evaluate.await_count, 61)

    async def test_empty_capture_retries_once_and_preserves_the_first_media_reply(self):
        browser = Browser(self.root, "fixture")
        empty = json.dumps({"data": "", "error": "", "format": None})
        first = {"data": "AQID", "format": {"bits": 24}, "error": ""}
        browser.evaluate = AsyncMock(side_effect=[None, None] + [True, empty] * 60 + [None, None, True, json.dumps(first)])
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        with patch("driver.asyncio.sleep", new=AsyncMock()):
            self.assertEqual(await browser.start_capture("42", 27), first)
        browser.reset_player.assert_awaited_once()
        browser.local.assert_awaited_once()

    async def test_parallel_preparation_does_not_restart_the_requested_track(self):
        browser = Browser(self.root, "fixture")
        metadata = {"available": True, "duration_ms": 180000}
        media = {"data": "AQID", "error": "", "format": {"sample_rate": 48000, "frames": 8640000}}
        browser.evaluate = AsyncMock(side_effect=[json.dumps(metadata), True, json.dumps(media)])
        self.assertEqual(await browser.prepare_capture("42", 27), (metadata, media))
        expressions = [call.args[0] for call in browser.evaluate.await_args_list]
        self.assertFalse(any(".begin(" in expression or ".seek(" in expression for expression in expressions))
        self.assertTrue(browser.evaluate.await_args_list[0].kwargs["user_activation"])

    async def test_catalog_metadata_avoids_a_second_metadata_request(self):
        browser = Browser(self.root, "fixture")
        metadata = {"available": True, "duration_ms": 180000}
        media = {"data": "AQID", "error": "", "format": {"sample_rate": 48000, "frames": 8640000}}
        browser.evaluate = AsyncMock(side_effect=[None, None, True, json.dumps(media)])
        self.assertEqual(await browser.prepare_capture("42", 27, metadata=metadata), (metadata, media))
        expressions = [call.args[0] for call in browser.evaluate.await_args_list]
        self.assertFalse(any(".metadata(" in expression or ".prepare(" in expression for expression in expressions))
        self.assertEqual(expressions.count("window.__zbq.seek(0)"), 1)

    async def test_parallel_preparation_rejects_metadata_before_draining_audio(self):
        for metadata in ({"available": False, "duration_ms": 180000},
                         {"available": True, "duration_ms": 0},
                         {"available": True, "duration_ms": "180000"}):
            with self.subTest(metadata=metadata):
                browser = Browser(self.root, "fixture")
                browser.evaluate = AsyncMock(side_effect=[json.dumps(metadata), None])
                with self.assertRaises(RuntimeError):
                    await browser.prepare_capture("42", 27)
                self.assertEqual(browser.evaluate.await_args_list[-1].args[0], "window.__relay.stop()")
                self.assertFalse(any("drain" in call.args[0] for call in browser.evaluate.await_args_list))

    async def test_parallel_preparation_retries_a_mismatched_stream_from_zero(self):
        browser = Browser(self.root, "fixture")
        metadata = {"available": True, "duration_ms": 180000}
        wrong = {"data": "wrong", "error": "", "format": {"sample_rate": 48000, "frames": 48000 * 30}}
        right = {"data": "right", "error": "", "format": {"sample_rate": 48000, "frames": 48000 * 180}}
        browser.evaluate = AsyncMock(side_effect=[json.dumps(metadata), True, json.dumps(wrong), None, None, True, json.dumps(right)])
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        self.assertEqual(await browser.prepare_capture("42", 27), (metadata, right))
        browser.reset_player.assert_awaited_once()
        expressions = [call.args[0] for call in browser.evaluate.await_args_list]
        self.assertEqual(expressions.count("window.__zbq.seek(0)"), 1)

    async def test_empty_capture_stops_after_one_retry(self):
        browser = Browser(self.root, "fixture")
        browser.evaluate = AsyncMock(return_value=json.dumps({"data": "", "error": "", "format": None}))
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        with patch("driver.asyncio.sleep", new=AsyncMock()):
            with self.assertRaises(RuntimeError):
                await browser.start_capture("42", 27)
        browser.reset_player.assert_awaited_once()
        self.assertEqual(browser.evaluate.await_count, 244)

    async def test_output_restored_after_play_selects_local_before_accepting_media(self):
        browser = Browser(self.root, "fixture")
        first = {"data": "AQID", "format": {"bits": 24}, "error": ""}
        browser.evaluate = AsyncMock(side_effect=[None, None, False, None, None, json.dumps(first)])
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        self.assertEqual(await browser.start_capture("42", 27), first)
        browser.reset_player.assert_not_awaited()
        browser.local.assert_awaited_once()
        expressions = [call.args[0] for call in browser.evaluate.await_args_list]
        self.assertEqual(expressions.count("JSON.stringify(window.__relay.drain())"), 1)
        self.assertEqual(expressions.count("window.__zbq.seek(0)"), 2)

    async def test_repeated_output_changes_have_a_bounded_retry_budget(self):
        browser = Browser(self.root, "fixture")
        async def evaluate(expression, **_):
            if expression == "window.__relay.localSelected()":
                return False
            return json.dumps({"data": "", "error": "", "format": None})
        browser.evaluate = AsyncMock(side_effect=evaluate)
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        with patch("driver.asyncio.sleep", new=AsyncMock()):
            with self.assertRaises(RuntimeError):
                await browser.start_capture("42", 27)
        browser.reset_player.assert_awaited_once()
        self.assertEqual(browser.local.await_count, 5)
        self.assertEqual(browser.evaluate.await_count, 22)

    async def test_previous_track_media_is_discarded_before_retrying(self):
        browser = Browser(self.root, "fixture")
        previous = {"data": "old", "format": {"sample_rate": 48000, "frames": 48000 * 30}, "error": ""}
        requested = {"data": "new", "format": {"sample_rate": 48000, "frames": 48000 * 180}, "error": ""}
        browser.evaluate = AsyncMock(side_effect=[None, None, True, json.dumps(previous), None, None, True, json.dumps(requested)])
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        self.assertEqual(await browser.start_capture("42", 27, duration_ms=180000), requested)
        browser.reset_player.assert_awaited_once()

    async def test_repeated_wrong_track_is_never_returned_as_audio(self):
        browser = Browser(self.root, "fixture")
        previous = {"data": "old", "format": {"sample_rate": 48000, "frames": 48000 * 30}, "error": ""}
        browser.evaluate = AsyncMock(side_effect=[None, None, True, json.dumps(previous)] * 2)
        browser.reset_player = AsyncMock()
        browser.local = AsyncMock()
        with self.assertRaises(RuntimeError):
            await browser.start_capture("42", 27, duration_ms=180000)
        browser.reset_player.assert_awaited_once()

    async def test_failed_cold_session_check_does_not_report_login_success(self):
        browser = SimpleNamespace(process=SimpleNamespace(returncode=None), close=AsyncMock(), remember_session=AsyncMock())
        async def start(**_):
            self.companion.browser = browser
        self.companion.start_browser.side_effect = start
        self.companion.account.side_effect = [None, RuntimeError("Session was not retained")]
        await self.setup.action({"action": "login"})
        await self.setup.task
        self.assertEqual(self.setup.phase, "login")
        self.assertTrue(self.setup.problem)
        self.assertEqual(browser.close.await_count, 2)
