//! Contained assets (spec 07, "Contained assets"): assets written inline,
//! inside another asset, where a key would normally go.
//!
//! A field that names an asset of another (or the same) type may hold the
//! asset itself instead of its key. The asset schema marks those fields: an
//! `anyOf` of a key string and the type's root schema (`"X.json#"`), with
//! `hytaleAssetRef: "X"`. Such an inline asset joins type X's assets under a
//! generated key:
//!
//! - usually `*` + owner key + `_` + the field path joined by `_` (field names,
//!   map keys and array indexes), so a contained asset inside a contained asset
//!   has one more `*`: `**Fire_Interactions_Collision_Interactions_0`;
//! - an item's `BlockType` takes the item's own key, and the block's `Parent`
//!   falls back to the item's parent (not for a contained state item);
//! - item states (`State.<name>`) and block states (`State.Definitions.<name>`)
//!   use the generated key and inherit from their owner when they name no
//!   `Parent` themselves.
//!
//! Contained assets are found by walking every asset of every type against
//! its schema, then walking each contained asset the same way.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::schema::{self, Schema};

/// An asset found inside another one.
#[derive(Clone, Debug)]
pub struct Contained {
    /// Asset type (schema name, e.g. `RootInteraction`).
    pub class: String,
    pub key: String,
    pub parent: Option<String>,
    /// The asset's JSON as written (it may still name a `Parent`).
    pub value: Value,
    /// Pack path of the file the outermost owner came from.
    pub file: String,
    /// The asset it was written in: type and key.
    pub owner_class: String,
    pub owner: String,
    /// Type of the outermost owner (the file asset).
    pub root_class: String,
}

/// The assets of one type that contained assets can inherit from or be
/// looked up in, by lower-cased key.
pub type Known = HashMap<String, HashMap<String, Value>>;

struct Owner<'a> {
    class: &'a str,
    key: &'a str,
    parent: Option<&'a str>,
}

struct Walker<'a> {
    schema: &'static Schema,
    known: &'a mut Known,
    found: Vec<Contained>,
    seen: HashSet<(String, String)>,
    file: String,
    root_class: String,
    /// Assets named by key in fields that may hold one: (type, key).
    named: Vec<(String, String)>,
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl Walker<'_> {
    /// Whether a schema node can describe `v` (by JSON type only).
    fn fits(&self, file: &str, node: &Value, v: &Value, depth: usize) -> bool {
        let (file, node) = self.schema.deref(file, node);
        let t = json_type(v);
        if let Some(ty) = node.get("type") {
            let ok = |s: &str| s == t || (s == "integer" && t == "number");
            return match ty {
                Value::String(s) => ok(s),
                Value::Array(a) => a.iter().filter_map(Value::as_str).any(ok),
                _ => true,
            };
        }
        if node.get("properties").is_some() || node.get("additionalProperties").is_some() {
            return t == "object";
        }
        if depth < 8 {
            if let Some(alts) = node
                .get("anyOf")
                .or(node.get("oneOf"))
                .and_then(Value::as_array)
            {
                return alts.iter().any(|a| self.fits(file, a, v, depth + 1));
            }
        }
        true
    }

    /// The `Type` of an asset that names a parent but not its own type: the
    /// nearest ancestor's.
    fn type_from_parent(&self, class: &str, prop: &str, v: &Value) -> Option<String> {
        let mut cur = v.clone();
        for _ in 0..16 {
            if let Some(t) = cur.get(prop).and_then(Value::as_str) {
                return Some(t.to_owned());
            }
            let p = cur.get("Parent")?.as_str()?.to_lowercase();
            cur = self.known.get(class)?.get(&p)?.clone();
        }
        None
    }

    fn walk(
        &mut self,
        file: &'static str,
        node: &'static Value,
        v: &Value,
        path: &mut Vec<String>,
        owner: &Owner,
    ) {
        let (file, node) = self.schema.deref(file, node);

        // A field that may hold a contained asset.
        if let Some(class) = node.get("hytaleAssetRef").and_then(Value::as_str) {
            let inline = node
                .get("anyOf")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|x| Schema::root_ref(x) == Some(class)));
            if inline {
                if v.is_object() {
                    self.contain(class, v, path, owner);
                } else if let Some(k) = v.as_str() {
                    self.named.push((class.to_owned(), k.to_owned()));
                }
                return;
            }
        }

        // (Objects with properties may also carry a `oneOf` of `required`
        // clauses; that is validation, not alternatives.)
        let alts = node
            .get("anyOf")
            .or(node.get("oneOf"))
            .filter(|_| node.get("properties").is_none())
            .and_then(Value::as_array);
        if let Some(alts) = alts {
            if let Some(tf) = node.get("hytaleSchemaTypeField") {
                let prop = tf.get("property").and_then(Value::as_str).unwrap_or("Type");
                let values: Vec<&str> = tf
                    .get("values")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let parent_class = alts.iter().find_map(|a| {
                    let (_, a) = self.schema.deref(file, a);
                    a.pointer("/properties/Parent/hytaleParent/type")?.as_str()
                });
                let t = v
                    .get(prop)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| parent_class.and_then(|c| self.type_from_parent(c, prop, v)))
                    .or_else(|| {
                        tf.get("defaultValue")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                match t.and_then(|t| values.iter().position(|x| *x == t)) {
                    Some(i) if i < alts.len() => self.walk(file, &alts[i], v, path, owner),
                    // Unknown type: every variant may apply; the same contained
                    // asset found twice is kept once.
                    _ => {
                        for a in alts {
                            if self.fits(file, a, v, 0) {
                                self.walk(file, a, v, path, owner);
                            }
                        }
                    }
                }
                return;
            }
            if let Some(a) = alts.iter().find(|a| self.fits(file, a, v, 0)) {
                self.walk(file, a, v, path, owner);
            }
            return;
        }

        match v {
            Value::Object(m) => {
                // Declared fields in declaration order (the official decoder
                // goes through them in that order), then map entries as written.
                let props = node.get("properties").and_then(Value::as_object);
                let extra = node.get("additionalProperties").filter(|x| x.is_object());
                let mut fields: Vec<(&String, &Value, &'static Value)> = Vec::new();
                if let Some(p) = props {
                    for (k, sub) in p {
                        if let Some(x) = m.get(k) {
                            fields.push((k, x, sub));
                        }
                    }
                }
                if let Some(extra) = extra {
                    for (k, x) in m {
                        if !props.is_some_and(|p| p.contains_key(k)) {
                            fields.push((k, x, extra));
                        }
                    }
                }
                for (k, x, sub) in fields {
                    path.push(k.clone());
                    self.walk(file, sub, x, path, owner);
                    path.pop();
                }
            }
            Value::Array(a) => {
                if let Some(items) = node.get("items").filter(|x| x.is_object()) {
                    for (i, x) in a.iter().enumerate() {
                        path.push(i.to_string());
                        self.walk(file, items, x, path, owner);
                        path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    /// NPC roles are read by their own builders, not by the asset schema.
    /// Their contained assets are the root interactions of `InteractionVars`
    /// maps, anywhere in the role, keyed by the field names on the way (array
    /// indexes are not part of the path), and of `Modify._InteractionVars`,
    /// keyed by the entry name alone; and inline `CombatConfig` balance assets.
    fn npc_role(&mut self, v: &Value, path: &mut Vec<String>, owner: &Owner) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    let vars = match (k.as_str(), path.as_slice()) {
                        ("InteractionVars", _) => Some(false),
                        ("_InteractionVars", [m]) if m == "Modify" => Some(true),
                        _ => None,
                    };
                    if k == "CombatConfig" && x.is_object() {
                        let mut p = path.clone();
                        p.push(k.clone());
                        self.contain("BalanceAsset", x, &p, owner);
                        continue;
                    }
                    match (vars, x.as_object()) {
                        (Some(bare), Some(entries)) => {
                            for (name, e) in entries {
                                if e.is_object() {
                                    let mut p = if bare { Vec::new() } else { path.clone() };
                                    if !bare {
                                        p.push(k.clone());
                                    }
                                    p.push(name.clone());
                                    self.contain("RootInteraction", e, &p, owner);
                                }
                            }
                        }
                        _ => {
                            path.push(k.clone());
                            self.npc_role(x, path, owner);
                            path.pop();
                        }
                    }
                }
            }
            Value::Array(a) => {
                for x in a {
                    self.npc_role(x, path, owner);
                }
            }
            _ => {}
        }
    }

    fn contain(&mut self, class: &str, v: &Value, path: &[String], owner: &Owner) {
        let own_parent = v.get("Parent").and_then(Value::as_str).map(str::to_owned);
        let generated = format!("*{}_{}", owner.key, path.join("_"));
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        let (key, parent) = match (owner.class, class, p.as_slice()) {
            // An item's block: the item's key; the parent falls back to the
            // item's, except for a contained (state) item, whose parent is
            // injected and not recorded for its block.
            ("Item", "BlockType", ["BlockType"]) => (
                owner.key.to_owned(),
                own_parent.or_else(|| {
                    (!owner.key.starts_with('*'))
                        .then(|| owner.parent.map(str::to_owned))
                        .flatten()
                }),
            ),
            // Item and block states inherit from their owner.
            ("Item", "Item", ["State", _])
            | ("BlockType", "BlockType", ["State", "Definitions", _]) => {
                let parent =
                    own_parent.or_else(|| (owner.key != generated).then(|| owner.key.to_owned()));
                (generated, parent)
            }
            _ => (generated, own_parent),
        };
        if !self.seen.insert((class.to_owned(), key.clone())) {
            return;
        }
        self.known
            .entry(class.to_owned())
            .or_default()
            .insert(key.to_lowercase(), v.clone());
        self.found.push(Contained {
            class: class.to_owned(),
            key,
            parent,
            value: v.clone(),
            file: self.file.clone(),
            owner_class: owner.class.to_owned(),
            owner: owner.key.to_owned(),
            root_class: self.root_class.clone(),
        });
    }
}

/// Finds every contained asset. `files` holds each asset type's file assets
/// (type name → key → JSON), in the order they should be walked.
/// Each file asset is (key, parent, JSON, pack path).
pub type FileAssets = Vec<(String, Vec<(String, Option<String>, Value, String)>)>;

pub fn discover(files: &FileAssets) -> Vec<Contained> {
    discover_named(files).0
}

/// Like `discover`, also returning the assets named by key in fields that may
/// hold one, as (type, key).
pub fn discover_named(files: &FileAssets) -> (Vec<Contained>, Vec<(String, String)>) {
    let schema = schema::get();
    let mut known: Known = HashMap::new();
    for (class, assets) in files {
        let m = known.entry(class.clone()).or_default();
        for (k, _, v, _) in assets {
            m.insert(k.to_lowercase(), v.clone());
        }
    }
    let mut w = Walker {
        schema,
        known: &mut known,
        found: Vec::new(),
        seen: HashSet::new(),
        file: String::new(),
        root_class: String::new(),
        named: Vec::new(),
    };
    // Breadth first: the file assets, then what they contain, and so on.
    let mut queue: VecDeque<(String, String, Option<String>, Value, String, String)> =
        VecDeque::new();
    for (class, assets) in files {
        for (k, p, v, f) in assets {
            queue.push_back((
                class.clone(),
                k.clone(),
                p.clone(),
                v.clone(),
                f.clone(),
                class.clone(),
            ));
        }
    }
    while let Some((class, key, parent, v, file, root)) = queue.pop_front() {
        w.file = file;
        w.root_class = root;
        let Some((file, root)) = schema.root(&class) else {
            continue;
        };
        let before = w.found.len();
        let owner = Owner {
            class: &class,
            key: &key,
            parent: parent.as_deref(),
        };
        if class == "NPCRole" {
            w.npc_role(&v, &mut Vec::new(), &owner);
        } else {
            w.walk(file, root, &v, &mut Vec::new(), &owner);
        }
        for c in &w.found[before..] {
            queue.push_back((
                c.class.clone(),
                c.key.clone(),
                c.parent.clone(),
                c.value.clone(),
                c.file.clone(),
                c.root_class.clone(),
            ));
        }
    }
    (w.found, w.named)
}

/// Every contained asset of an installation, in the order found.
pub fn discover_ctx(ctx: &crate::ctx::Ctx) -> crate::Result<Vec<Contained>> {
    Ok(ctx.contained()?.values().flatten().cloned().collect())
}

/// Music containers the official server makes for ambience effects still
/// written the old way (spec 07): an effect with `Music {Tracks, Volume}`
/// (inherited) and no `MusicContainer` gets a random container
/// `_legacy_<effect>` (shuffle, loop count 0, avoid repeat 1, resume memory
/// 600 s, the effect's audio category) over single tracks
/// `_legacy_<effect>_<i>`. They come after every other container.
pub fn legacy_music(ambience: &crate::store::Store) -> Vec<Contained> {
    let mut out = Vec::new();
    for k in ambience.own.keys() {
        let a = ambience.resolved(k, "AmbienceFX");
        if a.get("MusicContainer").is_some() {
            continue;
        }
        let Some(music) = a.get("Music").filter(|m| m.is_object()) else {
            continue;
        };
        let id = format!("_legacy_{k}");
        let tracks: Vec<&str> = music
            .get("Tracks")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let children: Vec<Value> = (0..tracks.len())
            .map(|i| Value::String(format!("{id}_{i}")))
            .collect();
        let mut c = serde_json::json!({
            "Type": "Random",
            "Volume": music.get("Volume").cloned().unwrap_or(0.into()),
            "LoopCount": 0,
            "Mode": "Shuffle",
            "AvoidRepeatCount": 1,
            "ResumeMemoryDuration": 600.0,
            "Children": children,
        });
        if let Some(cat) = a.get("AudioCategory") {
            c["AudioCategory"] = cat.clone();
        }
        let mk = |key: String, value: Value| Contained {
            class: "MusicContainer".into(),
            key,
            parent: None,
            value,
            file: ambience.files.get(k).cloned().unwrap_or_default(),
            owner_class: "AmbienceFX (legacy music)".into(),
            owner: k.clone(),
            root_class: "AmbienceFX (legacy music)".into(),
        };
        out.push(mk(id.clone(), c));
        for (i, t) in tracks.iter().enumerate() {
            out.push(mk(
                format!("{id}_{i}"),
                serde_json::json!({"Type": "SingleTrack", "Track": t}),
            ));
        }
    }
    out
}

/// Interactions the official server makes up when one is named but does not
/// exist (spec 07): an empty `Simple` interaction under that key, after every
/// other interaction. Their JSON is `{"$Unknown": true}`.
pub fn unknown_interactions(
    files: &FileAssets,
    found: &[Contained],
    named: &[(String, String)],
) -> Vec<Contained> {
    let mut known: HashSet<String> = HashSet::new();
    for (class, assets) in files {
        if class == "Interaction" {
            known.extend(assets.iter().map(|a| a.0.to_lowercase()));
        }
    }
    known.extend(
        found
            .iter()
            .filter(|c| c.class == "Interaction")
            .map(|c| c.key.to_lowercase()),
    );
    for b in [
        "*change_active_slot",
        "*pickupitem",
        "*simple_crafting_default",
        "*diagram_crafting_default",
        "*structural_crafting_default",
        "*usenpc",
    ] {
        known.insert(b.into());
    }
    let mut keys: Vec<&String> = named
        .iter()
        .filter(|(c, k)| c == "Interaction" && !known.contains(&k.to_lowercase()))
        .map(|(_, k)| k)
        .collect();
    keys.sort();
    keys.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    keys.into_iter()
        .map(|k| Contained {
            class: "Interaction".into(),
            key: k.clone(),
            parent: None,
            value: serde_json::json!({"$Unknown": true}),
            file: String::new(),
            owner_class: "(unknown)".into(),
            owner: k.clone(),
            root_class: "(unknown)".into(),
        })
        .collect()
}
