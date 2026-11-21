# 04 · Server account and auth packets

How a server gets the tokens the authenticated login (spec 03) needs, and the
layouts of the three auth packets.

## Logging in (OAuth device flow, RFC 8628)

Endpoints under `https://oauth.accounts.hytale.com/oauth2/`. Client id
`hytale-server`, scopes `openid offline auth:server`. Bodies are
`application/x-www-form-urlencoded`.

1. `POST device/auth` with `client_id`, `scope` → JSON `device_code`,
   `user_code`, `verification_uri`, `verification_uri_complete`, `expires_in`,
   `interval`.
2. The account owner opens the verification URI and enters the code.
3. Poll `POST token` with
   `grant_type=urn:ietf:params:oauth:grant-type:device_code`, `client_id`,
   `device_code`. While waiting the answer is HTTP 400 with
   `error=authorization_pending` (or `slow_down`: poll less often). On success:
   `access_token`, `refresh_token`, `id_token`, `expires_in`.
4. Later: `POST token` with `grant_type=refresh_token`, `client_id`,
   `refresh_token`.

## Profile and game session

- `GET https://account-data.hytale.com/my-account/get-profiles`, bearer: the
  OAuth access token → `{"owner": uuid, "profiles": [{"uuid", "username"}]}`.
- `POST https://sessions.hytale.com/game-session/new`, bearer: the OAuth
  access token, body `{"uuid": "<profile uuid>"}` →
  `{"sessionToken", "identityToken", "expiresAt"}`.
- `POST /game-session/refresh` with the session token as bearer renews it; the
  official server does so 5 minutes before `expiresAt`.
- The server identity token must carry scope `hytale:server`.
- The server's audience is a random UUID picked once per server run.

## Tokens

- JWT, header `alg: EdDSA`, Ed25519 keys from
  `https://sessions.hytale.com/.well-known/jwks.json` (`keys[]` with `kty: OKP`,
  `crv: Ed25519`, `x`, `kid`). Issuer: `https://sessions.hytale.com`.
- Identity token claims: `sub` (player UUID), `scope` (space separated),
  `profile.username`, `profile.skin` (JSON text), `profile.entitlements`.
- Access token claims: `sub`, `username`, `aud`, and `cnf` with `x5t#S256`, the
  base64url (no padding) SHA-256 of the client certificate's DER bytes.

## Session service calls during a join

Both with `Authorization: Bearer <server session token>` and JSON bodies:

| Call | Body | Answer |
| --- | --- | --- |
| `POST /server-join/auth-grant` | `{"identityToken", "aud"}` (aud = server audience) | `{"authorizationGrant"}` |
| `POST /server-join/auth-token` | `{"authorizationGrant", "x509Fingerprint"}` (the server's own certificate) | `{"accessToken"}` |

## Packets

All three: 1 byte of nullable bits, then an offset table of two slots (bytes
1–8), variable block from byte 9.

| Id | Name | Dir | Field 1 (bit 0) | Field 2 (bit 1) |
| --- | --- | --- | --- | --- |
| 11 | AuthGrant | to client | authorization grant (UTF-8 ≤ 4096) | server identity token (UTF-8 ≤ 8192) |
| 12 | AuthToken | to server | access token (UTF-8 ≤ 8192) | server authorization grant (UTF-8 ≤ 4096) |
| 13 | ServerAuthToken | to client | server access token (UTF-8 ≤ 8192) | password challenge (bytes ≤ 64) |

After `ServerAuthToken` the login continues exactly as the insecure one after
`ConnectAccept`: password exchange if a challenge was sent, then setup.
