# 29 - Chat messages

How the official server writes the chat lines it sends, and which of them
Aerie sends the same way. In prose: the reference was read, nothing is
copied from it.

## What the official server does

A chat line is a formatted message: almost always a translation key the
client turns into text in its own language, with named parameters, rather
than finished text. Raw text is used only for what players type and for
names shown as written.

- **Parameters** come in two maps: plain values (text, 32-bit and 64-bit
  integers, floating point, true/false, each sent with its kind) and values
  that are messages themselves, translated on their own (an item's name,
  a game mode's name). The kind follows the value's type in the server's
  code: coordinates are integers, a block's id is text.
- Both maps are Java hash maps, so the parameters go out in that map's
  iteration order (by the bucket of the name's hash in a table of 16 that
  doubles when more than three quarters full; insertion order inside a
  bucket), not in the order the code added them. Setting a name twice
  keeps the last value in the first one's place.
- **Lines Aerie has an equivalent for:**

| when | key | parameters |
|---|---|---|
| a player joins the world (to everyone in it, the joiner included) | `server.general.playerJoinedWorld` | `username` (text); `world` (message: the world's display name, raw text `default` for the default world) |
| a player leaves (to those still there) | `server.general.playerLeftWorld` | same |
| a player's chat line (to everyone) | `server.chat.playerMessage` | `username`, `message` (text) |
| `/block set` | `server.commands.block.set.success` | `x`, `y`, `z` (integers), `id` (text: the block's own key, whatever case was typed) |
| `/block setticking` | `server.commands.block.setticking.success` | `x`, `y`, `z` (integers) |
| `/gamemode` to another mode | `server.commands.gamemode.success.self` | `mode` (message: `server.general.gamemodes.<mode>`) |
| `/gamemode` to the current mode | `server.commands.gamemode.alreadyInMode.self` | none; nothing else is sent |
| `/give` | `server.commands.give.gave` / `…insufficientInvSpace` | as spec 14 |
| `/player stats` | spec 17 | |

## In Aerie

`messages.rs` builds every keyed line (`Msg::key(…)` with typed parameters,
message parameters, colour, children) and orders the parameters as the
official hash maps do. The lines in the table use it; the world's display
name is `default`.

## Not done yet

- Aerie's own commands that the official server does not have (`/setblock`,
  `/fill`, `/blocks`, `/pos`, `/perf`, `/clear`, the short `/give ITEM`)
  still answer in plain English; they are Aerie's, with no official line to
  match.
- `/tp` replies belong to the teleport commands (#33); `/help`, `/list` and
  command errors to spec 28 (#34).
- Where the join line falls among the other join packets was not compared.

## Checked against the official server

Two probes on each server (2026-11-24): one stays, the other joins, says a
line, runs `/block set` twice (a block, then `empty`), `/block setticking`,
`/gamemode creative` twice and `/gamemode adventure`, and leaves. Both
probes' chat, as decoded, is the same on both servers line for line (keys,
parameter names, values, order), coordinates aside since each server is
tested in its own place, and the parameter kinds of every line match.
