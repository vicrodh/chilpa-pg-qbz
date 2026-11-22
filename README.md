# Qobuz for Chilpa — pg-qbz

`pg-qbz` is a process source plugin with Qobuz search, library, playlists and discovery,
and a host-managed Firefox companion for sign-in and audio preparation. It has its
own identity, `io.github.vicrodh.qbz`, and does not replace another Qobuz plugin.
The package is named `pg-qbz`; the sidebar and search advertise it as Qobuz.

This is a local pre-release. No catalog entry or public release has been made.

Opening Qobuz pages, albums, artists, playlists or search prepares Firefox in the
background. This moves browser startup ahead of Play without selecting an audio
output or fetching a track. Firefox still closes after 45 seconds without further
browsing or audio preparation. Playing immediately from a cold browser still
requires startup; service latency and the bounded stream retry remain.

Playback reuses duration, availability and normalization already returned by the
catalog API. These bounded in-memory records expire with the catalog reply or
within five minutes, and clear when the account is changed. Missing metadata is
fetched alongside browser media preparation; availability and full-duration checks still run before
audio is exposed to Chilpa. Complete PCM is also checked against the FLAC checksum.

## Setup on Linux

1. Install `0.1.4-linux-x86_64.zip` in a Chilpa build with the desktop companion
   contract. Approve the displayed permissions, including the desktop helper.
2. Open Qobuz settings. Chilpa starts the bundled helper and detects Firefox. If
   Firefox is missing, choose its installation using the native file picker.
3. Press **Sign in with Firefox**. Sign in on Qobuz's own page. When the session is
   detected, the plugin retains it in Firefox’s private profile, waits for Firefox
   to finish shutting down and flush that profile,
   and verifies a cold browser restart before activating the account automatically.
4. Wait for **Qobuz is connected**. Search and the Qobuz sidebar pages are then
   ready for playback. No second confirmation, companion application, terminal
   launch, connection code or separate download is needed.

The **same plugin ZIP** serves native Chilpa and Chilpa Flatpak. On Flatpak, the
wizard shows a copyable permission command only when host process access is
unavailable. Restart Chilpa after applying the displayed permission. This grants
Chilpa the ability to launch host processes; it is broader than browser-only
access. The permission is not applied silently.

Firefox Flatpak receives a per-launch grant limited to the plugin's profile directory.
Firefox and FFmpeg are external dependencies; Python and the helper libraries are
inside the plugin package. The helper is owned by Chilpa and stops when the plugin
is disabled or Chilpa exits. No second GUI or tray application is involved.

A subscription with lossless streaming is required. Previews and unsupported
formats fail explicitly rather than being presented as complete lossless tracks.

## Audio

Firefox uses the official web player in an isolated profile and selects its local
output, corrects a late QConnect output restore before accepting media, and starts
each requested track at position zero. The companion captures its lossless MSE data before browser rendering.
FFmpeg decodes it without gain, filters, or resampling into a progressive PCM WAV
with the original depth, sample rate, channel count and exact frame count.
The source FLAC checksum is checked after the complete track is prepared when
present. A late error aborts delivery; no padding is synthesized.

Chilpa owns audible playback, device selection, volume, seeking, queue and casting.
Bit-perfect playback still depends on Chilpa's output settings and device path.
Local gapless uses Chilpa's existing queue and prefetch contract; Firefox never
opens the audio output. DLNA uses Chilpa's existing cast transport, whose current
implementation does not guarantee gapless transitions. QConnect remote queue
control is a separate host feature and is not supplied by this plugin.

Preparation is bounded: two tracks, 2 GiB total cache, 1 GiB PCM and 480 MiB captured
compressed data per track, three minutes per preparation. Long recordings that
exceed these budgets fail. Firefox closes after 45 seconds without preparation
and is restarted when needed. Its first startup can delay playback.

## Accounts and privacy

Provider sign-in stays in the companion's Firefox profile. The plugin retains
the official web player’s account in that origin’s own browser storage, so a
non-remembered sign-in survives the browser restart required for playback. The plugin receives a
validated provider session in memory for catalog requests. The host passes a
private loopback port and random key directly to the plugin during initialization;
these never enter the settings document. Audio tickets are temporary,
loopback-only capabilities. Chilpa supplies its own LAN proxy for casting.

**Disconnect** clears the plugin's validated account state. The private Firefox
profile retains its browser session, so a later login may reuse it. The helper
never imports normal Firefox profiles. Uninstalling the plugin removes its owned
browser profile and cache along with the plugin's other data.

Host playback events update the plugin's local recent history. Provider end-of-
stream reporting is incomplete: the browser relay does not currently return the
provider's reporting receipt, so no end receipt is fabricated. Browser UI changes
may break capture; this integration uses internal web-player interfaces.

## Build and verification

```sh
python3 -m venv .venv
.venv/bin/pip install -r browser/requirements.txt pyinstaller==6.19.0
.venv/bin/python tools/build-browser
python3 tools/build-native
python3 -m unittest discover -s browser/tests -v
node browser/tests/capture.cjs
cargo test --locked -p pg-qbz --lib
```

`tools/smoke-setup` installs the exact ZIP in an isolated host and exercises the
native login controls, automatic return, cold-session verification and disable lifecycle.
It also checks the Flatpak permission-denied state. A test can explicitly select
an existing authorized Firefox profile; it does not copy credentials.
`tools/smoke-host` runs the managed login followed by playback, pause, seek, a
genuine next-track transition and stop. It can also measure local gapless playback
with an empty cache. Its output defaults to a file sink.

`tools/smoke-host --catalog` also activates the real sidebar entry, opens discovery,
global search, then plays from a search result. Library and edit contracts retain
fixture coverage; complete live collection rendering has not been revalidated.

See `compatibility.json` for reviewed SDK/host pins and the validation matrix.
Linux x86_64 and macOS Apple Silicon have local test packages. The macOS package
passed the RPC and managed-helper checks, including Firefox and FFmpeg detection
and shutdown. The real macOS host completed sign-in in an isolated profile and
loaded the exact ZIP; real playback still needs validation there.
Build the macOS helper with Python 3.14 and the same commands above. Current
Chilpa process plugins do not run on Windows; a Firefox `.exe` picker does not
remove that host limitation. These packages have not been published.

## Provenance

The catalog implementation derives from the Chilpa Qobuz source plugin. Firefox
integration adapts ZBQ v0.1.1's early injection and action bridge, with an independent
bounded capture and relay. See `NOTICE`, `COPYING`, `LICENSE-INDEX` and
`browser/vendor/LICENSE`. The complete plugin is GPL-3.0-or-later; retained
components keep their notices.

## Upgrading from the login-only prototype

Install the new ZIP over the existing plugin. Its ID and private profile are
unchanged. If the previous login did not retain its browser session, sign in once
again; the new wizard stores and checks that session before showing Connected.
An expired browser session asks for sign-in instead of treating every song as an
unavailable track. The icon is the original monochrome QBZ tray icon, with transparent cutouts for host tinting.
