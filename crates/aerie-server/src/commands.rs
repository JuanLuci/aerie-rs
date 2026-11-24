//! Command lines (spec 28): how a chat line starting with `/` is split into
//! words, matched against the command tree, checked, and what each failure
//! answers. The world only runs a command once its line passed here.
//!
//! The tree holds the commands the official server has and Aerie knows: for
//! each, its sub-commands, variants (the same command with more required
//! words), arguments and options. A node that Aerie can run is marked
//! `done`; the others are still part of the tree because the usage hints the
//! official server prints list the whole tree.

use std::sync::OnceLock;

use aerie_assets::chm::java_hash;
use aerie_protocol::proto;

use crate::messages::Msg;

/// What the world tells the parser about itself.
pub trait Env {
    /// The names of the players online.
    fn players(&self) -> Vec<String>;
    /// A block type by key (any case); `None` when there is no such block.
    /// A world without a block registry (tests) answers with the input.
    fn block_key(&self, name: &str) -> Option<String>;
    /// An item by key.
    fn item_key(&self, name: &str) -> Option<String>;
    /// An entity stat by key (any case).
    fn stat_key(&self, name: &str) -> Option<String>;
    /// Every block key, for the "did you mean" hints.
    fn block_keys(&self) -> Vec<String>;
    fn item_keys(&self) -> Vec<String>;
    fn stat_keys(&self) -> Vec<String>;
}

/// What became of a command line.
pub enum Outcome {
    /// Answered here (errors, usage, `/help`, `/who`): send these lines.
    Replies(Vec<Msg>),
    /// A page for the player (`/help`).
    Page(proto::CustomPage),
    /// Valid: the world runs it from these words (the command name first,
    /// in its main spelling, players by their real names).
    Run(String),
    /// Valid, but a form Aerie cannot run yet.
    Unavailable,
    /// Not one of the commands in the tree: Aerie's own (`/pos`, `/tp` …).
    Other,
}

/// Aerie's own commands, outside the tree (see spec 28, "In Aerie").
const OTHER: &[&str] = &["pos", "perf", "setblock", "fill", "clear", "entity"];

// ---------------------------------------------------------------------
// Words

/// One word of a command line, or the reason the line cannot be split.
fn tokenize(input: &str) -> Result<Vec<String>, Msg> {
    let mut out = Vec::new();
    let (first, rest) = match input.split_once(' ') {
        Some((a, b)) => (a, Some(b)),
        None => (input, None),
    };
    out.push(first.to_owned());
    let Some(rest) = rest else { return Ok(out) };
    let mut s: Vec<char> = rest.chars().collect();
    let mut quote = '\0';
    let mut start = 0usize;
    let mut in_list = false;
    let mut i = 0usize;
    let text =
        |s: &[char], a: usize, b: usize| -> String { s[a.min(b)..b.min(s.len())].iter().collect() };
    while i < s.len() {
        let c = s[i];
        let mut extract = false;
        match c {
            ' ' => {
                if quote == '\0' {
                    if start < i {
                        out.push(text(&s, start, i));
                    }
                    start = i + 1;
                }
            }
            '"' => {
                if quote == '\0' {
                    quote = '"';
                } else if quote == '"' {
                    quote = '\0';
                    out.push(text(&s, start, i + 1));
                    start = i + 1;
                }
            }
            '\'' => {
                if quote == '\0' && start == i {
                    quote = '\'';
                } else if quote == '\'' {
                    quote = '\0';
                    out.push(text(&s, start, i + 1));
                    start = i + 1;
                }
            }
            ',' => {
                if quote == '\0' {
                    let t = text(&s, start, i);
                    if !t.is_empty() {
                        out.push(t);
                    }
                    start = i;
                    extract = true;
                }
            }
            '[' => {
                if quote == '\0' {
                    if in_list {
                        return Err(Msg::key(
                            "server.commands.parsing.error.cannotBeginListInsideList",
                        )
                        .int("index", i as i32));
                    }
                    in_list = true;
                    start = i;
                    extract = true;
                }
            }
            '\\' => {
                if s.len() <= i + 1 {
                    return Err(Msg::key("server.commands.parsing.error.invalidEscape")
                        .int("index", i as i32 + 1)
                        .text("input", input));
                }
                let next = s[i + 1];
                if matches!(next, '"' | '\'' | ',' | '[' | '\\' | ']') {
                    // The backslash goes; the escaped character is kept and
                    // the one after it is not looked at.
                    s.remove(i);
                    i += 1;
                } else {
                    return Err(
                        Msg::key("server.commands.parsing.error.invalidEscapeForSymbol")
                            .text("symbol", &next.to_string())
                            .int("index", i as i32 + 1)
                            .text("input", input)
                            .text("command", input),
                    );
                }
            }
            ']' => {
                if quote == '\0' {
                    if !in_list {
                        return Err(Msg::key(
                            "server.commands.parsing.error.cannotEndListWithoutStarting",
                        )
                        .int("index", i as i32));
                    }
                    let t = text(&s, start, i);
                    if !t.is_empty() {
                        out.push(t);
                    }
                    start = i;
                    in_list = false;
                    extract = true;
                }
            }
            _ => {}
        }
        if extract {
            out.push(text(&s, start, i + 1));
            start = i + 1;
        }
        if start > s.len() {
            start = s.len();
            break;
        }
        i += 1;
    }
    if quote != '\0' {
        return Err(Msg::key("server.commands.parsing.error.unbalancedQuotes"));
    }
    if start != s.len() {
        out.push(text(&s, start, s.len()));
    }
    Ok(out)
}

/// A word slot before the options: one word, or a list of them.
#[derive(Debug)]
enum Slot {
    One(String),
    List(ListSlot),
}

#[derive(Debug, Default)]
struct ListSlot {
    words: Vec<String>,
    first_sep: bool,
    per_arg: usize,
    since_sep: usize,
    items: usize,
}

impl ListSlot {
    fn show(&self) -> String {
        let mut s = String::from("[");
        for (i, w) in self.words.iter().enumerate() {
            if i != 0 && self.per_arg != 0 && i % self.per_arg == 0 && i != self.words.len() - 1 {
                s.push_str(" ,");
            }
            s.push(' ');
            s.push_str(w);
        }
        s.push_str("<-- *HERE* ... ]");
        s
    }

    fn add(&mut self, w: &str) -> Result<(), Msg> {
        let same_length = |l: &ListSlot| {
            Msg::key("server.commands.parsing.error.allArgumentsInListNeedSameLength")
                .text("error", &l.show())
        };
        if w == "," {
            if !self.first_sep {
                self.first_sep = true;
                self.since_sep = 0;
                self.items += 1;
            } else if self.since_sep != self.per_arg {
                self.words.push(w.to_owned());
                return Err(same_length(self));
            } else {
                self.since_sep = 0;
                self.items += 1;
            }
            if self.items > 10 {
                return Err(
                    Msg::key("server.commands.parsing.error.tooManyListItems").int("amount", 10)
                );
            }
        } else {
            self.since_sep += 1;
            if self.items == 0 {
                self.per_arg += 1;
            }
            if self.first_sep && self.since_sep > self.per_arg {
                self.words.push(w.to_owned());
                return Err(same_length(self));
            }
            self.words.push(w.to_owned());
        }
        Ok(())
    }
}

/// The words of a line, sorted into positional slots and `--options`.
#[derive(Debug, Default)]
struct Line {
    slots: Vec<Slot>,
    /// Options in the order first seen: lower-cased name, value groups.
    opts: Vec<(String, Vec<Vec<String>>)>,
    /// Positional words before any list.
    singles_before_list: usize,
}

fn opt_name(t: &str) -> Option<&str> {
    let r = t.strip_prefix("--")?;
    let n = r
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(r.len());
    Some(&r[..n])
}

/// `--name=value`: the value, quotes taken off.
fn inline_value(t: &str) -> Option<String> {
    let r = t.strip_prefix("--")?;
    let eq = r.find('=')?;
    let name = &r[..eq];
    let v = &r[eq + 1..];
    if name.is_empty()
        || v.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return Some(v[1..v.len() - 1].to_owned());
    }
    Some(v.to_owned())
}

impl Line {
    /// Starts (or restarts) the option `name`; its position in the list.
    fn new_opt(&mut self, name: &str) -> usize {
        let name = name.to_lowercase();
        match self.opts.iter().position(|(n, _)| *n == name) {
            Some(i) => {
                self.opts[i].1 = Vec::new();
                i
            }
            None => {
                self.opts.push((name, Vec::new()));
                self.opts.len() - 1
            }
        }
    }
}

/// Sorts the words (the first is the command name).
fn contextualize(tokens: &[String]) -> Result<Line, Msg> {
    let mut line = Line::default();
    let mut began_opts = false;
    let mut in_list = false;
    let mut single_list = false;
    let mut last_special = false;
    let mut entered_before = false;
    let mut last_opt: Option<usize> = None;
    let special = |t: &str| matches!(t, "[" | "]" | ",");
    for (i, t) in tokens.iter().enumerate() {
        if in_list {
            entered_before = true;
        }
        if special(t) {
            let ends_and_starts = i > 0 && tokens[i - 1] == "]" && !in_list && t == "[";
            if last_special && !ends_and_starts {
                let mut shown = String::new();
                for (j, w) in tokens.iter().enumerate() {
                    shown.push_str(w);
                    shown.push(' ');
                    if j == i {
                        shown.push_str(" <--- *HERE!* ");
                    }
                }
                // Two lines: the reason, then the line with a marker.
                return Err(
                    Msg::key("server.commands.parsing.error.cantDoublePlaceSpecialTokens")
                        .child(Msg::raw("\n"))
                        .child(Msg::raw(&shown)),
                );
            }
            last_special = true;
        } else {
            last_special = false;
        }
        if let Some(name) = opt_name(t) {
            began_opts = true;
            last_opt = Some(line.new_opt(name));
            if let Some(v) = inline_value(t) {
                if let Some(k) = last_opt {
                    let groups = &mut line.opts[k].1;
                    if groups.is_empty() {
                        groups.push(vec![v]);
                    } else {
                        groups.last_mut().unwrap().push(v);
                    }
                }
            }
        } else if began_opts {
            if let Some(k) = last_opt {
                let (_, groups) = &mut line.opts[k];
                match t.as_str() {
                    "[" | "]" => {}
                    "," => groups.push(Vec::new()),
                    _ if groups.is_empty() => groups.push(vec![t.clone()]),
                    _ => groups.last_mut().unwrap().push(t.clone()),
                }
            }
        } else if t == "[" {
            in_list = true;
            single_list = false;
            line.slots.push(Slot::List(ListSlot::default()));
        } else if t == "]" {
            in_list = false;
        } else if in_list {
            if let Some(Slot::List(l)) = line.slots.last_mut() {
                l.add(t)?;
            }
            if single_list && !last_special && tokens.get(i + 1).is_some_and(|n| n != ",") {
                in_list = false;
            }
        } else if tokens.get(i + 1).is_some_and(|n| n == ",") {
            in_list = true;
            single_list = true;
            let mut l = ListSlot::default();
            l.add(t)?;
            line.slots.push(Slot::List(l));
        } else {
            if !entered_before {
                line.singles_before_list += 1;
            }
            line.slots.push(Slot::One(t.clone()));
        }
    }
    if in_list && !single_list {
        return Err(
            Msg::key("server.commands.parsing.error.endCommandWithOpenList")
                .text("listEndToken", "]"),
        );
    }
    Ok(line)
}

impl Line {
    /// How many required words the slots from `from` on make.
    fn words(&self, from: usize) -> usize {
        self.slots[from.min(self.slots.len())..]
            .iter()
            .map(|s| match s {
                Slot::One(_) => 1,
                Slot::List(l) => l.per_arg,
            })
            .sum()
    }

    fn first(&self, at: usize) -> Option<&str> {
        match self.slots.get(at)? {
            Slot::One(w) => Some(w),
            Slot::List(l) => l.words.first().map(String::as_str),
        }
    }
}

// ---------------------------------------------------------------------
// The tree

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ty {
    Int,
    Double,
    Str,
    /// Three words: x y z, each a number or `~` relative.
    Pos,
    /// One such word.
    Coord,
    Block,
    Item,
    Quality,
    Player,
    Mode,
    /// A text that must be an entity stat.
    Stat,
    World,
    /// A decimal coordinate: `5.0`, `~-2.3`, `_` for the ground height.
    Dbl,
    /// A rotation in degrees, `~` relative to the current one.
    Rel,
    /// Three decimal coordinates.
    Pos3,
    /// Three numbers: pitch, yaw, roll.
    Rot3,
}

impl Ty {
    fn count(self) -> usize {
        if matches!(self, Ty::Pos | Ty::Pos3 | Ty::Rot3) {
            3
        } else {
            1
        }
    }

    /// (name key, usage key, examples)
    fn info(self) -> (&'static str, &'static str, &'static [&'static str]) {
        match self {
            Ty::Int => (
                "server.commands.parsing.argtype.integer.name",
                "server.commands.parsing.argtype.integer.usage",
                &["-27432", "-1", "0", "1", "56346"],
            ),
            Ty::Double => (
                "server.commands.parsing.argtype.double.name",
                "server.commands.parsing.argtype.double.usage",
                &["-3.14", "0.0", "3.141596"],
            ),
            Ty::Str | Ty::Stat => (
                "server.commands.parsing.argtype.string.name",
                "server.commands.parsing.argtype.string.usage",
                &[
                    "\"Hytale is really cool!\"",
                    "\"Numbers work 2!\"",
                    "\"If you can type it...\"",
                ],
            ),
            Ty::Pos => (
                "server.commands.parsing.argtype.relativeBlockPosition.name",
                "server.commands.parsing.argtype.relativeBlockPosition.usage",
                &["124 232 234", "~5 ~ ~-3", "~ ~ ~"],
            ),
            Ty::Coord => (
                "server.commands.parsing.argtype.integerCoordinate.name",
                "server.commands.parsing.argtype.integerCoordinate.usage",
                &["5", "~-2", "0"],
            ),
            Ty::Block => (
                "server.commands.parsing.argtype.asset.blocktype.name",
                "server.commands.parsing.argtype.asset.blocktype.usage",
                &[],
            ),
            Ty::Item => (
                "server.commands.parsing.argtype.asset.item.name",
                "server.commands.parsing.argtype.asset.item.usage",
                &[],
            ),
            Ty::Quality => (
                "server.commands.parsing.argtype.asset.itemquality.name",
                "server.commands.parsing.argtype.asset.itemquality.usage",
                &[],
            ),
            Ty::Player => (
                "server.commands.parsing.argtype.player.name",
                "server.commands.parsing.argtype.player.usage",
                &["john_doe", "user123"],
            ),
            Ty::Mode => (
                "server.commands.parsing.argtype.gamemode.name",
                "server.commands.parsing.argtype.gamemode.usage",
                &["adventure", "creative", "a", "c"],
            ),
            Ty::World => (
                "server.commands.parsing.argtype.world.name",
                "server.commands.parsing.argtype.world.usage",
                &["default"],
            ),
            Ty::Dbl => (
                "server.commands.parsing.argtype.doubleCoordinate.name",
                "server.commands.parsing.argtype.doubleCoordinate.usage",
                &["5.0", "~-2.3", "0.0"],
            ),
            Ty::Rel => (
                "server.commands.parsing.argtype.relativeFloat.name",
                "server.commands.parsing.argtype.relativeFloat.usage",
                &["90.0", "~-45.5", "~"],
            ),
            Ty::Pos3 => (
                "server.commands.parsing.argtype.relativePosition.name",
                "server.commands.parsing.argtype.relativePosition.usage",
                &["124.63 232.27 234.22", "~5.5 ~ ~", "~ ~ ~"],
            ),
            Ty::Rot3 => (
                "server.commands.parsing.argtype.rotation.name",
                "server.commands.parsing.argtype.rotation.usage",
                &["124.63 232.27 234.22"],
            ),
        }
    }
}

/// A required argument.
struct Arg {
    name: &'static str,
    ty: Ty,
    desc: &'static str,
}

/// An option (`--name`): a flag when `ty` is `None`; with `def` it has a
/// default value.
struct Opt {
    name: &'static str,
    ty: Option<Ty>,
    def: Option<&'static str>,
    desc: &'static str,
}

struct Node {
    name: &'static str,
    aliases: &'static [&'static str],
    desc: &'static str,
    perm: &'static str,
    /// Prints its usage when called with nothing (a group of sub-commands).
    collection: bool,
    req: Vec<Arg>,
    opts: Vec<Opt>,
    /// The same command with another number of required words (no name).
    variants: Vec<Node>,
    subs: Vec<Node>,
    /// Aerie runs this form.
    done: bool,
}

impl Node {
    fn new(name: &'static str, desc: &'static str) -> Self {
        Node {
            name,
            aliases: &[],
            desc,
            perm: "",
            collection: false,
            req: vec![],
            opts: vec![],
            variants: vec![],
            subs: vec![],
            done: false,
        }
    }
    fn aliases(mut self, a: &'static [&'static str]) -> Self {
        self.aliases = a;
        self
    }
    fn perm(mut self, p: &'static str) -> Self {
        self.perm = p;
        self
    }
    fn group(mut self) -> Self {
        self.collection = true;
        self
    }
    fn req(mut self, name: &'static str, ty: Ty, desc: &'static str) -> Self {
        self.req.push(Arg { name, ty, desc });
        self
    }
    fn opt(mut self, name: &'static str, ty: Ty, desc: &'static str) -> Self {
        self.opts.push(Opt {
            name,
            ty: Some(ty),
            def: None,
            desc,
        });
        self
    }
    fn dflt(mut self, name: &'static str, ty: Ty, def: &'static str, desc: &'static str) -> Self {
        self.opts.push(Opt {
            name,
            ty: Some(ty),
            def: Some(def),
            desc,
        });
        self
    }
    fn flag(mut self, name: &'static str, desc: &'static str) -> Self {
        self.opts.push(Opt {
            name,
            ty: None,
            def: None,
            desc,
        });
        self
    }
    fn variant(mut self, v: Node) -> Self {
        self.variants.push(v);
        self
    }
    fn sub(mut self, s: Node) -> Self {
        self.subs.push(s);
        self
    }
    fn done(mut self) -> Self {
        self.done = true;
        self
    }
    fn total(&self) -> usize {
        self.req.iter().map(|a| a.ty.count()).sum()
    }
    fn find_sub(&self, word: &str) -> Option<&Node> {
        let w = word.to_lowercase();
        self.subs
            .iter()
            .find(|s| s.name == w)
            .or_else(|| self.subs.iter().find(|s| s.aliases.contains(&w.as_str())))
    }
    fn find_opt(&self, word: &str) -> Option<&Opt> {
        // An exact name, else the only name it begins, else the only one
        // that contains it.
        let w = word.to_lowercase();
        if let Some(o) = self.opts.iter().find(|o| o.name.to_lowercase() == w) {
            return Some(o);
        }
        let by = |f: &dyn Fn(&str) -> bool| {
            let m: Vec<_> = self
                .opts
                .iter()
                .filter(|o| f(&o.name.to_lowercase()))
                .collect();
            if m.len() == 1 { Some(m[0]) } else { None }
        };
        by(&|n| n.starts_with(&w)).or_else(|| by(&|n| n.contains(&w)))
    }
}

/// The player's options, as every `player` leaf has.
fn who(n: Node) -> Node {
    n.opt("player", Ty::Player, "server.commands.argtype.player.desc")
}

fn tree() -> &'static Vec<Node> {
    static TREE: OnceLock<Vec<Node>> = OnceLock::new();
    TREE.get_or_init(|| {
        let block_pos = |n: Node| n.req("x y z", Ty::Pos, "server.commands.block.set.arg.coords");
        let world = |n: Node| n.opt("world", Ty::World, "server.commands.worldthread.arg.desc");
        let give_opts = |n: Node| {
            n.opt(
                "durability",
                Ty::Double,
                "server.commands.give.durability.desc",
            )
            .opt("quality", Ty::Quality, "server.commands.give.quality.desc")
            .opt("metadata", Ty::Str, "server.commands.give.metadata.desc")
            .dflt(
                "quantity",
                Ty::Int,
                "1",
                "server.commands.give.quantity.desc",
            )
        };
        let block = Node::new("block", "server.commands.block.desc")
            .aliases(&["blocks"])
            .perm("hytale.universe.command.block")
            .group()
            .sub(
                world(block_pos(Node::new(
                    "set",
                    "server.commands.block.set.desc",
                )))
                .req("block", Ty::Block, "server.commands.block.set.arg.block")
                .done(),
            )
            .sub(
                world(block_pos(Node::new(
                    "setticking",
                    "server.commands.block.setticking.desc",
                )))
                .done(),
            )
            .sub(world(block_pos(Node::new(
                "get",
                "server.commands.block.get.desc",
            ))))
            .sub(world(block_pos(Node::new(
                "getstate",
                "server.commands.block.getstate.desc",
            ))))
            .sub(Node::new("row", "server.commands.block.row.desc").req(
                "wildcard block query",
                Ty::Str,
                "server.commands.block.row.arg.desc",
            ))
            .sub(
                Node::new("bulk", "server.commands.block.bulk.desc")
                    .group()
                    .sub(
                        Node::new("find", "server.commands.block.find.desc")
                            .req(
                                "chunkX",
                                Ty::Coord,
                                "server.commands.block.find.chunkX.desc",
                            )
                            .req(
                                "chunkZ",
                                Ty::Coord,
                                "server.commands.block.find.chunkZ.desc",
                            )
                            .req("block", Ty::Str, "server.commands.block.find.block.desc")
                            .req("count", Ty::Int, "server.commands.block.find.count.desc")
                            .req(
                                "timeout",
                                Ty::Int,
                                "server.commands.block.find.timeout.desc",
                            )
                            .opt(
                                "chunkY",
                                Ty::Coord,
                                "server.commands.block.find.chunkY.desc",
                            )
                            .opt("radius", Ty::Int, "server.commands.block.find.radius.desc")
                            .opt("world", Ty::World, "server.commands.worldthread.arg.desc")
                            .flag("confirm", ""),
                    )
                    .sub(
                        Node::new("find-here", "server.commands.block.find-here.desc")
                            .req(
                                "block",
                                Ty::Str,
                                "server.commands.block.find-here.block.desc",
                            )
                            .dflt(
                                "radius",
                                Ty::Int,
                                "3",
                                "server.commands.block.find-here.radius.desc",
                            )
                            .flag("print", "server.commands.block.find-here.print.desc"),
                    )
                    .sub(
                        Node::new("replace", "server.commands.block.bulk.replace.desc")
                            .req(
                                "find",
                                Ty::Str,
                                "server.commands.block.bulk.replace.find.desc",
                            )
                            .req(
                                "replaceWith",
                                Ty::Str,
                                "server.commands.block.bulk.replace.replace.desc",
                            )
                            .req(
                                "radius",
                                Ty::Int,
                                "server.commands.block.bulk.replace.radius.desc",
                            ),
                    ),
            )
            .sub(
                Node::new("inspectphys", "server.commands.block.inspectphys.desc")
                    .flag("all", "server.commands.block.inspectphys.all.desc"),
            )
            .sub(Node::new(
                "inspectfiller",
                "server.commands.block.inspectfiller.desc",
            ))
            .sub(Node::new(
                "inspectrotation",
                "server.commands.block.inspectrotation.desc",
            ))
            .sub(
                world(block_pos(Node::new(
                    "setstate",
                    "server.commands.block.setstate.desc",
                )))
                .req(
                    "state",
                    Ty::Str,
                    "server.commands.block.setstate.arg.state",
                ),
            );

        let give = give_opts(
            Node::new("give", "server.commands.give.desc")
                .perm("hytale.command.give.self")
                .req("item", Ty::Item, "server.commands.give.item.desc")
                .done(),
        )
        .variant(give_opts(
            Node::new("", "server.commands.give.other.desc")
                .req("player", Ty::Player, "server.commands.argtype.player.desc")
                .req("item", Ty::Item, "server.commands.give.item.desc")
                .done(),
        ))
        .sub(
            Node::new("armor", "server.commands.givearmor.desc")
                .req("search", Ty::Str, "server.commands.givearmor.search.desc")
                .opt("player", Ty::Str, "server.commands.givearmor.player.desc")
                .flag("set", "server.commands.givearmor.set.desc"),
        );

        let gamemode = Node::new("gamemode", "server.commands.gamemode.desc")
            .aliases(&["gm"])
            .perm("hytale.command.gamemode.self")
            .req(
                "gamemode",
                Ty::Mode,
                "server.commands.gamemode.gamemode.desc",
            )
            .done()
            .variant(
                Node::new("", "server.commands.gamemode.other.desc")
                    .req(
                        "gamemode",
                        Ty::Mode,
                        "server.commands.gamemode.gamemode.desc",
                    )
                    .req("player", Ty::Player, "server.commands.argtype.player.desc"),
            );

        let stat = |n: &'static str, d: &'static str| Node::new(n, d);
        let stats = Node::new("stats", "server.commands.player.stats.desc")
            .aliases(&["stat"])
            .group()
            .sub(who(stat("add", "server.commands.player.stats.add.desc")
                .req(
                    "statName",
                    Ty::Stat,
                    "server.commands.player.stats.add.arg.statName",
                )
                .req(
                    "statAmount",
                    Ty::Int,
                    "server.commands.player.stats.add.arg.statAmount",
                )
                .done()))
            .sub(who(stat("get", "server.commands.player.stats.get.desc")
                .req(
                    "statName",
                    Ty::Stat,
                    "server.commands.player.stats.get.arg.statName",
                )
                .done()))
            .sub(who(stat("set", "server.commands.player.stats.set.desc")
                .req(
                    "statName",
                    Ty::Stat,
                    "server.commands.player.stats.set.arg.statName",
                )
                .req(
                    "statValue",
                    Ty::Int,
                    "server.commands.player.stats.set.arg.statValue",
                )
                .done()))
            .sub(who(stat(
                "settomax",
                "server.commands.player.stats.settomax.desc",
            )
            .req(
                "statName",
                Ty::Stat,
                "server.commands.player.stats.settomax.arg.statName",
            )
            .done()))
            .sub(who(stat("dump", "server.commands.player.stats.dump.desc")))
            .sub(who(stat(
                "reset",
                "server.commands.player.stats.reset.desc",
            )
            .req(
                "statName",
                Ty::Stat,
                "server.commands.player.stats.reset.arg.statName",
            )
            .done()));
        let player = Node::new("player", "server.commands.player.desc")
            .perm("hytale.system.command.player")
            .group()
            .sub(who(Node::new("reset", "server.commands.player.reset.desc")))
            .sub(stats)
            .sub(
                Node::new("effect", "server.commands.player.effect.desc")
                    .group()
                    .sub(
                        Node::new("apply", "server.commands.player.effect.apply.desc")
                            .req("effect", Ty::Str, "")
                            .dflt("duration", Ty::Double, "100.0", "")
                            .variant(
                                Node::new("", "")
                                    .req("player", Ty::Player, "")
                                    .req("effect", Ty::Str, "")
                                    .dflt("duration", Ty::Double, "100.0", ""),
                            ),
                    )
                    .sub(
                        Node::new("clear", "server.commands.player.effect.clear.desc")
                            .variant(Node::new("", "").req("player", Ty::Player, "")),
                    ),
            )
            .sub(
                Node::new("respawn", "server.commands.player.respawn.desc")
                    .variant(Node::new("", "").req("player", Ty::Player, "")),
            )
            .sub(
                Node::new("lives", "server.commands.player.lives.desc")
                    .group()
                    .sub(who(Node::new("get", "")))
                    .sub(who(Node::new("set", "").req("count", Ty::Int, "")))
                    .sub(who(Node::new("clear", ""))),
            )
            .sub(
                Node::new("camera", "server.commands.camera.desc")
                    .group()
                    .sub(who(Node::new("reset", "")))
                    .sub(who(Node::new("topdown", "")))
                    .sub(who(Node::new("sidescroller", "")))
                    .sub(
                        Node::new("demo", "")
                            .group()
                            .sub(who(Node::new("activate", "")))
                            .sub(who(Node::new("deactivate", ""))),
                    ),
            )
            .sub(
                Node::new("viewradius", "server.commands.player.viewradius.desc")
                    .group()
                    .sub(who(Node::new("get", "")))
                    .sub(who(Node::new("set", "")
                        .req("radius", Ty::Int, "")
                        .flag("blocks", "")
                        .flag("bypass", ""))),
            )
            .sub(who(Node::new("zone", "server.commands.player.zone.desc")));

        // Teleport (spec 30). The rotation options are listed in the order
        // the official usage prints them.
        let at = |n: Node| {
            n.req("x", Ty::Dbl, "server.commands.teleport.x.desc")
                .req("y", Ty::Dbl, "server.commands.teleport.y.desc")
                .req("z", Ty::Dbl, "server.commands.teleport.z.desc")
        };
        let pitch = |n: Node| n.opt("pitch", Ty::Rel, "server.commands.teleport.pitch.desc");
        let yaw = |n: Node| n.opt("yaw", Ty::Rel, "server.commands.teleport.yaw.desc");
        let roll = |n: Node| n.opt("roll", Ty::Rel, "server.commands.teleport.roll.desc");
        let tp = Node::new("tp", "server.commands.tp.desc")
            .aliases(&["teleport"])
            .perm("hytale.teleport.command.tp")
            .group()
            .variant(
                Node::new("", "server.commands.teleport.otherToPlayer.desc")
                    .req("player", Ty::Player, "server.commands.argtype.player.desc")
                    .req(
                        "targetPlayer",
                        Ty::Player,
                        "server.commands.teleport.targetPlayer.desc",
                    )
                    .done(),
            )
            .variant(
                roll(yaw(pitch(at(
                    Node::new("", "server.commands.teleport.toCoordinates.desc").req(
                        "player",
                        Ty::Player,
                        "server.commands.teleport.targetPlayer.desc",
                    ),
                ))))
                .done(),
            )
            .variant(
                Node::new("", "server.commands.teleport.toPlayer.desc")
                    .req(
                        "targetPlayer",
                        Ty::Player,
                        "server.commands.teleport.targetPlayer.desc",
                    )
                    .done(),
            )
            .variant(
                roll(yaw(pitch(at(Node::new(
                    "",
                    "server.commands.teleport.toCoordinates.desc",
                )))))
                .done(),
            )
            .sub(
                roll(world(yaw(pitch(at(Node::new(
                    "all",
                    "server.commands.tpall.desc",
                ))))))
                .done(),
            )
            .sub(Node::new("home", "server.commands.home.desc").done())
            .sub(Node::new("top", "server.commands.top.desc").done())
            .sub(
                Node::new("back", "server.commands.teleport.recent.desc")
                    .opt(
                        "count",
                        Ty::Int,
                        "server.commands.teleport.back.count.desc",
                    )
                    .done(),
            )
            .sub(
                Node::new("forward", "server.commands.teleport.next.desc")
                    .opt(
                        "count",
                        Ty::Int,
                        "server.commands.teleport.forward.count.desc",
                    )
                    .done(),
            )
            .sub(Node::new("history", "server.commands.teleport.dump.desc").done())
            .sub(
                who(
                    Node::new("world", "server.commands.worldport.desc").req(
                        "worldName",
                        Ty::World,
                        "server.commands.worldport.worldName.desc",
                    ),
                )
                .done(),
            );
        let spawn_index = |n: Node| n.opt("spawnIndex", Ty::Int, "server.commands.spawn.index.desc");
        let spawn = spawn_index(
            Node::new("spawn", "server.commands.spawn.desc")
                .perm("hytale.command.spawn.self")
                .done(),
        )
        .variant(spawn_index(
            Node::new("", "server.commands.spawn.other.desc")
                .req("player", Ty::Player, "server.commands.argtype.player.desc")
                .done(),
        ))
        .sub(
            world(Node::new("set", "server.commands.spawn.set.desc"))
                .dflt(
                    "rotation",
                    Ty::Rot3,
                    "Rotation3f{x=0.0, y=0.0, z=0.0}",
                    "server.commands.spawn.set.rotation.desc",
                )
                .opt(
                    "position",
                    Ty::Pos3,
                    "server.commands.spawn.set.position.desc",
                ),
        )
        .sub(world(Node::new(
            "default",
            "server.commands.spawn.set.default.desc",
        )));
        let name = |n: Node, d: &'static str| n.req("name", Ty::Str, d);
        let warp = Node::new("warp", "server.commands.warp.desc")
            .perm("hytale.teleport.command.warp")
            .group()
            .variant(
                Node::new("", "server.commands.warp.go.desc")
                    .req("warpName", Ty::Str, "server.commands.warp.warpName.desc")
                    .done(),
            )
            .sub(
                Node::new("go", "server.commands.warp.go.desc")
                    .req("warpName", Ty::Str, "server.commands.warp.warpName.desc")
                    .done(),
            )
            .sub(
                Node::new("list", "server.commands.warp.list.desc")
                    .opt("page", Ty::Int, "server.commands.warp.list.page.desc")
                    .done(),
            )
            .sub(
                name(
                    Node::new("remove", "server.commands.warp.remove.desc"),
                    "server.commands.warp.remove.name.desc",
                )
                .flag("force", "server.commands.warp.remove.force.desc")
                .done(),
            )
            .sub(Node::new("reload", "server.commands.warp.reload.desc").done())
            .sub(
                name(
                    Node::new("set", "server.commands.warp.set.desc"),
                    "server.commands.warp.set.name.desc",
                )
                .flag("force", "server.commands.warp.set.force.desc")
                .done(),
            );
        let help = Node::new("help", "server.commands.help.desc")
            .aliases(&["?"])
            .perm("hytale.system.command.help")
            .done()
            .variant(
                Node::new("", "server.commands.help.command.desc")
                    .req("command", Ty::Str, "server.commands.help.command.name.desc")
                    .done(),
            );
        let who_cmd = Node::new("who", "server.commands.who.desc")
            .perm("hytale.system.command.who")
            .done();
        vec![
            block,
            gamemode,
            give,
            help,
            player,
            spawn,
            tp,
            warp,
            who_cmd,
        ]
    })
}

fn find_command(name: &str) -> Option<&'static Node> {
    let n = name.to_lowercase();
    tree()
        .iter()
        .find(|c| c.name == n)
        .or_else(|| tree().iter().find(|c| c.aliases.contains(&n.as_str())))
}

// ---------------------------------------------------------------------
// Fuzzy hints

/// How well `query` fits `term`: matching letters in order, more for runs.
fn fuzzy(term: &str, query: &str) -> i32 {
    let t: Vec<char> = term.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let (mut score, mut ti, mut prev) = (0i32, 0usize, i64::MIN);
    for qc in q {
        let mut found = false;
        while ti < t.len() && !found {
            if t[ti] == qc {
                score += 1;
                if prev + 1 == ti as i64 {
                    score += 2;
                }
                prev = ti as i64;
                found = true;
            }
            ti += 1;
        }
    }
    score
}

/// The best `n` of `items` for `query`, best first, equal ones in the order
/// given.
fn best(query: &str, items: &[String], n: usize) -> Vec<String> {
    let mut v: Vec<(i32, &String)> = items.iter().map(|s| (fuzzy(s, query), s)).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(n).map(|(_, s)| s.clone()).collect()
}

fn list_text(v: &[String]) -> String {
    format!("[{}]", v.join(", "))
}

/// The order a Java `HashSet` of these strings iterates (built from the
/// first `n` and grown by the rest).
fn java_set_order(names: &[String], first: usize) -> Vec<String> {
    let mut cap = 16usize;
    let want = (first as f32 / 0.75) as usize + 1;
    while cap < want {
        cap *= 2;
    }
    while names.len() * 4 > cap * 3 {
        cap *= 2;
    }
    let mut v: Vec<(usize, usize, &String)> = names
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let h = java_hash(s);
            (((h ^ (h >> 16)) as usize) & (cap - 1), i, s)
        })
        .collect();
    v.sort_by_key(|&(b, i, _)| (b, i));
    v.into_iter().map(|(_, _, s)| s.clone()).collect()
}

// ---------------------------------------------------------------------
// Messages

const ARG_COLOR: &str = "#C1E0FF";
const OPT_COLOR: &str = "#7E9EBC";

fn one_liner_req(a: &Arg) -> Msg {
    Msg::raw("<")
        .child(Msg::raw(a.name))
        .child(Msg::raw(">"))
        .color(ARG_COLOR)
}

fn one_liner_opt(o: &Opt) -> Msg {
    let m = Msg::raw("[--").child(Msg::raw(o.name));
    let m = match (o.ty, o.def) {
        (None, _) => m.child(Msg::raw("]")),
        (Some(_), Some(d)) => m
            .child(Msg::raw("="))
            .child(Msg::raw(d))
            .child(Msg::raw("]")),
        (Some(_), None) => m.child(Msg::raw("=?]")),
    };
    m.color(OPT_COLOR)
}

/// The short usage of a command (`fully` writes the whole name).
fn usage_short(n: &Node, fqn: &str, depth: usize, fully: bool) -> Msg {
    let indent = " ".repeat(depth);
    if n.subs.is_empty() && n.variants.is_empty() {
        let name = if fully { fqn } else { n.name };
        let mut m = Msg::raw(&indent).child(Msg::raw(name));
        for a in &n.req {
            m = m.child(Msg::raw(" ")).child(one_liner_req(a));
        }
        for o in &n.opts {
            m = m.child(Msg::raw(" ")).child(one_liner_opt(o));
        }
        return m;
    }
    let prefix = if depth == 0 {
        "/".to_owned()
    } else {
        indent.clone()
    };
    let mut m = Msg::raw(&prefix).child(Msg::raw(n.name));
    for v in &n.variants {
        m = m
            .child(Msg::raw("\n "))
            .child(Msg::raw(&indent))
            .child(usage_short(v, fqn, depth + 1, fully));
    }
    for s in &n.subs {
        let sub_fqn = format!("{fqn} {}", s.name);
        m = m
            .child(Msg::raw("\n "))
            .child(Msg::raw(&indent))
            .child(usage_short(s, &sub_fqn, depth + 1, fully));
    }
    m
}

/// What a group answers when called with nothing.
fn group_usage(n: &Node, fqn: &str, depth: usize) -> Msg {
    Msg::key("server.commands.help.usage")
        .child(Msg::raw(":"))
        .child(Msg::raw("  ("))
        .child(Msg::key("server.commands.help.useHelpOnAnySubCommand"))
        .child(Msg::raw(")"))
        .child(Msg::raw("\n"))
        .child(usage_short(n, fqn, depth, false))
}

fn wrong_count(n: &Node, fqn: &str, depth: usize, actual: usize) -> Msg {
    Msg::key("server.commands.parsing.error.wrongNumberRequiredParameters")
        .int("expected", n.total() as i32)
        .int("actual", actual as i32)
        .child(Msg::raw("\n"))
        .child(
            Msg::key("server.commands.help.usagecolon")
                .msg("usage", usage_short(n, fqn, depth, true)),
        )
        .child(Msg::raw("\n"))
        .child(Msg::key("server.commands.help.useHelpToLearnMore").text("command", fqn))
}

fn not_found(kind: &str, input: &str, choices: &[String]) -> Vec<Msg> {
    vec![
        Msg::key("server.commands.notfound")
            .text("type", kind)
            .text("id", input)
            .color("#ff0000"),
        Msg::key("server.general.failed.didYouMean")
            .text("choices", &list_text(&best(input, choices, 5))),
    ]
}

// ---------------------------------------------------------------------
// Checking words against their types

fn parse_int(s: &str) -> bool {
    s.parse::<i32>().is_ok()
}

fn parse_double(s: &str) -> bool {
    let t = s.trim();
    let u = t.trim_end_matches(['f', 'F', 'd', 'D']);
    matches!(t, "NaN" | "Infinity" | "+Infinity" | "-Infinity")
        || (!u.is_empty() && u.parse::<f64>().is_ok() && !u.contains(['n', 'N', 'i', 'I']))
}

/// `~`, `_` and `c` marks, then an integer.
fn parse_coord(s: &str) -> bool {
    let mut rest = s;
    loop {
        match rest.chars().next() {
            Some('_' | 'c' | '~') => {
                rest = &rest[1..];
                if rest.is_empty() {
                    return true;
                }
            }
            Some(_) => return parse_int(rest),
            None => return false,
        }
    }
}

/// Checks `words` as a `ty`; the text to run with, or the replies.
fn check(ty: Ty, words: &[&str], env: &dyn Env) -> Result<Vec<String>, Vec<Msg>> {
    let w = words[0];
    let one = |s: String| Ok(vec![s]);
    match ty {
        Ty::Int => {
            if parse_int(w) {
                one(w.to_owned())
            } else {
                Err(vec![
                    Msg::key("server.commands.parsing.argtype.integer.fail").text("input", w),
                ])
            }
        }
        Ty::Double => {
            if parse_double(w) {
                one(w.to_owned())
            } else {
                Err(vec![
                    Msg::key("server.commands.parsing.argtype.double.fail").text("input", w),
                ])
            }
        }
        Ty::Str | Ty::Quality => one(w.to_owned()),
        Ty::Stat => one(w.to_owned()),
        Ty::Pos | Ty::Coord => {
            for x in words {
                if !parse_coord(x) {
                    return Err(vec![
                        Msg::key("server.commands.parsing.argtype.integerCoordinate.fail")
                            .text("input", x),
                    ]);
                }
            }
            Ok(words.iter().map(|s| s.to_string()).collect())
        }
        Ty::Dbl => {
            if crate::teleport::Coord::parse(w).is_some() {
                one(w.to_owned())
            } else {
                Err(vec![
                    Msg::key("server.commands.parsing.argtype.doubleCoordinate.fail")
                        .text("input", w),
                ])
            }
        }
        Ty::Rel => match crate::teleport::RelFloat::parse(w) {
            Ok(_) => one(w.to_owned()),
            Err(shown) => Err(vec![Msg::raw(&format!("Invalid float: {shown}"))]),
        },
        Ty::Pos3 => {
            for x in words {
                if crate::teleport::Coord::parse(x).is_none() {
                    return Err(vec![
                        Msg::key("server.commands.parsing.argtype.doubleCoordinate.fail")
                            .text("input", x),
                    ]);
                }
            }
            Ok(words.iter().map(|s| s.to_string()).collect())
        }
        Ty::Rot3 => Ok(words.iter().map(|s| s.to_string()).collect()),
        Ty::Block => match env.block_key(w) {
            Some(k) => one(k),
            None => Err(not_found("BlockType", w, &env.block_keys())),
        },
        Ty::Item => match env.item_key(w) {
            Some(k) => one(k),
            None => Err(not_found("Item", w, &env.item_keys())),
        },
        Ty::Player => {
            // The name that is equal, else the shortest one it begins.
            let lw = w.to_lowercase();
            let names = env.players();
            let hit = names.iter().find(|n| n.to_lowercase() == lw).or_else(|| {
                names
                    .iter()
                    .filter(|n| n.to_lowercase().starts_with(&lw))
                    .min_by_key(|n| n.len())
            });
            match hit {
                Some(n) => one(n.clone()),
                None => Err(vec![
                    Msg::key("server.commands.errors.noSuchPlayer").text("username", w),
                ]),
            }
        }
        Ty::Mode => {
            let l = w.to_lowercase();
            if matches!(l.as_str(), "a" | "c" | "adventure" | "creative") {
                one(l)
            } else {
                let all = ["c", "a", "creative", "adventure"].map(String::from);
                Err(vec![
                    Msg::key("server.commands.parsing.argtype.gamemode.invalid")
                        .text("input", w)
                        .text("suggestions", &list_text(&best(&l, &all, 5))),
                ])
            }
        }
        Ty::World => {
            if w.eq_ignore_ascii_case("default") {
                one("default".to_owned())
            } else {
                Err(vec![
                    Msg::key("server.commands.errors.noSuchWorld").text("name", w),
                ])
            }
        }
    }
}

// ---------------------------------------------------------------------
// Running a line

/// Parses and checks one command line (without the `/`).
pub fn handle(line: &str, env: &dyn Env) -> Outcome {
    let name = line.split(' ').next().unwrap_or("").to_lowercase();
    let Some(cmd) = find_command(&name) else {
        if OTHER.contains(&name.as_str()) {
            return Outcome::Other;
        }
        return Outcome::Replies(vec![
            Msg::key("server.modules.command.notFound").text("cmd", line),
        ]);
    };
    let tokens = match tokenize(line) {
        Ok(t) => t,
        Err(m) => return Outcome::Replies(vec![m]),
    };
    let ctx = match contextualize(&tokens) {
        Ok(c) => c,
        Err(m) => return Outcome::Replies(vec![m]),
    };
    match run(cmd, cmd.name.to_owned(), 0, 1, &ctx, &tokens, env) {
        Step::Fail(m) => Outcome::Replies(m),
        Step::Unavailable => Outcome::Unavailable,
        Step::Valid(path, words) => finish(cmd, &path, words, env),
    }
}

enum Step {
    Fail(Vec<Msg>),
    Unavailable,
    /// The names down the tree that were walked and the words to run with.
    Valid(Vec<&'static str>, Vec<String>),
}

fn run(
    n: &'static Node,
    fqn: String,
    depth: usize,
    at: usize,
    ctx: &Line,
    tokens: &[String],
    env: &dyn Env,
) -> Step {
    let have = ctx.words(at);
    // A sub-command by the first word, else a variant by the word count.
    if !n.subs.is_empty() {
        if let Some(first) = ctx.first(at) {
            if let Some(s) = n.find_sub(first) {
                let f = format!("{fqn} {}", s.name);
                return match run(s, f, depth + 1, at + 1, ctx, tokens, env) {
                    Step::Valid(mut p, w) => {
                        p.insert(0, s.name);
                        Step::Valid(p, w)
                    }
                    other => other,
                };
            }
        }
    }
    if n.total() != have {
        if let Some(v) = n.variants.iter().find(|v| v.total() == have) {
            return match run(v, fqn, depth + 1, at, ctx, tokens, env) {
                Step::Valid(mut p, w) => {
                    p.insert(0, "");
                    Step::Valid(p, w)
                }
                other => other,
            };
        }
    }
    if n.collection && have != 0 {
        let first = ctx.first(at).unwrap_or("");
        let mut names: Vec<String> = n.subs.iter().map(|s| s.name.to_owned()).collect();
        let first_n = names.len();
        for s in &n.subs {
            names.extend(s.aliases.iter().map(|a| a.to_string()));
        }
        let ordered = java_set_order(&names, first_n);
        let top = best(first, &ordered, 5);
        let prefix = format!("\n/{fqn} ");
        let sug = format!("{prefix}{}", top.join(&prefix));
        return Step::Fail(vec![
            Msg::key("server.commands.parsing.error.commandCollectionSubcommandNotFound")
                .text("subcommand", first)
                .text("suggestions", &sug),
        ]);
    }
    if ctx.opts.iter().any(|(k, _)| k == "help") {
        return Step::Unavailable;
    }
    if n.total() != have {
        return Step::Fail(vec![wrong_count(n, &fqn, depth, have)]);
    }
    if n.collection {
        return Step::Fail(vec![group_usage(n, &fqn, depth)]);
    }
    // Required words.
    let mut words: Vec<String> = Vec::new();
    let mut slot = at;
    for a in &n.req {
        let c = a.ty.count();
        let mut got: Vec<&str> = Vec::new();
        for _ in 0..c {
            match ctx.slots.get(slot) {
                Some(Slot::One(w)) => got.push(w),
                _ => {
                    return Step::Fail(vec![
                        Msg::key("server.commands.parsing.error.notAList").text("name", a.name),
                    ]);
                }
            }
            slot += 1;
        }
        match check(a.ty, &got, env) {
            Ok(v) => words.extend(v),
            Err(m) => return Step::Fail(m),
        }
    }
    // Options.
    let mut opt_words: Vec<String> = Vec::new();
    for (key, groups) in &ctx.opts {
        let Some(o) = n.find_opt(key) else {
            return Step::Fail(vec![
                Msg::key("server.commands.parsing.error.couldNotFindOptionalArgName")
                    .text("input", key),
            ]);
        };
        let given: Vec<&str> = groups
            .first()
            .map(|g| g.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let want = o.ty.map_or(0, Ty::count);
        if given.is_empty() && o.def.is_some() {
            continue;
        }
        if given.len() != want {
            return Step::Fail(vec![
                Msg::key("server.commands.parsing.error.wrongNumberOfParametersForArgument")
                    .text("argument", o.name)
                    .int("expected", want as i32)
                    .int("actual", given.len() as i32)
                    .text("input", &given.join(" ")),
            ]);
        }
        if let Some(ty) = o.ty {
            match check(ty, &given, env) {
                Ok(v) => {
                    // Aerie has one world: nothing to pass on.
                    if ty != Ty::World {
                        opt_words.push(format!("--{}={}", o.name, v.join(" ")));
                    }
                }
                Err(m) => return Step::Fail(m),
            }
        } else {
            opt_words.push(format!("--{}", o.name));
        }
    }
    // Stat names are looked up when the command runs, after every word
    // was read.
    for (a, w) in n.req.iter().zip(words.iter_mut()) {
        if a.ty == Ty::Stat {
            match env.stat_key(w) {
                Some(k) => *w = k,
                None => {
                    let mut m = vec![
                        Msg::key("server.commands.entityStats.entityStatNotFound").text("name", w),
                    ];
                    m.push(
                        Msg::key("server.general.failed.didYouMean")
                            .text("choices", &list_text(&best(w, &env.stat_keys(), 5))),
                    );
                    return Step::Fail(m);
                }
            }
        }
    }
    if !n.done {
        return Step::Unavailable;
    }
    words.extend(opt_words);
    Step::Valid(vec![], words)
}

/// The line the world runs.
fn finish(cmd: &Node, path: &[&str], words: Vec<String>, env: &dyn Env) -> Outcome {
    match cmd.name {
        "help" => {
            return match words.first() {
                None => Outcome::Page(help_page(None)),
                Some(c) => Outcome::Page(help_page(Some(c))),
            };
        }
        "who" => return Outcome::Replies(vec![who_list(&env.players())]),
        _ => {}
    }
    let mut out = vec![cmd.name.to_owned()];
    // `/give ITEM` gives to oneself, as `/give <you> ITEM` does.
    if cmd.name == "give" && path.is_empty() {
        out.push("@s".to_owned());
    }
    // Sub-command names (the empty ones are variants).
    out.extend(path.iter().filter(|p| !p.is_empty()).map(|p| p.to_string()));
    out.extend(words);
    Outcome::Run(out.join(" "))
}

// ---------------------------------------------------------------------
// /who

fn who_list(players: &[String]) -> Msg {
    let header = Msg::key("server.formatting.list.header")
        .int("count", players.len() as i32)
        .msg("header", Msg::raw("default"));
    let one = |n: &str| {
        Msg::key("server.commands.who.playerWithDisplayName")
            .text("username", n)
            .msg("displayName", Msg::raw(n))
    };
    let mut m = Msg::default().child(header);
    if players.len() <= 4 {
        m = m.child(Msg::key("server.formatting.list.inlineHeaderSuffix"));
    }
    if players.is_empty() {
        return m.child(Msg::key("server.formatting.list.empty"));
    }
    if players.len() <= 4 {
        for (i, p) in players.iter().enumerate() {
            m = m.child(one(p));
            if i + 1 < players.len() {
                m = m.child(Msg::key("server.formatting.list.itemSeparator"));
            }
        }
    } else {
        for p in players {
            m = m
                .child(Msg::raw("\n"))
                .child(Msg::key("server.formatting.list.row").msg("value", one(p)));
        }
    }
    m
}

// ---------------------------------------------------------------------
// /help: the page

/// A message as the interface language writes it.
enum Ui {
    Raw(String, Vec<Ui>),
    Id(String),
    IdWith(String, Vec<(&'static str, Ui)>, Vec<(&'static str, String)>),
}

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

impl Ui {
    fn json(&self) -> String {
        match self {
            Ui::Raw(t, ch) if ch.is_empty() => format!("{{\"RawText\": {}}}", js(t)),
            Ui::Raw(t, ch) => {
                let c: Vec<String> = ch.iter().map(Ui::json).collect();
                format!(
                    "{{\"RawText\": {}, \"Children\": [{}]}}",
                    js(t),
                    c.join(", ")
                )
            }
            Ui::Id(k) => format!("{{\"MessageId\": {}}}", js(k)),
            Ui::IdWith(k, mp, p) => {
                let mut s = format!("{{\"MessageId\": {}", js(k));
                if !mp.is_empty() {
                    let v: Vec<String> = mp
                        .iter()
                        .map(|(n, m)| format!("{}: {}", js(n), m.json()))
                        .collect();
                    s.push_str(&format!(", \"MessageParams\": {{{}}}", v.join(", ")));
                }
                if !p.is_empty() {
                    let v: Vec<String> = p
                        .iter()
                        .map(|(n, m)| format!("{}: {}", js(n), js(m)))
                        .collect();
                    s.push_str(&format!(", \"Params\": {{{}}}", v.join(", ")));
                }
                s.push('}');
                s
            }
        }
    }
}

fn raw(t: &str) -> Ui {
    Ui::Raw(t.to_owned(), vec![])
}

fn id(k: &str) -> Ui {
    Ui::Id(k.to_owned())
}

/// The short usage on the page: name, required words, an options marker.
fn usage_simple(n: &Node, fqn: &str) -> Ui {
    let mut ch = vec![raw(fqn)];
    for a in &n.req {
        ch.push(raw(" <"));
        ch.push(id(a.name));
        ch.push(raw(">"));
    }
    if !n.opts.is_empty() {
        ch.push(raw(" ["));
        ch.push(id("server.customUI.commandListPage.optionsIndicator"));
        ch.push(raw("]"));
    }
    Ui::Raw("/".to_owned(), ch)
}

struct Page {
    cmds: Vec<proto::CustomUICommand>,
    binds: Vec<proto::CustomUIEventBinding>,
}

impl Page {
    fn cmd(
        &mut self,
        t: proto::CustomUICommandType,
        sel: Option<&str>,
        data: Option<String>,
        text: Option<&str>,
    ) {
        self.cmds.push(proto::CustomUICommand {
            r#type: t,
            selector: sel.map(str::to_owned),
            data,
            text: text.map(str::to_owned),
        });
    }
    fn set(&mut self, sel: &str, json: String) {
        self.cmd(
            proto::CustomUICommandType::SET,
            Some(sel),
            Some(format!("{{\"0\": {json}}}")),
            None,
        );
    }
    fn set_ui(&mut self, sel: &str, m: &Ui) {
        self.set(sel, m.json());
    }
    fn show(&mut self, sel: &str, on: bool) {
        self.set(sel, on.to_string());
    }
    fn clear(&mut self, sel: &str) {
        self.cmd(proto::CustomUICommandType::CLEAR, Some(sel), None, None);
    }
    fn append(&mut self, sel: Option<&str>, file: &str) {
        self.cmd(proto::CustomUICommandType::APPEND, sel, None, Some(file));
    }
    fn bind(&mut self, t: proto::CustomUIEventBindingType, sel: &str, data: String, locks: bool) {
        self.binds.push(proto::CustomUIEventBinding {
            r#type: t,
            selector: Some(sel.to_owned()),
            data: Some(data),
            locks_interface: locks,
        });
    }
    fn click(&mut self, sel: &str, key: &str, value: &str) {
        self.bind(
            proto::CustomUIEventBindingType::ACTIVATING,
            sel,
            format!("{{{}: {}}}", js(key), js(value)),
            true,
        );
    }
}

/// The order the page lists a command's argument types in.
fn type_order(n: &Node) -> Vec<Ty> {
    let mut v: Vec<Ty> = Vec::new();
    let all = n
        .req
        .iter()
        .map(|a| a.ty)
        .chain(n.opts.iter().filter_map(|o| o.ty));
    for t in all {
        let t = if t == Ty::Stat { Ty::Str } else { t };
        if !v.contains(&t) {
            v.push(t);
        }
    }
    // The official page keeps a hash set's order; the known ones.
    let rank = |t: &Ty| match t {
        Ty::Quality => 0,
        Ty::Double => 1,
        Ty::Item => 2,
        Ty::Int => 3,
        Ty::Str => 4,
        _ => 5,
    };
    v.sort_by_key(rank);
    v
}

fn select(p: &mut Page, n: &Node) {
    p.set_ui("#CommandName.TextSpans", &raw(n.name));
    // Sub-command cards, three to a row.
    p.clear("#SubcommandCards");
    if n.subs.is_empty() {
        p.show("#SubcommandSection.Visible", false);
    } else {
        p.show("#SubcommandSection.Visible", true);
        for (i, s) in n.subs.iter().enumerate() {
            let (row, col) = (i / 3, i % 3);
            if col == 0 {
                p.cmd(
                    proto::CustomUICommandType::APPEND_INLINE,
                    Some("#SubcommandCards"),
                    None,
                    Some("Group { LayoutMode: Left; Anchor: (Bottom: 0); }"),
                );
            }
            p.append(
                Some(&format!("#SubcommandCards[{row}]")),
                "Pages/SubcommandCard.ui",
            );
            let at = format!("#SubcommandCards[{row}][{col}]");
            p.set_ui(&format!("{at} #SubcommandName.TextSpans"), &raw(s.name));
            p.set_ui(
                &format!("{at} #SubcommandUsage.TextSpans"),
                &usage_simple(s, &format!("{} {}", n.name, s.name)),
            );
            p.set_ui(
                &format!("{at} #SubcommandDescription.TextSpans"),
                &if s.desc.is_empty() {
                    raw("")
                } else {
                    id(s.desc)
                },
            );
            p.click(&at, "Subcommand", s.name);
        }
    }
    p.show("#BackButton.Visible", false);
    // The command itself.
    p.set_ui(
        "#CommandDescription.TextSpans",
        &if n.desc.is_empty() {
            raw("")
        } else {
            id(n.desc)
        },
    );
    p.clear("#VariantsList");
    if n.variants.is_empty() {
        p.show("#VariantsSection.Visible", false);
    } else {
        p.show("#VariantsSection.Visible", true);
        for (i, v) in n.variants.iter().enumerate() {
            p.append(Some("#VariantsList"), "Pages/VariantCard.ui");
            p.set_ui(
                &format!("#VariantsList[{i}] #VariantUsage.TextSpans"),
                &usage_simple(v, n.name),
            );
            p.click(
                &format!("#VariantsList[{i}]"),
                "Variant",
                &v.total().to_string(),
            );
        }
    }
    if n.aliases.is_empty() {
        p.show("#AliasesSection.Visible", false);
    } else {
        p.show("#AliasesSection.Visible", true);
        p.set_ui("#AliasesList.TextSpans", &raw(&n.aliases.join(", ")));
    }
    if n.perm.is_empty() {
        p.show("#PermissionSection.Visible", false);
    } else {
        p.show("#PermissionSection.Visible", true);
        p.set_ui("#PermissionLabel.TextSpans", &raw(n.perm));
    }
    p.set_ui("#CommandUsageLabel.TextSpans", &usage_simple(n, n.name));
    // Parameters.
    for l in [
        "#RequiredArgumentsList",
        "#OptionalArgumentsList",
        "#DefaultArgumentsList",
        "#FlagArgumentsList",
    ] {
        p.clear(l);
    }
    let desc_ui = |d: &str| {
        if d.is_empty() {
            id("server.customUI.commandListPage.noDescription")
        } else {
            id(d)
        }
    };
    let ty_ui = |t: Ty| {
        let t = if t == Ty::Stat { Ty::Str } else { t };
        Ui::IdWith(
            "server.customUI.commandListPage.paramType".into(),
            vec![("type", id(t.info().0))],
            vec![],
        )
    };
    for (i, a) in n.req.iter().enumerate() {
        let at = format!("#RequiredArgumentsList[{i}]");
        p.append(Some("#RequiredArgumentsList"), "Pages/ParameterItem.ui");
        p.set_ui(&format!("{at} #ParamName.TextSpans"), &raw(a.name));
        p.set_ui(
            &format!("{at} #ParamTag.TextSpans"),
            &id("server.customUI.commandListPage.required"),
        );
        p.set_ui(&format!("{at} #ParamType.TextSpans"), &ty_ui(a.ty));
        p.set_ui(
            &format!("{at} #ParamDescription.TextSpans"),
            &desc_ui(a.desc),
        );
    }
    let (mut oi, mut di, mut fi) = (0, 0, 0);
    for o in &n.opts {
        let (list, k, tag) = match (o.ty, o.def) {
            (None, _) => ("#FlagArgumentsList", &mut fi, "flag"),
            (Some(_), Some(_)) => ("#DefaultArgumentsList", &mut di, "default"),
            (Some(_), None) => ("#OptionalArgumentsList", &mut oi, "optional"),
        };
        let at = format!("{list}[{k}]");
        *k += 1;
        p.append(Some(list), "Pages/ParameterItem.ui");
        let name = if o.ty.is_none() {
            format!("--{}", o.name)
        } else {
            format!("--{} <{}>", o.name, o.name)
        };
        p.set_ui(&format!("{at} #ParamName.TextSpans"), &raw(&name));
        p.set_ui(
            &format!("{at} #ParamTag.TextSpans"),
            &id(&format!("server.customUI.commandListPage.{tag}")),
        );
        let ty = match (o.ty, o.def) {
            (None, _) => id("server.customUI.commandListPage.paramTypeFlag"),
            (Some(t), Some(d)) => Ui::IdWith(
                "server.customUI.commandListPage.paramTypeDefault".into(),
                vec![("default", id(d)), ("type", id(t.info().0))],
                vec![],
            ),
            (Some(t), None) => ty_ui(t),
        };
        p.set_ui(&format!("{at} #ParamType.TextSpans"), &ty);
        p.set_ui(
            &format!("{at} #ParamDescription.TextSpans"),
            &desc_ui(o.desc),
        );
    }
    p.show(
        "#ParametersSection.Visible",
        !n.req.is_empty() || !n.opts.is_empty(),
    );
    // Argument types.
    p.clear("#ArgumentTypesList");
    let types = type_order(n);
    p.show("#ArgumentTypesSection.Visible", !types.is_empty());
    for (i, t) in types.iter().enumerate() {
        let (name, usage, ex) = t.info();
        let at = format!("#ArgumentTypesList[{i}]");
        p.append(Some("#ArgumentTypesList"), "Pages/ArgumentTypeItem.ui");
        p.set_ui(&format!("{at} #TypeName.TextSpans"), &id(name));
        p.set_ui(&format!("{at} #TypeDescription.TextSpans"), &id(usage));
        if ex.is_empty() {
            p.show(&format!("{at} #TypeExamples.Visible"), false);
        } else {
            p.set_ui(
                &format!("{at} #TypeExamples.TextSpans"),
                &Ui::IdWith(
                    "server.customUI.commandListPage.examples".into(),
                    vec![],
                    vec![("examples", ex.join("', '"))],
                ),
            );
        }
    }
}

/// The page `/help` opens, with `first` (a command or alias) selected when
/// it is one of the commands listed.
fn help_page(first: Option<&str>) -> proto::CustomPage {
    let mut names: Vec<&Node> = tree().iter().collect();
    names.sort_by_key(|n| n.name);
    let mut p = Page {
        cmds: vec![],
        binds: vec![],
    };
    p.append(None, "Pages/CommandListPage.ui");
    p.bind(
        proto::CustomUIEventBindingType::VALUE_CHANGED,
        "#SearchInput",
        "{\"@SearchQuery\": \"#SearchInput.Value\"}".into(),
        false,
    );
    p.click("#BackButton", "NavigateUp", "true");
    p.click("#SendToChatButton", "SendToChat", "true");
    p.clear("#CommandList");
    for (i, n) in names.iter().enumerate() {
        p.append(Some("#CommandList"), "Pages/BasicTextButton.ui");
        p.set_ui(&format!("#CommandList[{i}].TextSpans"), &raw(n.name));
        p.click(&format!("#CommandList[{i}]"), "Command", n.name);
    }
    let pick = first
        .and_then(find_command)
        .and_then(|c| names.iter().position(|n| n.name == c.name))
        .unwrap_or(0);
    select(&mut p, names[pick]);
    p.set(
        &format!("#CommandList[{pick}].Style"),
        "{\"$Document\": \"Pages/BasicTextButton.ui\", \"@Value\": \"SelectedLabelStyle\"}".into(),
    );
    proto::CustomPage {
        key: Some("CommandListPage".into()),
        is_initial: true,
        clear: true,
        lifetime: proto::CustomPageLifetime::CAN_DISMISS,
        commands: Some(p.cmds),
        event_bindings: Some(p.binds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct W;
    impl Env for W {
        fn players(&self) -> Vec<String> {
            vec!["Prober3".into(), "Ana".into()]
        }
        fn block_key(&self, n: &str) -> Option<String> {
            ["Rock_Stone", "Empty"]
                .iter()
                .find(|k| k.eq_ignore_ascii_case(n))
                .map(|s| s.to_string())
        }
        fn item_key(&self, n: &str) -> Option<String> {
            self.block_key(n)
        }
        fn stat_key(&self, n: &str) -> Option<String> {
            ["Health", "Mana"]
                .iter()
                .find(|k| k.eq_ignore_ascii_case(n))
                .map(|s| s.to_string())
        }
        fn block_keys(&self) -> Vec<String> {
            vec!["Rock_Stone".into(), "Empty".into()]
        }
        fn item_keys(&self) -> Vec<String> {
            self.block_keys()
        }
        fn stat_keys(&self) -> Vec<String> {
            vec!["Health".into(), "Mana".into()]
        }
    }

    fn words(s: &str) -> Vec<String> {
        tokenize(s).ok().unwrap()
    }

    fn keys(o: Outcome) -> Vec<String> {
        match o {
            Outcome::Replies(v) => v
                .into_iter()
                .map(|m| m.into_inner().message_id.unwrap_or_default())
                .collect(),
            Outcome::Run(l) => vec![format!("run {l}")],
            Outcome::Page(_) => vec!["page".into()],
            Outcome::Unavailable => vec!["unavailable".into()],
            Outcome::Other => vec!["other".into()],
        }
    }

    #[test]
    fn splits_words_like_the_official_server() {
        assert_eq!(words("give  a   b"), ["give", "a", "b"]);
        assert_eq!(words("give \"a b\" c"), ["give", "\"a b\"", "c"]);
        assert_eq!(words("give a,b"), ["give", "a", ",", "b"]);
        assert_eq!(words("give [a b]"), ["give", "[", "a", "b", "]"]);
        assert!(tokenize("give \"a").is_err());
        assert!(tokenize("give a\\").is_err());
        assert!(tokenize("give a]").is_err());
    }

    #[test]
    fn unknown_and_other_commands() {
        assert_eq!(
            keys(handle("nothing a b", &W)),
            ["server.modules.command.notFound"]
        );
        assert_eq!(
            keys(handle("list", &W)),
            ["server.modules.command.notFound"]
        );
        assert_eq!(keys(handle("pos", &W)), ["other"]);
    }

    #[test]
    fn counts_and_types() {
        assert_eq!(
            keys(handle("block set 1 2", &W)),
            ["server.commands.parsing.error.wrongNumberRequiredParameters"]
        );
        assert_eq!(
            keys(handle("block set a b c Rock_Stone", &W)),
            ["server.commands.parsing.argtype.integerCoordinate.fail"]
        );
        assert_eq!(
            keys(handle("block set 1 2 3 Nope", &W)),
            [
                "server.commands.notfound",
                "server.general.failed.didYouMean"
            ]
        );
        assert_eq!(
            keys(handle("block set ~ 2 3 rock_stone --world=default", &W)),
            ["run block set ~ 2 3 Rock_Stone"]
        );
        assert_eq!(keys(handle("gm c", &W)), ["run gamemode c"]);
        assert_eq!(
            keys(handle("gamemode foo", &W)),
            ["server.commands.parsing.argtype.gamemode.invalid"]
        );
        assert_eq!(
            keys(handle("give prob Rock_Stone --quantity=2", &W)),
            ["run give Prober3 Rock_Stone --quantity=2"]
        );
        assert_eq!(
            keys(handle("give Rock_Stone --all", &W)),
            ["server.commands.parsing.error.couldNotFindOptionalArgName"]
        );
        assert_eq!(
            keys(handle("player stats get health", &W)),
            ["run player stats get Health"]
        );
        assert_eq!(
            keys(handle("player stats get Foo", &W)),
            [
                "server.commands.entityStats.entityStatNotFound",
                "server.general.failed.didYouMean"
            ]
        );
    }

    #[test]
    fn groups_print_usage_or_suggest() {
        assert_eq!(keys(handle("block", &W)), ["server.commands.help.usage"]);
        let sug = |l: &str| match handle(l, &W) {
            Outcome::Replies(v) => {
                let m = v[0].clone().into_inner();
                match &m
                    .params
                    .unwrap()
                    .iter()
                    .find(|p| p.0 == "suggestions")
                    .unwrap()
                    .1
                {
                    proto::ParamValue::StringParamValue(s) => s.value.clone().unwrap(),
                    _ => panic!(),
                }
            }
            _ => panic!(),
        };
        assert_eq!(
            sug("block foo"),
            "\n/block inspectfiller\n/block getstate\n/block set\n/block setstate\n/block get"
        );
        assert_eq!(
            sug("block stone"),
            "\n/block getstate\n/block inspectrotation\n/block set\n/block inspectfiller\n/block setstate"
        );
    }

    #[test]
    fn who_and_help() {
        assert_eq!(keys(handle("help", &W)), ["page"]);
        assert_eq!(keys(handle("help give", &W)), ["page"]);
        assert_eq!(keys(handle("?", &W)), ["page"]);
        assert_eq!(
            keys(handle("help a b", &W)),
            ["server.commands.parsing.error.wrongNumberRequiredParameters"]
        );
        assert!(keys(handle("who", &W)).len() == 1);
    }
}
