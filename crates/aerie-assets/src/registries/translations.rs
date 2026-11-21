//! `UpdateTranslations` (64): the client's language strings (spec 07).
//!
//! Sources, for `en-US` (other languages fall back to it key by key):
//!
//! - every `.lang` file under `Server/Languages/en-US/`, keyed
//!   `<file path with dots>.<key>` (`server.lang` gives `server.…`,
//!   `foo/bar.lang` gives `foo.bar.…`);
//! - names generated from assets: each block type with a crafting bench
//!   (`Bench` of type `Crafting` or `DiagramCrafting`) gives
//!   `server.items.<block>.bench.categories.<category>.name` and
//!   `….itemCategories.<item category>.name`, and any bench with a
//!   `DescriptiveLabel` gives `server.items.<block>.bench.descriptiveLabel`;
//!   each fieldcraft category with a `Name` gives
//!   `fieldcraftCategories.<id>.name`. These replace file entries.
//!
//! `.lang` format: `key = value` per line, `#` comments, blank lines
//! skipped; a value ending in `\` continues on the next line (trimmed); a
//! one-line value in double quotes loses them; `\n` and `\t` are escapes;
//! the first definition of a key wins. Lines are trimmed of ASCII
//! whitespace only, so a byte order mark stays part of the first line (the
//! official server reads it that way too, and a commented first line then
//! becomes a key).
//!
//! The official server also has strings built into it (commands, server
//! UI); it only uses them on its side for keys the packs lack, so they are
//! not part of the packet.

use std::collections::BTreeMap;

use aerie_protocol::proto::UpdateTranslations;
use serde_json::Value;

use super::item_categories;
use crate::values::Get;
use crate::{Result, ctx::Ctx};

pub const LANGUAGE: &str = "en-US";
pub const BLOCKS: &str = "Item/Block/Blocks";

/// Parses one `.lang` file.
pub fn parse_lang(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let put = |out: &mut Vec<(String, String)>, k: String, v: String| {
        if !out.iter().any(|(x, _)| *x == k) {
            out.push((k, v.replace("\\n", "\n").replace("\\t", "\t")));
        }
    };
    let mut cur: Option<(String, String)> = None;
    // Java's `String.trim`: only characters up to U+0020.
    let jtrim = |s: &str| s.trim_matches(|c: char| c <= ' ').to_owned();
    for line in text.lines() {
        let line = &jtrim(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match cur.take() {
            None => {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                let (k, v) = (jtrim(k), jtrim(v));
                let (k, v) = (k.as_str(), v.as_str());
                if k.is_empty() || v.is_empty() {
                    continue;
                }
                if let Some(head) = v.strip_suffix('\\') {
                    cur = Some((k.to_owned(), head.to_owned()));
                } else {
                    let lit = if v.len() > 1 && v.starts_with('"') && v.ends_with('"') {
                        &v[1..v.len() - 1]
                    } else {
                        v
                    };
                    put(&mut out, k.to_owned(), lit.to_owned());
                }
            }
            Some((k, mut v)) => match line.strip_suffix('\\') {
                Some(head) => {
                    v.push_str(&jtrim(head));
                    cur = Some((k, v));
                }
                None => {
                    v.push_str(line);
                    put(&mut out, k, v);
                }
            },
        }
    }
    if let Some((k, v)) = cur {
        put(&mut out, k, v);
    }
    out
}

pub fn messages(ctx: &Ctx, language: &str) -> Result<BTreeMap<String, String>> {
    let root = format!("Server/Languages/{language}/");
    let mut out = BTreeMap::new();
    for name in ctx.pack.list(root.trim_end_matches('/'), ".lang") {
        let rel = name[root.len()..]
            .trim_end_matches(".lang")
            .replace('/', ".");
        let bytes = ctx.pack.read(name)?;
        for (k, v) in parse_lang(&String::from_utf8_lossy(&bytes)) {
            out.entry(format!("{rel}.{k}")).or_insert(v);
        }
    }
    if language == LANGUAGE {
        let blocks = ctx.store(BLOCKS)?;
        for k in blocks.own.keys() {
            let b = blocks.resolved(k, "BlockType");
            let Some(bench) = b.obj("Bench") else {
                continue;
            };
            if matches!(
                bench.str_at("Type").as_deref(),
                Some("Crafting") | Some("DiagramCrafting")
            ) {
                for c in bench
                    .get("Categories")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let (Some(cid), Some(name)) = (c.str_at("Id"), c.str_at("Name")) else {
                        continue;
                    };
                    out.insert(
                        format!("server.items.{k}.bench.categories.{cid}.name"),
                        name,
                    );
                    for ic in c
                        .get("ItemCategories")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let (Some(iid), Some(iname)) = (ic.str_at("Id"), ic.str_at("Name")) {
                            out.insert(
                                format!("server.items.{k}.bench.categories.{cid}.itemCategories.{iid}.name"),
                                iname,
                            );
                        }
                    }
                }
            }
            if let Some(l) = bench.str_at("DescriptiveLabel") {
                out.insert(format!("server.items.{k}.bench.descriptiveLabel"), l);
            }
        }
        let fieldcraft = ctx.store(item_categories::FIELDCRAFT)?;
        for (k, v) in &fieldcraft.own {
            if let Some(name) = v.str_at("Name") {
                out.insert(format!("fieldcraftCategories.{k}.name"), name);
            }
        }
    }
    Ok(out)
}

pub fn build(ctx: &Ctx) -> Result<UpdateTranslations> {
    Ok(UpdateTranslations {
        r#type: super::INIT,
        translations: Some(messages(ctx, LANGUAGE)?.into_iter().collect()),
    })
}
