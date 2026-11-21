//! Asset tags (spec 07, "Tags").
//!
//! An asset's `"Tags": {"Material": ["Solid", "Stone"], "Ore": []}` expands to
//! `Material`, `Solid`, `Material=Solid`, `Stone`, `Material=Stone`, `Ore`.
//! An asset with a parent first takes the parent's raw tags (each expanded
//! name, with its values), then adds its own. Packets send the expanded tags
//! as indexes in a table the server fills as names first appear, in the
//! iteration order of the fastutil `IntOpenHashSet` holding them.
//!
//! Orders that matter: the parent's raw tags are walked in the iteration
//! order of a `java.util.HashMap` created with capacity 0, the asset's own
//! `Tags` in that of a default `HashMap`; within an entry, the key, then
//! each value and `key=value`.

use serde_json::Value;

use crate::chm::{hashmap_order, hashmap_order_from, int_set_order};
use crate::ctx::Ctx;
use crate::store::Store;

/// Tags of one asset: raw entries in insertion order, and the expanded names
/// in the order they were added.
#[derive(Clone, Default, Debug)]
pub struct Tags {
    pub raw: Vec<(String, Vec<String>)>,
    pub expanded: Vec<String>,
}

impl Tags {
    fn ensure(&mut self, name: &str) {
        if !self.expanded.iter().any(|n| n == name) {
            self.expanded.push(name.to_owned());
        }
    }

    fn put_if_absent(&mut self, name: &str) {
        if !self.raw.iter().any(|(k, _)| k == name) {
            self.raw.push((name.to_owned(), Vec::new()));
        }
    }

    /// `putTags`: entries in the given order.
    pub fn put(&mut self, entries: &[(String, Vec<String>)]) {
        for (tag, values) in entries {
            match self.raw.iter_mut().find(|(k, _)| k == tag) {
                Some((_, v)) => v.extend(values.iter().cloned()),
                None => self.raw.push((tag.clone(), values.clone())),
            }
            self.ensure(tag);
            for v in values {
                self.ensure(v);
                self.put_if_absent(v);
                let kv = format!("{tag}={v}");
                self.put_if_absent(&kv);
                self.ensure(&kv);
            }
        }
    }

    /// The raw entries in the iteration order of the `HashMap` holding them.
    pub fn raw_in_map_order(&self) -> Vec<(String, Vec<String>)> {
        let keys: Vec<String> = self.raw.iter().map(|(k, _)| k.clone()).collect();
        hashmap_order_from(&keys, 1)
            .into_iter()
            .filter_map(|k| self.raw.iter().find(|(r, _)| *r == k).cloned())
            .collect()
    }
}

/// The entries of a `"Tags"` object, in the iteration order of a default
/// `HashMap`.
pub fn own_entries(v: &Value) -> Vec<(String, Vec<String>)> {
    let Some(m) = v.get("Tags").and_then(Value::as_object) else {
        return Vec::new();
    };
    let keys: Vec<String> = m.keys().cloned().collect();
    hashmap_order(&keys)
        .into_iter()
        .map(|k| {
            let vals = m[&k]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (k, vals)
        })
        .collect()
}

/// An asset's tags, down its parent chain.
pub fn of(store: &Store, key: &str) -> Tags {
    fn go(store: &Store, key: &str, depth: usize) -> Tags {
        let mut t = Tags::default();
        if depth < 32 {
            if let Some(p) = store
                .parents
                .get(key)
                .filter(|p| store.own.contains_key(*p))
            {
                let parent = go(store, p, depth + 1);
                t.put(&parent.raw_in_map_order());
            }
        }
        if let Some(v) = store.own.get(key) {
            t.put(&own_entries(v));
        }
        t
    }
    go(store, key, 0)
}

/// The tag indexes a packet sends for an asset, in the official order.
pub fn indexes(ctx: &Ctx, store: &Store, key: &str) -> Vec<i32> {
    let t = of(store, key);
    let idx: Vec<i32> = t.expanded.iter().map(|n| ctx.tag(n)).collect();
    int_set_order(&idx, 0)
}

/// Collects captured tag lists (compare runs) for `solve`: each observation
/// pairs an asset's expanded tag names with the set of indexes the official
/// server sent for it. A name's index is in every set of an asset that has
/// the name and in no set of one that lacks it; names left with one
/// candidate are pinned, repeatedly.
pub fn learn(ctx: &Ctx, observations: &[(Vec<String>, Vec<i32>)]) {
    ctx.tag_observations
        .lock()
        .unwrap()
        .extend(observations.iter().cloned());
}

/// Solves every observation `learn` collected (call once, after all of
/// them, so that each packet's lists help the others).
pub fn solve(ctx: &Ctx) {
    let observations = ctx.tag_observations.lock().unwrap().clone();
    let observations = observations.as_slice();
    use std::collections::{BTreeSet, HashMap, HashSet};
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for (n, _) in observations {
        names.extend(n.iter().map(String::as_str));
    }
    let all: BTreeSet<i32> = observations
        .iter()
        .flat_map(|(_, i)| i.iter().copied())
        .collect();
    let mut cand: HashMap<&str, BTreeSet<i32>> = HashMap::new();
    for name in &names {
        let mut c = all.clone();
        for (n, idx) in observations {
            let set: HashSet<i32> = idx.iter().copied().collect();
            if n.iter().any(|x| x == name) {
                c.retain(|i| set.contains(i));
            } else {
                c.retain(|i| !set.contains(i));
            }
        }
        cand.insert(name, c);
    }
    if let Ok(w) = std::env::var("AERIE_TAG_DEBUG") {
        for (n, c) in &cand {
            if w.split(',').any(|x| x == *n) {
                eprintln!("tag {n}: candidates {c:?}");
            }
        }
    }
    // Names already pinned (by other packets) are known.
    let pinned = ctx.pinned_tags();
    let mut done: HashMap<&str, i32> = HashMap::new();
    for name in &names {
        if let Some(i) = pinned.get(*name) {
            done.insert(name, *i);
        }
    }
    let taken: HashSet<i32> = pinned.values().copied().collect();
    for (n, c) in cand.iter_mut() {
        match done.get(n) {
            Some(i) => *c = BTreeSet::from([*i]),
            None => c.retain(|i| !taken.contains(i)),
        }
    }
    // The joint search is off: two of its parts have no fitting pairing
    // (some insertion order is still wrong) and it changed no result.
    let mut tried_joint = !JOINT;
    loop {
        let mut fixed: Vec<(&str, i32)> = cand
            .iter()
            .filter(|(n, c)| c.len() == 1 && !done.contains_key(*n))
            .map(|(n, c)| (*n, *c.iter().next().unwrap()))
            .collect();
        if fixed.is_empty() {
            // Names that always appear together cannot be told apart here;
            // any pairing gives the same packets, so pair them in order.
            let mut groups: HashMap<Vec<i32>, Vec<&str>> = HashMap::new();
            for (n, c) in &cand {
                if !done.contains_key(n) && c.len() > 1 {
                    groups
                        .entry(c.iter().copied().collect())
                        .or_default()
                        .push(n);
                }
            }
            // A group the captured orders decide goes first; with none,
            // the first group is paired in order.
            let mut ties: Vec<(Vec<i32>, Vec<&str>)> = groups
                .into_iter()
                .filter(|(i, ns)| i.len() == ns.len())
                .collect();
            ties.sort();
            for (_, ns) in ties.iter_mut() {
                ns.sort();
            }
            // The joint search runs once: after it fails, groups are decided
            // one at a time.
            if !tried_joint {
                tried_joint = true;
                if let Some(f) = joint(&ties, &done, observations) {
                    fixed = f;
                }
            }
            for (idx, ns) in ties.iter_mut() {
                if !fixed.is_empty() {
                    break;
                }
                if let Some(f) = decide(ns, idx, &done, observations) {
                    fixed = f;
                    break;
                }
            }
            if fixed.is_empty() {
                if let Some((idx, ns)) = ties.first() {
                    fixed = tie_break(ns, idx, &done, observations);
                }
            }
        }
        if fixed.is_empty() {
            break;
        }
        for (n, i) in fixed {
            done.insert(n, i);
            for (m, c) in cand.iter_mut() {
                if *m != n {
                    c.remove(&i);
                } else {
                    *c = BTreeSet::from([i]);
                }
            }
        }
    }
    for (n, i) in done {
        if std::env::var("AERIE_TAG_DEBUG").is_ok_and(|w| w.split(',').any(|x| x == n)) {
            eprintln!("tag {n} -> {i}");
        }
        ctx.pin_tag(n, i);
    }
}

/// Whether `solve` tries `joint` first (see there).
const JOINT: bool = false;

/// Pairs every group of names that always appear together with its
/// indexes at once: a depth-first search over the groups' pairings that
/// checks each captured list as soon as all its names have an index (one
/// group alone often cannot be checked, when its lists also hold another
/// undecided group). Groups that share no list are searched apart; a part
/// with no fitting pairing (or too long a search) is left to the one-group
/// steps. `None` when no part is solved.
fn joint<'a>(
    ties: &[(Vec<i32>, Vec<&'a str>)],
    done: &std::collections::HashMap<&str, i32>,
    observations: &[(Vec<String>, Vec<i32>)],
) -> Option<Vec<(&'a str, i32)>> {
    use std::collections::{HashMap, VecDeque};
    if ties.is_empty() {
        return None;
    }
    fn perms(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for p in perms(n - 1) {
            for i in 0..=p.len() {
                let mut q = p.clone();
                q.insert(i, n - 1);
                out.push(q);
            }
        }
        out
    }
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    for (g, (_, ns)) in ties.iter().enumerate() {
        for n in ns {
            group_of.insert(n, g);
        }
    }
    // The groups each checkable list needs (lists with a name that is
    // neither known nor in a group can never be checked).
    let mut lists: Vec<(usize, Vec<usize>)> = Vec::new();
    for (o, (names, _)) in observations.iter().enumerate() {
        let mut gs: Vec<usize> = Vec::new();
        let mut ok = true;
        for n in names {
            if done.contains_key(n.as_str()) {
                continue;
            }
            match group_of.get(n.as_str()) {
                Some(g) => {
                    if !gs.contains(g) {
                        gs.push(*g);
                    }
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && !gs.is_empty() {
            lists.push((o, gs));
        }
    }
    // Groups linked by lists form independent components.
    let mut links: Vec<Vec<usize>> = vec![Vec::new(); ties.len()];
    for (_, gs) in &lists {
        for &a in gs {
            for &b in gs {
                if a != b && !links[a].contains(&b) {
                    links[a].push(b);
                }
            }
        }
    }
    let mut seen = vec![false; ties.len()];
    let mut value: HashMap<&str, i32> = done.iter().map(|(k, v)| (*k, *v)).collect();
    let mut out = Vec::new();
    let (mut solved, mut failed) = (0, 0);
    for start in 0..ties.len() {
        if seen[start] {
            continue;
        }
        // Breadth-first order, so each list is checked soon.
        let mut order = Vec::new();
        let mut q = VecDeque::from([start]);
        seen[start] = true;
        while let Some(g) = q.pop_front() {
            order.push(g);
            for &h in &links[g] {
                if !seen[h] {
                    seen[h] = true;
                    q.push_back(h);
                }
            }
        }
        if order.iter().any(|&g| ties[g].1.len() > 6) {
            failed += 1;
            continue;
        }
        let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(i, g)| (*g, i)).collect();
        let mut checks: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
        for (o, gs) in &lists {
            if let Some(last) = gs.iter().filter_map(|g| pos.get(g)).max() {
                if gs.iter().all(|g| pos.contains_key(g)) {
                    checks[*last].push(*o);
                }
            }
        }
        let comp: Vec<&(Vec<i32>, Vec<&'a str>)> = order.iter().map(|&g| &ties[g]).collect();
        let all_perms: Vec<Vec<Vec<usize>>> = comp.iter().map(|(_, ns)| perms(ns.len())).collect();
        let mut budget: usize = 50_000;
        #[allow(clippy::too_many_arguments)]
        fn go<'a>(
            g: usize,
            comp: &[&(Vec<i32>, Vec<&'a str>)],
            all_perms: &[Vec<Vec<usize>>],
            checks: &[Vec<usize>],
            observations: &[(Vec<String>, Vec<i32>)],
            value: &mut HashMap<&'a str, i32>,
            budget: &mut usize,
        ) -> bool {
            if g == comp.len() {
                return true;
            }
            let (idx, ns) = comp[g];
            for p in &all_perms[g] {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                for (k, &i) in p.iter().enumerate() {
                    value.insert(ns[k], idx[i]);
                }
                let ok = checks[g].iter().all(|&o| {
                    let (names, cap) = &observations[o];
                    let v: Vec<i32> = names.iter().map(|n| value[n.as_str()]).collect();
                    int_set_order(&v, 0) == *cap
                });
                if ok && go(g + 1, comp, all_perms, checks, observations, value, budget) {
                    return true;
                }
            }
            for n in ns {
                value.remove(n);
            }
            false
        }
        if go(
            0,
            &comp,
            &all_perms,
            &checks,
            observations,
            &mut value,
            &mut budget,
        ) {
            solved += 1;
            for (_, ns) in &comp {
                for n in ns {
                    out.push((*n, value[n]));
                }
            }
        } else {
            failed += 1;
        }
    }
    if std::env::var_os("AERIE_TAG_DEBUG").is_some() {
        eprintln!(
            "joint: {} groups, {solved} components solved, {failed} not",
            ties.len()
        );
    }
    (!out.is_empty()).then_some(out)
}

/// Pairs names that always appear together with their indexes. Pairings
/// give the same sets, but the order of a captured list can still tell them
/// apart (colliding slots keep insertion order): the first pairing that
/// reproduces every captured list wins; otherwise names and indexes go in
/// sorted order.
fn tie_break<'a>(
    names: &[&'a str],
    idx: &[i32],
    done: &std::collections::HashMap<&str, i32>,
    observations: &[(Vec<String>, Vec<i32>)],
) -> Vec<(&'a str, i32)> {
    fn perms(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for p in perms(n - 1) {
            for i in 0..=p.len() {
                let mut q = p.clone();
                q.insert(i, n - 1);
                out.push(q);
            }
        }
        out
    }
    let default: Vec<(&str, i32)> = names.iter().copied().zip(idx.iter().copied()).collect();
    if names.len() > 6 {
        return default;
    }
    let relevant: Vec<&(Vec<String>, Vec<i32>)> = observations
        .iter()
        .filter(|(n, _)| n.iter().any(|x| names.contains(&x.as_str())))
        .collect();
    for p in perms(names.len()) {
        let pairing: Vec<(&str, i32)> = p
            .iter()
            .enumerate()
            .map(|(k, &i)| (names[k], idx[i]))
            .collect();
        let ok = relevant.iter().all(|(n, cap)| {
            let mut values = Vec::new();
            for x in n {
                let v = pairing
                    .iter()
                    .find(|(m, _)| *m == x)
                    .map(|(_, i)| *i)
                    .or_else(|| done.get(x.as_str()).copied());
                match v {
                    Some(v) => values.push(v),
                    None => return true,
                }
            }
            let good = int_set_order(&values, 0) == *cap;
            if !good
                && std::env::var("AERIE_TAG_DEBUG")
                    .is_ok_and(|w| names.iter().any(|x| w.split(',').any(|y| y == *x)))
            {
                eprintln!(
                    "tie {pairing:?}: {n:?} gives {:?}, captured {cap:?}",
                    int_set_order(&values, 0)
                );
            }
            good
        });
        if ok {
            return pairing;
        }
    }
    default
}

/// Like `tie_break`, but only answers when some captured list involving the
/// group is fully known under a pairing: the pairing that reproduces every
/// such list (and at least one), else `None`.
fn decide<'a>(
    names: &[&'a str],
    idx: &[i32],
    done: &std::collections::HashMap<&str, i32>,
    observations: &[(Vec<String>, Vec<i32>)],
) -> Option<Vec<(&'a str, i32)>> {
    if names.len() > 6 {
        return None;
    }
    fn perms(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for p in perms(n - 1) {
            for i in 0..=p.len() {
                let mut q = p.clone();
                q.insert(i, n - 1);
                out.push(q);
            }
        }
        out
    }
    let relevant: Vec<&(Vec<String>, Vec<i32>)> = observations
        .iter()
        .filter(|(n, _)| n.iter().any(|x| names.contains(&x.as_str())))
        .collect();
    let mut winners = Vec::new();
    for p in perms(names.len()) {
        let pairing: Vec<(&str, i32)> = p
            .iter()
            .enumerate()
            .map(|(k, &i)| (names[k], idx[i]))
            .collect();
        let (mut known, mut ok) = (0, true);
        for (n, cap) in &relevant {
            let values: Option<Vec<i32>> = n
                .iter()
                .map(|x| {
                    pairing
                        .iter()
                        .find(|(m, _)| *m == x)
                        .map(|(_, i)| *i)
                        .or_else(|| done.get(x.as_str()).copied())
                })
                .collect();
            if let Some(v) = values {
                known += 1;
                if int_set_order(&v, 0) != *cap {
                    ok = false;
                    break;
                }
            }
        }
        if ok && known > 0 {
            winners.push(pairing);
        }
    }
    (winners.len() == 1).then(|| winners.remove(0))
}
