# 01 · Transport

Observed against server build 0.7.7 (protocol build 258).

## Connection

- QUIC over UDP. Default port 5520.
- TLS 1.3 inside QUIC, as QUIC requires.
- ALPN: the server offers `hytale/3` and `hytale/2`. A connection that lands on
  a version below 3 is let in only long enough to receive a "client outdated"
  disconnect, then closed with application error `ClientOutdated`.
- The server presents a self-signed certificate (CN `localhost`, generated at
  start-up). Clients do not validate it against a CA.
- **Mutual TLS is mandatory.** The client must present a certificate. The server
  accepts any certificate (no chain check) but closes the connection if none is
  sent. The certificate is kept and tied to the player for the rest of the
  session (it matters for the authenticated login, see 03).
- 0-RTT is off. Connection migration is off. Congestion control is BBR.

## Transport parameters the server sets

| Parameter | Value |
| --- | --- |
| initial_max_data | 512 KiB |
| initial_max_stream_data_bidi_local / remote | 128 KiB each |
| initial_max_streams_bidi | 8 |
| initial_max_streams_uni / stream_data_uni | 0 (no unidirectional streams) |
| ack_delay_exponent | 3 |
| max_idle_timeout | the server's "play" timeout |

## Streams

Everything travels on client-opened bidirectional streams.

- **The first stream the client opens is the game stream.** It carries every
  packet on the `Default` channel, and the whole login runs on it.
- When the player joins the world, **the server opens two unidirectional
  streams**, Chunks then WorldMap, with no header (spec 05). A client must
  allow the server at least 2 unidirectional streams.
- Later client-opened streams are auxiliary. They start with `StreamOpen`
  (460) naming the stream type; the server answers whether it accepts. They are bound to one of the other channels
  (`Chunks`, `WorldMap`, `Voice`) and must be identified within 5 seconds or
  they are dropped. (How a stream announces its channel: still to document.)
- A packet that arrives on a stream bound to a different channel than the
  packet's own is a protocol violation.

Channels:

| Value | Channel |
| --- | --- |
| 0 | Default |
| 1 | Chunks |
| 2 | WorldMap |
| 3 | Voice |

## Closing

Protocol violations close the whole QUIC connection with transport error
`PROTOCOL_VIOLATION`. Orderly disconnects send a `ServerDisconnect` packet first
and then close with an application error code (`QuicApplicationErrorCode`).
