# 26 - Game package download

How the official server gets a new version of itself with its own account
login, and how Aerie uses the same calls to fetch the game files, so a
machine with no other tool (a phone) can get them. In prose: the reference
was read, nothing is copied from it.

## What the official server does

The official server has an update module. It works with the OAuth access
token of the server's account login, the same device-code login spec 04
describes, refreshed first when it is about to expire.

- **Release channels.** `GET /my-account/get-patchlines` on the account
  data service, with the token as a bearer, answers
  `{"patchlines": [{"name", "expiresAt"}, …]}`: the channels the account may
  use, and for each the time (milliseconds since the epoch) after which it
  stops having it, or 0. The server warns when its channel has expired or
  is about to. Its channel is the one set in its settings, or else the one
  its own build was made for, or else `release`.
- **Signed links.** Files are not fetched from the account service itself:
  `GET /game-assets/<path>`, with the bearer token, answers `{"url": …}`, a
  short-lived signed link to the file in storage. The link is then fetched
  without the token.
- **Version manifest.** The path `version/<channel>.json` gives
  `{"version", "download_url", "sha256"}`: the channel's current version,
  the storage path of its package (signed in turn, like any path), and the
  package's SHA-256 in hex.
- **Package.** The package is a zip. The server streams it to a temporary
  file, hashing as it writes, and drops it when the hash differs from the
  manifest's. It allows the whole download thirty minutes and does not
  resume one that fails.
- **Contents.** At the root: the assets archive, a `Server` folder (the
  server jar, its ahead-of-time cache file, licences) and start scripts for
  each system. Installing unpacks it over the server's folder and marks the
  start script executable.

## In Aerie

- `aerie-auth` (`game.rs`) makes the same calls: `patchlines`,
  `signed_url`, `manifest`, and `download`.
- `download` asks for a new signed link at each attempt (links expire),
  resumes from what an earlier attempt left in `<package>.part` with a
  `Range` request (a store that answers the whole file instead makes it
  start over), retries a dropped transfer up to eight times with a growing
  pause, reports progress as it goes, and checks the SHA-256 at the end; a
  file with the wrong hash is deleted, not resumed from. Its HTTP client has
  no limit on the whole transfer, only on connecting (30 s) and on a stalled
  read (60 s): a phone on a slow link can take far longer than the official
  thirty minutes.
- `aerie-server fetch-game [--out DIR] [--patchline NAME] [--list]
  [--keep-package] [--account FILE]` logs in first when there is no saved
  login, refreshes the token when needed, picks the given channel (or
  `release`, or the first one the account has), skips everything when `DIR`
  already holds that version (`game-version.txt`), downloads, unpacks
  (refusing paths that leave `DIR`), and deletes the package unless asked to
  keep it. `--list` shows each channel and its current version.

## Not done yet

- The package is unpacked next to itself, so for a moment the disk holds
  both (about 1.7 GB compressed, 3.9 GB unpacked for 0.7.7). On a phone the
  assets archive could be read in place inside the package instead.
- Expiry of the account's channels is not shown.
- The phone app's own screens (progress, "only on Wi-Fi") are the app's.

## Checked against the official server

With one account (2026-11-24): the channel list was `release` 0.7.7,
`pre-release` 0.8.0-pre.6, and three older channels. The `release` package
(1.7 GB) downloaded, matched its manifest's SHA-256, and unpacked into an
assets archive, server jar, its cache file and start script byte for byte
the same as those the official downloader had fetched for 0.7.7. A
pre-release download stopped at 14 % resumed from there and passed the hash
check.
