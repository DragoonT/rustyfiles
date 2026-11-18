# Ferry: the protocol

How rustyfiles devices on the same network find each other and send files. **Ferry** is the feature's name; the code is
`crates/rf-nearby` (platform-free Rust, used by the Windows app and, through UniFFI, by the macOS app). Protocol version: **1**.

## Discovery

DNS-SD over mDNS. Every device that is visible registers:

- service type `_rustyfiles._udp.local.`, instance name = the device id, host `<device id>.local.`
- port = the UDP port of its QUIC endpoint
- TXT records:

| key | value |
|---|---|
| `id` | device id (see Identity) |
| `name` | the name to show ("Özer's PC") |
| `os` | `windows`, `macos`, `linux` |
| `proto` | protocol version, `1` |
| `vis` | `trusted` (lists only for its trusted devices) or `everyone` |
| `link` | optional: its network link and rate in Mbit/s, `wifi:866`, `ethernet:1000`, `other:100` (for a first speed estimate) |

A device with visibility "off" does not register and refuses incoming connections. A browser lists a device whose `vis` is
`trusted` only if that device is in its own trusted list. Devices with another `proto` are ignored.

## Identity

Each device makes one key pair and a self-signed X.509 certificate on first start and keeps them. Its **fingerprint** is the
SHA-256 of the certificate (DER), lower-case hex; its **device id** is the first 20 characters of the fingerprint. An id is never
taken on trust: it is always recomputed from the certificate the peer proves in the TLS handshake.

## Transport

QUIC (RFC 9000) with TLS 1.3, ALPN `rustyfiles-nearby/1`. Mutual TLS: both sides present their certificate and the handshake
signatures are verified normally; the certificates themselves are not chained to any root. What a certificate is worth is decided by
its fingerprint (trusted list) or by the user (an offer, a pairing code).

One connection per transfer or pairing. The connecting side opens one bidirectional **control stream** and speaks first. Messages
on the control stream are **frames**: a 4-byte big-endian length, then that many bytes of UTF-8 JSON (at most 64 MiB).

### Hello (client → server, first frame)

```json
{ "proto": 1, "id": "<device id>", "name": "<name>", "os": "macos",
  "purpose": { "kind": "send", "items": [
    { "path": "project/sub/big.bin", "size": 20971520, "dir": false, "mode": 493, "mtime": 1600000000123 },
    { "path": "App.app/Contents/Frameworks/Lib", "size": 0, "dir": false, "link": "Versions/Current/Lib" } ] } }
```

`purpose.kind` is `send` (with `items`) or `pair`. The server closes the connection when `proto` differs or when `id` does not
match the client certificate.

`items` lists every regular file, every **empty** folder (`dir: true`, `size: 0`) and every symbolic link (`link`: its target as
it is, no bytes sent). Paths are relative to the offer, parts separated by `/`, in Unicode **NFC**. Optional fields: `mode` (Unix
permission bits; Windows sends none) and `mtime` (last change, milliseconds since 1970 UTC). Links are sent as links, never
followed.

A receiver refuses an offer containing a path that is absolute, empty, has an empty part, `.` or `..`, or a control character.
Any other name is stored as the receiving platform allows: on Windows `<>:"|?*\` become `_`, trailing dots and spaces are dropped,
and reserved device names (CON, NUL, COM1...) get a leading `_`.

### Sending

1. Server → client `Answer`: `{ "accept": true, "reason": null, "resume": [0, 1048576, ...] }`. `resume[i]` is how many bytes of
   item `i` the receiver already holds from an earlier, broken transfer (0 for folders). A refusal is `accept: false` with a
   `reason`; the client then reports the transfer as failed.
2. For each file item, the client opens a **unidirectional stream**: first a frame `{ "index": i, "offset": resume[i] }`, then the
   file's bytes from `offset` to the end, then it finishes the stream. Several files go at once (the reference sends four).
3. Client → server on the control stream: `Done` `{ "hashes": ["<blake3 hex of the whole file>", ...] }`, one per item, empty for
   folders.
4. The receiver writes everything at a **staging** path: a top-level file as `<name>.ferrying`, everything inside a top-level
   folder under `<folder>.ferrying/` ("still on its way"). It checks the BLAKE3 of every file; when all match, each top-level item is renamed to its real name.
   Then `Outcome` `{ "ok": true, "error": null }`. A file whose hash does not match is deleted and `ok` is false; the other parts
   stay where they are, so sending the same items again resumes them (`resume` offsets) and sends the damaged file anew.

Final paths: under the destination folder the receiver chose. A top-level name that is already taken there gets ` (2)`, ` (3)`...
before its extension. Names are compared without regard to letter case (NTFS and APFS are case-insensitive), so two items that
differ only in case do not land on one file: the later one is numbered the same way.

After the files: each kept file gets its `mtime` and, on Unix, its `mode`. Links are made last (so no file is ever written
through one), and only when the target, resolved from the link's own folder, stays inside the top-level item it came with; others
are left out. Windows makes no links.

### Speed test

After a hello with `"purpose": { "kind": "speed" }` nothing touches the disk and nothing is asked. The server answers `Answer`
(`accept`, `reason`; `resume` empty): it refuses a device its visibility does not admit (trusted devices only), and a device that
is already testing it, that tested it less than 10 seconds ago, or when two tests already run ("the device is busy with another
speed test").

1. **Up.** The client opens a unidirectional stream and writes bytes that do not compress. The server leaves the first 750 ms
   out (congestion control and Wi-Fi's rate still ramping up), counts what arrives for 2 seconds after that, then stops the stream
   (STOP_SENDING), and writes `SpeedReport` `{ "bytes": n, "micros": t }` on the control stream. A sender stops by itself after
   3.25 s or 512 MiB.
2. **Down.** The server opens a unidirectional stream and writes the same way; the client counts it the same way.
3. The client closes the connection. Up is `bytes / micros` of the report, down the client's own count; the round trip is
   QUIC's estimate on that connection right after the answer, before the load (after it, the filled queues make it look longer).

A transfer of at least 4 MiB also gives a speed (bytes moved / time). The reference keeps the last of either per device (on disk
for trusted devices) and, with nothing of the kind, estimates from the two `link` values: the slower of own and peer rate times
0.5 for Wi-Fi, 0.9 for Ethernet, 0.6 otherwise, divided by 8, and shown 15% lower than that (real transfers came out lower).

### Pairing

After a `pair` hello, both sides compute the same six-digit code from the TLS session: 8 bytes from the RFC 5705 exporter with label
`rustyfiles nearby pairing` and an empty context, read as a big-endian u64, modulo 1 000 000, zero-padded to six digits. Each side
shows it and asks its user. Each side then sends `PairConfirm` `{ "ok": true|false }` on the control stream (the client on its half,
the server on its half) and reads the other's. Only when both are `true` does each side add the other's id, name and fingerprint to
its trusted list.

## Who is asked what

| visibility of the receiver | sender trusted | offer |
|---|---|---|
| off | either | connection refused |
| trusted devices only | yes | taken into the inbox without asking (if the app allows), else asked |
| trusted devices only | no | refused (`accept: false`) |
| everyone | yes | as above |
| everyone | no | asked; no answer in 3 minutes counts as no |

A receiver asks about one offer per device at a time and about four at most in all; an offer beyond that is refused with
`accept: false` and the reason "the device is busy with other offers". Clients match the reasons by these words, so they do not
change: "only takes files from its trusted devices", "declined", "busy with other offers", "busy with another speed test",
"arrived damaged".
