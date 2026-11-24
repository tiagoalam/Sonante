# Subsonic provider

The source is presented as **Subsonic**. Its current implementation reuses the
existing Navidrome provider and saved connection file so an existing
`navidrome.json` connection and `sources.navidrome` preference remain valid.
The internal command and media-locator names are intentionally retained for
backward compatibility.

## Protocol and authentication

The provider uses the common Subsonic REST endpoints already needed by the
client: `ping`, `getAlbumList2`, `getAlbum`, `getArtist`, `search3`, `getSong`,
`stream`, and `getCoverArt`. Requests preserve a configured reverse-proxy path.
Traditional username/password authentication continues to use a salted token.

OpenSubsonic API-key authentication is offered only when the public
`getOpenSubsonicExtensions` response advertises
`apiKeyAuthentication` version 1. API-key requests contain `apiKey` and omit
`u`, `t`, and `s`. Legacy saved connections default to password authentication.
The selected REST API version is lowered only after Subsonic error code 30;
authentication, permission, malformed-response, and network errors do not
become empty catalog results or trigger protocol fallback.

HTTPS is recommended. HTTP remains available for compatibility and the
connection form warns that credentials and streams are unencrypted. The local
MPD proxy remains loopback-only; remote IDs are bounded and encoded before
being placed in its path, and external credentials are never placed in MPD's
URL.

Favorites remain local to Sonante. Password-authenticated favorites preserve
their existing server/account scope. API-key favorites are scoped by server,
the optional local account label, and a one-way API-key fingerprint, so two
keys cannot share favorites accidentally. Replacing an API key therefore
creates a separate local favorite scope.

## Compatibility boundary

Automated fixtures cover the existing Navidrome-style response, a legacy
Subsonic version negotiation, OpenSubsonic capability discovery, API-key
authentication, malformed data, permissions, opaque IDs, and local proxy
handling. Manual scenarios were also checked with Navidrome, Gonic 0.22.0 and
Ampache 8.4. This does not certify every endpoint or feature in those products,
and Airsonic, Airsonic-Advanced and other compatible servers have not been
homologated against real servers.

Server-specific APIs are not inferred from a product name. Optional OpenSubsonic
extensions are used only when advertised. Favorites are not written to the
remote server. Streaming/transcoding format selection remains the server's
responsibility; the UI does not claim that a server-transcoded stream is the
original lossless bitstream.
