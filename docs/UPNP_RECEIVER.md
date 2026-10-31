# UPnP receiver (0.5.7)

Sonante can act as an opt-in UPnP AV MediaRenderer on an IPv4 local network.
Enable **UPnP receiver** in Preferences, choose its device name, and Apply.
Select that name as the renderer in BubbleUPnP or another compatible controller.
The receiver is disabled by default. Discovery and playback have been exercised
with BubbleUPnP; compatibility with other controllers and outputs can vary.

## Behavior

- A received track replaces the current queue, using the existing MPD engine and
  selected output. Setting the URI prepares a stopped queue; only Play starts audio.
- Play, Pause, Stop and absolute/relative time Seek are supported. The controller
  manages subsequent tracks; SetNextAVTransportURI and OpenHome queues are not implemented.
- Title, artist, album and artwork supplied by the controller are displayed.
- Volume and mute obey the existing output controls. Equipment-controlled Direct
  volume is not replaced by a software gain stage. Existing Direct playback
  confirmation must be completed in Sonante if that output has not been approved yet.
- Selecting Local/Plex media invalidates transport commands for the old remote
  session. UPnP tracks cannot be saved as permanent playlist references.
- Remote URLs and remote queue metadata are not persisted for restart. No remote
  track automatically resumes on startup. Durable Local/Plex occurrences, if present,
  retain their order and a cursor only for a durable current item. Receiver enablement, name and UUID have
  separate private preferences; existing audio/library settings are unchanged.

## Network and lifecycle

Discovery uses SSDP, with AVTransport, RenderingControl and ConnectionManager
HTTP/SOAP services and GENA notifications. Only local/private IPv4 controller
addresses are accepted. Receiver connections, discovery responses, subscriptions,
XML and metadata have resource bounds. Notification callbacks must use the
requesting peer's IP. File URLs, local-host media endpoints, XML DTDs and malformed
SOAP are refused. The service does not change firewall or router settings.

The enabled receiver shares existing playback snapshots for notifications rather
than adding an MPD polling loop. Position/transport queries do not probe system
volume or enumerate audio hardware; volume queries use the existing output controls. Disabling it or closing Sonante releases its
worker and sockets. An interface/address change may require toggling the receiver.

This receiver is not a media server and does not implement TIDAL/Qobuz Connect.
It makes no claim of DLNA certification or verified gapless remote playback.

## Validation

Automated tests cover bounded HTTP/SOAP using owned loopback sockets, XML/URL
validation, metadata escaping, subscription ownership, native queue preparation,
secret-free persistence, and stale-session admission/effect checks. They do not
replace representative testing with the controller, network and output in use.
