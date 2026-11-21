//! The asset formats (spec 07, "Asset schema"): `assets.json` (written by
//! `tools/gen.py` into `gen/`), a
//! condensed copy of the JSON schemas the official server writes for every
//! asset type (`tools/asset_schema_extract.py`).
//!
//! Each asset type has a root schema `<Type>.json` whose `hytale` object names
//! its folder (`path`) and file extension; shared definitions live in
//! `common.json` and `other.json` and are reached through `$ref`.

use std::sync::OnceLock;

use serde_json::{Map, Value};

static TEXT: &str = include_str!(concat!(env!("OUT_DIR"), "/assets.json"));

pub struct Schema {
    files: Map<String, Value>,
}

/// One asset type: its name, folder under `Server/` and file extension.
pub struct AssetType<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub ext: &'a str,
}

impl AssetType<'_> {
    /// The `Ctx::store` path of this type's folder.
    pub fn store_path(&self) -> String {
        if self.ext == ".json" {
            self.path.to_owned()
        } else {
            format!("{}|{}", self.path, self.ext)
        }
    }
}

pub fn get() -> &'static Schema {
    static S: OnceLock<Schema> = OnceLock::new();
    S.get_or_init(|| {
        let v: Value = serde_json::from_str(TEXT).expect("assets.json");
        let files = match v {
            Value::Object(mut m) => match m.remove("files") {
                Some(Value::Object(f)) => f,
                _ => Map::new(),
            },
            _ => Map::new(),
        };
        Schema { files }
    })
}

impl Schema {
    /// Every asset type with a folder.
    pub fn asset_types(&self) -> Vec<AssetType<'_>> {
        self.files
            .iter()
            .filter_map(|(file, s)| {
                let h = s.get("hytale")?;
                Some(AssetType {
                    name: file.strip_suffix(".json")?,
                    path: h.get("path")?.as_str()?,
                    ext: h.get("extension")?.as_str()?,
                })
            })
            .collect()
    }

    pub fn asset_type(&self, name: &str) -> Option<AssetType<'_>> {
        self.asset_types().into_iter().find(|t| t.name == name)
    }

    /// The root schema of an asset type, with the file it lives in.
    pub fn root<'a>(&'a self, name: &str) -> Option<(&'a str, &'a Value)> {
        let (k, v) = self.files.get_key_value(&format!("{name}.json"))?;
        Some((k.as_str(), v))
    }

    /// Follows `$ref` until a node without one. `file` is the schema file the
    /// node came from (references without a file name are local to it).
    pub fn deref<'a>(&'a self, mut file: &'a str, mut node: &'a Value) -> (&'a str, &'a Value) {
        for _ in 0..32 {
            let Some(r) = node.get("$ref").and_then(Value::as_str) else {
                break;
            };
            let (f, ptr) = r.split_once('#').unwrap_or((r, ""));
            let Some((fk, fv)) = (if f.is_empty() {
                self.files.get_key_value(file)
            } else {
                self.files.get_key_value(f)
            }) else {
                break;
            };
            let Some(target) = (if ptr.is_empty() {
                Some(fv)
            } else {
                fv.pointer(ptr)
            }) else {
                break;
            };
            file = fk.as_str();
            node = target;
        }
        (file, node)
    }

    /// The asset type a `$ref` names when it points at a type's root schema
    /// (`"Interaction.json#"`).
    pub fn root_ref(node: &Value) -> Option<&str> {
        let r = node.get("$ref")?.as_str()?;
        r.strip_suffix(".json#")
    }
}

/// Whether a field inherits from the parent asset (`hytale.inheritsProperty`
/// on the field or on one of its `anyOf` alternatives).
fn inherits(node: &Value) -> bool {
    let flag = |n: &Value| {
        n.pointer("/hytale/inheritsProperty")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    flag(node)
        || node
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(flag))
}

impl Schema {
    /// The object schema (with `properties`) a node describes for `v`, if
    /// any: through `$ref`, nullable `anyOf`s and polymorphic variants
    /// (picked by `v`'s type field, else `fallback`'s, else the default).
    fn object_node<'a>(
        &'a self,
        file: &'a str,
        node: &'a Value,
        v: &Value,
        fallback: Option<&Value>,
    ) -> Option<(&'a str, &'a Value)> {
        let (file, node) = self.deref(file, node);
        if node.get("properties").is_some() {
            return Some((file, node));
        }
        let alts = node.get("anyOf")?.as_array()?;
        if let Some(tf) = node.get("hytaleSchemaTypeField") {
            let prop = tf.get("property")?.as_str()?;
            let t = v
                .get(prop)
                .or_else(|| fallback.and_then(|f| f.get(prop)))
                .and_then(Value::as_str)
                .or_else(|| tf.get("defaultValue").and_then(Value::as_str))?;
            let i = tf
                .get("values")?
                .as_array()?
                .iter()
                .position(|x| x.as_str() == Some(t))?;
            return self.object_node(file, alts.get(i)?, v, fallback);
        }
        alts.iter()
            .find_map(|a| self.object_node(file, a, v, fallback))
    }

    /// One inheritance step (spec 07, "Inheritance"): `own` as written over
    /// `parent` already resolved. A field the asset sets keeps its value,
    /// except that an object field whose schema has inheriting fields takes
    /// those from the parent's object when it does not set them; a field it
    /// does not set comes from the parent only if it inherits.
    pub fn inherit_step(
        &self,
        file: &str,
        node: &Value,
        own: &Value,
        parent: Option<&Value>,
    ) -> Value {
        let (file_outer, node_outer) = (file, node);
        let (Some(o), Some((file, node))) =
            (own.as_object(), self.object_node(file, node, own, parent))
        else {
            return own.clone();
        };
        let props = node.get("properties").and_then(Value::as_object);
        let mut out = serde_json::Map::new();
        // A nested polymorphic object keeps its parent's type when it names none.
        let (_, outer) = self.deref(file_outer, node_outer);
        if let (Some(tf), Some(p)) = (
            outer
                .pointer("/hytaleSchemaTypeField/property")
                .and_then(Value::as_str),
            parent,
        ) {
            if !o.contains_key(tf) {
                if let Some(t) = p.get(tf) {
                    out.insert(tf.to_owned(), t.clone());
                }
            }
        }
        if let Some(props) = props {
            for (k, sub) in props {
                let p = parent.and_then(|p| p.get(k));
                match (o.get(k), p) {
                    (Some(c), Some(p)) if inherits(sub) && c.is_object() && p.is_object() => {
                        out.insert(k.clone(), self.inherit_step(file, sub, c, Some(p)));
                    }
                    (Some(c), _) => {
                        out.insert(k.clone(), c.clone());
                    }
                    (None, Some(p)) if inherits(sub) => {
                        out.insert(k.clone(), p.clone());
                    }
                    _ => {}
                }
            }
        }
        for (k, v) in o {
            if !out.contains_key(k) && !props.is_some_and(|p| p.contains_key(k)) {
                out.insert(k.clone(), v.clone());
            }
        }
        Value::Object(out)
    }

    /// An asset of type `class` resolved down its parent chain (`chain`: the
    /// assets as written, oldest ancestor first).
    pub fn inherit_chain(&self, class: &str, chain: &[&Value]) -> Value {
        let Some((file, root)) = self.root(class) else {
            return chain.last().map(|v| (*v).clone()).unwrap_or(Value::Null);
        };
        // A polymorphic type's discriminating field (`Type`) passes down the
        // chain: a child that names only a parent is of the parent's type.
        let type_field = self
            .deref(file, root)
            .1
            .pointer("/hytaleSchemaTypeField/property")
            .and_then(Value::as_str);
        let mut acc: Option<Value> = None;
        for own in chain {
            let mut own = (*own).clone();
            if let (Some(tf), Some(prev), Value::Object(m)) = (type_field, &acc, &mut own) {
                if !m.contains_key(tf) {
                    if let Some(t) = prev.get(tf) {
                        m.insert(tf.to_owned(), t.clone());
                    }
                }
            }
            acc = Some(self.inherit_step(file, root, &own, acc.as_ref()));
        }
        acc.unwrap_or(Value::Null)
    }
}

impl Schema {
    /// The schema default of a top-level field of a (non-polymorphic) asset
    /// type, through nullable `anyOf`s.
    pub fn field_default(&self, class: &str, field: &str) -> Option<&Value> {
        let (file, root) = self.root(class)?;
        let (_, root) = self.deref(file, root);
        let node = root.get("properties")?.get(field)?;
        fn find(n: &Value) -> Option<&Value> {
            if let Some(d) = n.get("default") {
                return Some(d);
            }
            n.get("anyOf")?.as_array()?.iter().find_map(find)
        }
        find(node)
    }
}
