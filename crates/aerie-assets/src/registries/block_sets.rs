//! `UpdateBlockSets` (46) from `Item/Block/Sets` (spec 07): named sets of
//! block indexes, keyed by name.
//!
//! A set starts with its parent's blocks, then (in this order) adds every
//! block for `IncludeAll`, adds `IncludeBlockTypes`, `IncludeBlockGroups`,
//! `IncludeHitboxTypes`, removes `ExcludeBlockTypes`, `ExcludeBlockGroups`,
//! `ExcludeHitboxTypes`, then adds `IncludeCategories` and removes
//! `ExcludeCategories`. Names may be globs (`*` any run, `?` one character).
//! A category entry is a list: its first category's blocks, minus those of
//! each further one. A block's group and hitbox type are its own (hitbox
//! `Full` by default, cut at `|`); its categories are those of the item with
//! the same key.
//!
//! The official server sends each set as a fastutil hash set; Aerie sends
//! the same blocks in ascending order (the client reads a set).

use std::collections::{BTreeSet, HashMap};

use aerie_protocol::proto::{BlockSet, UpdateBlockSets};
use serde_json::Value;

use super::blocks;
use crate::values::Get;
use crate::{Result, ctx::Ctx, store::Store};

pub const PATH: &str = "Item/Block/Sets";

/// `StringUtil.isGlobMatching`.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => {
                let rest: Vec<char> = rest.iter().copied().skip_while(|c| *c == '*').collect();
                if rest.is_empty() {
                    return true;
                }
                (0..t.len()).any(|i| t[i] == rest[0] && go(&rest[1..], &t[i + 1..]))
            }
            Some((c, rest)) => match t.split_first() {
                Some((x, trest)) if *c == '?' || c == x => go(rest, trest),
                _ => false,
            },
        }
    }
    pattern == text || {
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = text.chars().collect();
        go(&p, &t)
    }
}

type Lookup = HashMap<String, BTreeSet<i32>>;

struct Tables {
    all: BTreeSet<i32>,
    names: Lookup,
    groups: Lookup,
    hitboxes: Lookup,
    categories: Lookup,
}

fn tables(ctx: &Ctx) -> Result<Tables> {
    let store = ctx.store(blocks::PATH)?;
    let items = ctx.store(blocks::ITEMS)?;
    let order = ctx.order(blocks::PATH, blocks::BUILTIN)?;
    let idx = ctx.indexes(blocks::PATH, blocks::BUILTIN)?;
    let mut t = Tables {
        all: BTreeSet::new(),
        names: Lookup::new(),
        groups: Lookup::new(),
        hitboxes: Lookup::new(),
        categories: Lookup::new(),
    };
    for (n, k) in order.iter().enumerate() {
        let i = idx.get(&k.to_lowercase()).copied().unwrap_or(n as i32);
        t.all.insert(i);
        t.names.entry(k.clone()).or_default().insert(i);
        let (group, hitbox) = match store.own.get(k) {
            Some(_) => {
                let a = store.resolved(k, "BlockType");
                (a.str_at("Group"), a.str_at("HitboxType"))
            }
            None => (
                k.eq_ignore_ascii_case("Empty").then(|| "@Tech".to_owned()),
                None,
            ),
        };
        if let Some(g) = group.filter(|g| !g.is_empty()) {
            t.groups.entry(g).or_default().insert(i);
        }
        let hitbox = hitbox.unwrap_or_else(|| "Full".into());
        let hitbox = match hitbox.find('|') {
            Some(0) => None,
            Some(p) => Some(hitbox[..p].to_owned()),
            None => Some(hitbox),
        };
        if let Some(h) = hitbox.filter(|h| !h.is_empty()) {
            t.hitboxes.entry(h).or_default().insert(i);
        }
        if items.own.contains_key(k) {
            for c in items
                .resolved(k, "Item")
                .strings("Categories")
                .unwrap_or_default()
            {
                t.categories.entry(c).or_default().insert(i);
            }
        }
    }
    Ok(t)
}

fn matching<'a>(map: &'a Lookup, name: &str) -> Vec<&'a BTreeSet<i32>> {
    if name.contains(['*', '?']) {
        map.iter()
            .filter(|(k, _)| glob(name, k))
            .map(|(_, v)| v)
            .collect()
    } else {
        map.get(name).into_iter().collect()
    }
}

fn set(
    store: &Store,
    t: &Tables,
    k: &str,
    done: &mut HashMap<String, BTreeSet<i32>>,
    depth: usize,
) -> BTreeSet<i32> {
    if let Some(s) = done.get(k) {
        return s.clone();
    }
    let Some(a) = store.own.get(k) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    if let Some(p) = store.parents.get(k).filter(|p| !p.is_empty()) {
        if depth < 32 {
            out.extend(set(store, t, p, done, depth + 1));
        }
    }
    if a.bool_or("IncludeAll", false) {
        out.extend(t.all.iter().copied());
    }
    let names = |key: &str| a.strings(key).unwrap_or_default();
    for (key, map) in [
        ("IncludeBlockTypes", &t.names),
        ("IncludeBlockGroups", &t.groups),
        ("IncludeHitboxTypes", &t.hitboxes),
    ] {
        for n in names(key) {
            for s in matching(map, &n) {
                out.extend(s.iter().copied());
            }
        }
    }
    for (key, map) in [
        ("ExcludeBlockTypes", &t.names),
        ("ExcludeBlockGroups", &t.groups),
        ("ExcludeHitboxTypes", &t.hitboxes),
    ] {
        for n in names(key) {
            for s in matching(map, &n) {
                out.retain(|i| !s.contains(i));
            }
        }
    }
    for (key, add) in [("IncludeCategories", true), ("ExcludeCategories", false)] {
        for cats in a.get(key).and_then(Value::as_array).into_iter().flatten() {
            let cats: Vec<&str> = cats
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let Some(first) = cats.first().and_then(|c| t.categories.get(*c)) else {
                continue;
            };
            let mut s = first.clone();
            let mut ok = true;
            for c in &cats[1..] {
                match t.categories.get(*c) {
                    Some(x) => s.retain(|i| !x.contains(i)),
                    None => ok = false,
                }
                if !ok || s.is_empty() {
                    break;
                }
            }
            if !ok || s.is_empty() {
                continue;
            }
            if add {
                out.extend(s);
            } else {
                out.retain(|i| !s.contains(i));
            }
        }
    }
    done.insert(k.to_owned(), out.clone());
    out
}

pub fn build(ctx: &Ctx) -> Result<UpdateBlockSets> {
    let store = ctx.store(PATH)?;
    let t = tables(ctx)?;
    let mut done = HashMap::new();
    let list = store
        .own
        .keys()
        .map(|k| {
            (
                k.clone(),
                BlockSet {
                    name: Some(k.clone()),
                    blocks: Some(set(&store, &t, k, &mut done, 0).into_iter().collect()),
                },
            )
        })
        .collect();
    Ok(UpdateBlockSets {
        r#type: super::INIT,
        block_sets: Some(list),
    })
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("Wood_*_Planks", "Wood_Oak_Planks"));
        assert!(glob("*", "x"));
        assert!(glob("Rock_?", "Rock_A"));
        assert!(!glob("Rock_?", "Rock_AB"));
        assert!(!glob("Wood_*_Planks", "Wood_Oak_Plank"));
    }
}
