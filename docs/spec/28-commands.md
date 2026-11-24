# 28 - Commands

How a chat line that starts with `/` is read, checked and answered. The
chat lines themselves (keys, parameter kinds and order) are in spec 29.

## How the official server reads a line

**Words.** The text after the `/` is split on spaces. Double or single
quotes keep spaces inside a word (the quotes stay in the word). `,`, `[`
and `]` are words of their own, and a backslash escapes `" ' , [ ] \`
(the character right after an escaped one is not looked at, so it cannot
end a word). Unbalanced quotes, a lonely backslash, an unknown escape, a
`[` inside a list and a `]` without `[` each have their own reply.

**Positional words and options.** Everything before the first `--name` is
positional; from there on, `--name=value`, or `--name value value`, is an
option and the words after it are its values. A positional word followed
by a `,`, or inside `[ ]`, is a list, and a command that does not take a
list answers "not a list" for it.

**The tree.** A command has a name (lower-cased for the lookup), aliases,
sub-commands, variants and required arguments. The first positional word
picks a sub-command by name or alias. A command with no matching
sub-command may have a variant: another form with a different number of
required words, picked by how many words there are. Groups (commands that
only hold sub-commands) print their usage when called with nothing and
answer with fuzzy suggestions when the first word is not a sub-command.

**Arguments.** Each required argument takes one word (a position takes
three). The type decides what is accepted and what the reply is: a whole
number, a decimal number, text, a block position (`~` marks relative,
numbers or `_`/`c` marks), a block, item or item quality by key (case does
not matter, the reply and the command use the real key), an online player
(a name, or the start of one), a game mode (`adventure`, `creative`, `a`,
`c`), the world (`default`), an entity stat. Options are matched by
exact name, else by the only name they begin, else by the only name that
contains them.

## Replies to a bad line

All are chat lines with keys (spec 29).

| situation | reply |
|---|---|
| no such command | `server.modules.command.notFound` with `cmd` = the whole line after `/` |
| word errors | `server.commands.parsing.error.unbalancedQuotes`, `invalidEscape` (`index`, `input`), `invalidEscapeForSymbol`, `cannotBeginListInsideList`, `cannotEndListWithoutStarting` (`index`), `endCommandWithOpenList` (`listEndToken`), `cantDoublePlaceSpecialTokens`, `tooManyListItems`, `allArgumentsInListNeedSameLength`, `notAList` (`name`) |
| a group with a word that is no sub-command | `…commandCollectionSubcommandNotFound` with `subcommand` and `suggestions`: the five best fuzzy matches of the names and aliases, each on its own line as `/group name`; ties keep the order of a Java hash set of those names |
| a group with nothing | `server.commands.help.usage`: ":", "  (", the "use help on any sub-command" key, ")", a new line and the usage of the whole group |
| wrong number of required words | `…wrongNumberRequiredParameters` with `expected`, `actual`, then a new line, `server.commands.help.usagecolon` with the short usage of the command that was reached, a new line, and `server.commands.help.useHelpToLearnMore` with the full command name |
| a word of the wrong type | `…argtype.integer.fail`, `double.fail`, `integerCoordinate.fail` (`input`), `…argtype.gamemode.invalid` (`input`, `suggestions`), `server.commands.errors.noSuchPlayer` (`username`), `…noSuchWorld` (`name`) |
| unknown block, item, item quality | `server.commands.notfound` (`type`, `id`, red) and then `server.general.failed.didYouMean` (`choices`, the five best fuzzy matches as a list) |
| unknown stat | `server.commands.entityStats.entityStatNotFound` (`name`) and the same "did you mean" |
| unknown option | `…couldNotFindOptionalArgName` (`input`) |
| option with the wrong number of values | `…wrongNumberOfParametersForArgument` (`argument`, `expected`, `actual`, `input`) |

The fuzzy score counts the letters of the typed text found in order in the
candidate (one point each, three for a letter right after the previous
match), best first. The short usage lists, for a command with variants or
sub-commands, each of them on its own line indented by depth, and for a
leaf its name followed by `<arg>` words (colour `#C1E0FF`) and `[--opt=?]`
words (colour `#7E9EBC`; a default shows its value, a flag has no `=`).

The order of the commands inside a hash set or map of the official server
decides ties, and the order of options in the usage text is the official
one, not the order they are declared in.

## `/who`

One line: `server.formatting.list.header` (`count`, `header` the world name
as text), the inline suffix key when there are at most four players, then
`server.commands.who.playerWithDisplayName` for each (`username`,
`displayName`), separated by the list separator key. More than four go one
per line (`list.row`). There is no `/list`: it answers "not found".

## `/help`

For a player it sends no chat line: it opens a page (a custom page packet)
with every command the player may use, sorted, the first one (or the one
named, by name or alias) selected. The page is built by interface commands:
a list of buttons, then for the selected command its name, sub-command
cards (three per row, with the usage and description key), variants,
aliases, permission, usage, parameters (required, optional, default, flag,
each with tag, type and description) and argument types with examples.
Buttons carry events (`Command`, `Subcommand`, `Variant`, `NavigateUp`,
`SendToChat`, `@SearchQuery`) the client sends back to change the page.

## In Aerie

`commands.rs` holds the tree for the commands Aerie knows: `block` (all its
sub-commands, as usage; `set` and `setticking` run), `give` (self form,
other form, `armor` as usage), `gamemode` (self form), `player` (the whole
tree as usage; `stats add/get/set/settomax/reset` run), `help`, `who`. It
splits the words, finds the command, counts and checks the words and
answers every failure above. A valid line is handed to `world.rs` with the
main spelling of the command, players by their real names and the options
Aerie understands. `/help` opens the page for the commands in the tree.

Commands outside the tree (`/pos`, `/perf`, `/setblock`, `/fill`, `/clear`,
`/tp`, `/entity`) are passed on untouched: the official server answers
`/pos`, `/perf` and `/setblock` as unknown, and has other commands under
`/fill` and `/clear`; Aerie keeps its own for tests and tools.

## Not done yet

- `--help` on any command (the long usage with argument types) answers a
  plain "not available" line.
- The page of `/help` lists only the commands in Aerie's tree (the official
  one lists all of its commands), uses its own page key, and the client
  events (clicks, search) are not answered.
- Forms the tree knows but Aerie cannot run: `gamemode <mode> <player>`,
  `player stats dump`, `block get`, … answer a plain "not available" line.
- "Did you mean" lists for blocks, items and stats: the order of equal
  scores follows the official asset maps, which Aerie does not reproduce,
  so the lists can differ after the first entries. Item qualities are not
  checked.
- Everything for `/tp` (spec of the teleport family, issue 33).
- Permissions: every player may run every command.

## Checked against the official server

Scripts of `cmd` lines (valid and invalid forms of each command, unknown
commands, missing, wrong and extra arguments, odd quoting, lists), run on
both servers and compared as decoded chat lines. Same keys, parameter names
and kinds, order, colours and children for every line of the tree except
the points under "Not done yet".
