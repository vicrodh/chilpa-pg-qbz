# Architecture

The native process plugin speaks Chilpa's JSON-line RPC protocol. The host grants
source registration, settings, own-account storage and the listed network domains.
Catalog IDs, navigation, favorites and playlist operations retain the existing
source contract. Provider sessions are configured from the companion, not from
an application secret or a separate OAuth callback flow.

QBZ Browser is a packaged desktop helper launched by Chilpa after approval of
`desktop_companion`. The host owns its stdin lifetime; the helper owns one Firefox instance,
one serialized BiDi session, and a bounded audio cache. Only loopback is bound.
RPC calls require a private bearer key; audio reads require an unguessable ticket.
No access logs contain credentials or media URLs. Responses have size/time bounds.

Firefox installation discovery and profile paths are platform adapters. An
external Firefox Flatpak is launched with a grant to the selected QBZ profile.
Its instance ID is obtained through `--instance-id-fd`; shutdown closes only
that instance. `--die-with-parent` also ties it to the managed helper.
After a cold launch, playback waits for the web player before selecting its local output.
If no initial media arrives within six seconds, it reloads the player once.
A second empty attempt fails and releases the preparation slot. No retry is
attempted after media has arrived.
The sandboxed source process does not launch host applications. The host uses
`flatpak-spawn --host --watch-bus` only for the separately approved helper and
only when the sandbox permission is present.
All connections use the host network namespace already granted to a networked
process plugin. A single plugin ZIP serves native and Flatpak hosts.

The capture hook owns buffers accepted by the current track's SourceBuffer.
Moving the paused media cursor within buffered data requests subsequent segments
without sending audio to a device. A 32 MiB queue transfers at most 1 MiB per
BiDi reply. FFmpeg decodes the FLAC data into integer PCM. The FLAC STREAMINFO
supplies total frames, allowing valid HTTP Content-Length and byte ranges before
preparation completes. Readers wait for actual bytes; no artificial silence is
inserted. The final decoded PCM MD5 is compared with STREAMINFO when available.

The player's audible duration remains authoritative for listening events. Browser
buffer movement is never converted into an audible-duration report. Full provider
reporting awaits a supported reporting receipt from the browser integration.

The companion's WAV transport deliberately exposes the delivered container as WAV;
it does not label decoded PCM as a compressed FLAC stream. Original sample depth
and rate remain available to the host's normal output negotiation. Seeking and
casting operate on exact PCM byte ranges. Gapless depends on timely host prefetch;
remote QConnect queue ownership is outside this source contract.

The helper emits one bounded JSON greeting (`protocol`, `port`, `key`) on stdout.
The host forwards it as `initialize.desktop_companion`, never as a UI value. EOF
on stdin shuts down the helper and its browser. Failed startup returns a diagnostic
instead of preventing the plugin settings from opening. The host restarts the
helper with the source process after a permission, network or package change.

The native form uses three steps: detection, Firefox login, automatic credential validation.
Login retains the account using the official web player’s `localuser` storage
shape inside the private Firefox profile, then verifies it after a cold restart.
Only a confirmed restart can mark the account ready. Catalogue access can finish
activation if the listener leaves the wizard before its next refresh.
Path controls return local file URLs from a native chooser. Permission commands
are read-only copy controls. Login status refresh runs only on the visible form;
the helper's login job has a ten-minute deadline and an explicit cancel action.
Verified browser preferences persist under the host-owned plugin desktop folder.
