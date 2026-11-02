# Sonante LAN remote — API v1 (beta)

The desktop application remains the player. The web controller sends commands;
no audio is streamed to the browser. The same versioned HTTP API is intended for
a future Android controller. The initial web UI has artwork and poster layouts,
transport, seeking, supported volume control, and a paginated playback queue.
Library browsing and audio-reactive visualizers are outside this first phase.

## Enable and pair

Enable and save **Preferences → General → Experimental features**, then reopen
preferences and enable **Remote control**. Apply an unprivileged port (default
8787). These are separate opt-ins; enabling experimental features alone does not
start a network listener. Saving experimental features as disabled stops remote
access, closes the listener, revokes paired devices and turns off its saved
network preference. Open the displayed IPv4 address from a device
on the same local network. Sonante must remain running.

Generate a pairing code on the desktop. It lasts 120 seconds, permits one pairing,
and closes after five incorrect guesses. The QR code contains the access URL and
temporary code in the URL fragment; it contains no persistent bearer token.
Up to eight authorized devices can connect. Pairing survives desktop restarts for up to 30 days. Only token hashes, device
names and absolute expiry times are saved in a private credentials file; bearer
tokens remain in the browser. Revocation and disabling the listener clear saved
credentials. Changing the port retains authorization, but browser storage belongs
to its original IP/port origin. Pairing codes and command replay results remain
runtime-only.

This first phase uses HTTP on a trusted local network, without transport
encryption. It accepts only private/link-local/loopback IPv4 peers and literal-IP
Host headers matching the accepted socket. It is not an Internet-facing service.
There is no router configuration, public discovery, or external QR service.

## Requests

Use relative paths with the displayed IP/port as origin. JSON POST requests must
have `Content-Type: application/json`. After pairing, send
`Authorization: Bearer <token>` in a header, never in a URL. Browser requests must
be same-origin; cookies and CORS authentication are not supported. Native Android
clients can omit Origin. All responses are `Cache-Control: no-store`.

- `POST /api/v1/pair` — `{ "code": "<six digits>", "name": "My phone" }`.
  Returns `{ "schema": 1, "token": "<64 hex characters>", "expires_in": 2592000 }`.
- `GET /api/v1/state` — cached native telemetry with `schema`, `available`,
  `state` (`play`/`pause`/`stop`), `elapsed`, `duration`, opaque `media` key,
  `title`, `artist`, `album`, `source`, `audio_format`, nullable queue `index`,
  `volume`, `muted`, and `volume_writable`. Seconds are numeric. The cache becomes
  unavailable after six seconds without confirmed desktop telemetry. HTTP reads
  do not add an audio-engine polling loop.
- `GET /api/v1/queue?offset=0` — `{ "revision", "offset", "total", "tracks" }`.
  Up to 60 tracks per page; each has `index`, `title`, `artist`, `album`, and nullable
  duration in seconds. Restart paging if the revision changes.
- `GET /api/v1/artwork/<media>` — authenticated PNG/JPEG/WebP for the current
  opaque media key, at most 2 MiB. Never accepts file paths or external URLs.
- `POST /api/v1/control` — `{ "request_id": "<unique 8–64 character ID>",
  "command": { ... } }`. An accepted command returns `{ "accepted": true }`.

Commands are discriminated by `action`:

| Action | Additional fields |
| --- | --- |
| `play`, `pause`, `next`, `previous` | `media`: last observed media key |
| `seek` | `media`, `seconds`: finite nonnegative position |
| `volume` | `value`: integer 0–100; only when writable |
| `queue_play` | `index`, `revision`: observed queue revision |

Commands use existing native playback/configuration admission and source guards.
They cannot resolve arbitrary music URLs, change output devices, confirm Direct
gain policy, or edit preferences. Direct output can require confirmation on the
desktop; unavailable equipment volume is disabled in the controller. Queue play
checks the revision under the native audio lock before selecting the index.

A request ID belongs to a paired client. The last 32 mutation results are retained
in memory. Repeating the same ID/payload returns the original result; changing
its payload is refused. The web client does not automatically retry mutations.

Failures return `{ "error": "<safe code>" }`: `unauthorized`, `origin`, `invalid`,
`pairing_closed`, `pairing_failed`, `device_limit`, `busy`, `changed`,
`desktop_required`, `unavailable`, or `artwork_missing`. Never show raw response
or transport internals to users. Refresh after `changed`; pair again after
`unauthorized`. A successful mutation is an acknowledgement, not an optimistic
playback snapshot; observe the next confirmed state.

The listener uses bounded request sizes, deadlines, four request workers and
per-peer request budgets. Closing/disabling joins its workers. The remote has no
catalog scanner, analyser lease, microphone, or separate playback engine.
