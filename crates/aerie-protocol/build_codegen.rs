//! Generates Rust types and codecs from gen/protocol.json (spec 06).
//! Every type gets a plain Rust form plus a `Wire` impl that follows the
//! layout rules of spec 02.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

use serde_json::Value;

const V: &str = "::std::vec::Vec";
const O: &str = "::std::option::Option";
const S: &str = "::std::string::String";
const B: &str = "::std::boxed::Box";
const R: &str = "crate::Result";

const KEYWORDS: &[&str] = &[
    "as", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "override",
    "priv", "pub", "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "async", "await", "abstract", "final", "macro", "typeof", "unsized",
    "virtual", "yield", "try", "gen",
];

fn snake(n: &str) -> String {
    let mut s = String::new();
    let chars: Vec<char> = n.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && (prev_lower || (next_lower && chars[i - 1].is_uppercase())) {
                s.push('_');
            }
            s.extend(c.to_lowercase());
        } else {
            s.push(*c);
        }
    }
    if KEYWORDS.contains(&s.as_str()) {
        format!("r#{s}")
    } else {
        s
    }
}

fn shout(n: &str) -> String {
    snake(n).trim_start_matches("r#").to_uppercase()
}

pub struct Gen {
    types: BTreeMap<String, Value>,
    boxed: HashSet<(String, String)>,
}

impl Gen {
    pub fn new(doc: &Value) -> Self {
        let types: BTreeMap<String, Value> = doc["types"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut g = Gen {
            types,
            boxed: HashSet::new(),
        };
        g.find_boxes();
        g
    }

    fn kind(&self, name: &str) -> &str {
        self.types
            .get(name)
            .and_then(|t| t["kind"].as_str())
            .unwrap_or("missing")
    }

    /// Struct types a struct holds directly (not through a list or map).
    fn direct(&self, name: &str) -> Vec<(String, String)> {
        let t = &self.types[name];
        let mut out = Vec::new();
        for f in t["fixed"].as_array().into_iter().flatten() {
            if let Some(s) = f["enc"]["struct"].as_str() {
                out.push((f["field"].as_str().unwrap().to_owned(), s.to_owned()));
            }
        }
        for f in t["var"].as_array().into_iter().flatten() {
            if let Some(r) = f["type"]["ref"].as_str() {
                if self.kind(r) == "struct" {
                    out.push((f["field"].as_str().unwrap().to_owned(), r.to_owned()));
                }
            }
        }
        out
    }

    fn reaches(&self, from: &str, to: &str, seen: &mut HashSet<String>) -> bool {
        if from == to {
            return true;
        }
        if !seen.insert(from.to_owned()) {
            return false;
        }
        self.direct(from)
            .iter()
            .any(|(_, t)| self.reaches(t, to, seen))
    }

    fn find_boxes(&mut self) {
        let mut boxed = HashSet::new();
        for (name, t) in &self.types {
            if t["kind"] != "struct" {
                continue;
            }
            for (field, target) in self.direct(name) {
                if self.reaches(&target, name, &mut HashSet::new()) {
                    boxed.insert((name.clone(), field));
                }
            }
        }
        self.boxed = boxed;
    }

    // ------------------------------------------------------------ types

    fn prim(p: &str) -> &'static str {
        match p {
            "bool" => "bool",
            "i8" => "i8",
            "i16" => "i16",
            "i32" => "i32",
            "i64" => "i64",
            "f32" => "f32",
            "f64" => "f64",
            _ => panic!("prim {p}"),
        }
    }

    fn helper_ty(h: &str) -> &'static str {
        match h {
            "uuid" => "::uuid::Uuid",
            "vec2f" => "[f32; 2]",
            "vec3f" => "[f32; 3]",
            "quatf" => "[f32; 4]",
            _ => panic!("helper {h}"),
        }
    }

    fn rty(&self, t: &Value) -> String {
        if t.get("string").is_some() {
            S.into()
        } else if let Some(p) = t["prim"].as_str() {
            Self::prim(p).into()
        } else if t.get("uuid").is_some() {
            "::uuid::Uuid".into()
        } else if let Some(h) = t["fixed_helper"].as_str() {
            Self::helper_ty(h).into()
        } else if t.get("array").is_some() {
            let e = self.rty(&t["array"]);
            if t["nullable_elements"].as_bool() == Some(true) {
                format!("{V}<{O}<{e}>>")
            } else {
                format!("{V}<{e}>")
            }
        } else if let Some(m) = t["map"].as_array() {
            format!("{V}<({}, {})>", self.rty(&m[0]), self.rty(&m[1]))
        } else if let Some(r) = t["ref"].as_str() {
            r.into()
        } else {
            panic!("type {t}")
        }
    }

    /// Closure `|b, p| -> Result<(T, usize)>` decoding `t`.
    fn dec(&self, t: &Value, enum32: bool) -> String {
        let head = "|b: &[u8], p: usize|";
        if t.get("string").is_some() {
            format!("{head} wire::get_string(b, p)")
        } else if let Some(p) = t["prim"].as_str() {
            format!("{head} wire::get::<{}>(b, p)", Self::prim(p))
        } else if t.get("uuid").is_some() || t["fixed_helper"] == "uuid" {
            format!("{head} wire::get_uuid(b, p)")
        } else if let Some(h) = t["fixed_helper"].as_str() {
            let n = match h {
                "vec2f" => 2,
                "vec3f" => 3,
                _ => 4,
            };
            format!("{head} wire::get_floats::<{n}>(b, p)")
        } else if t.get("array").is_some() {
            let e = self.dec(&t["array"], enum32);
            if t["nullable_elements"].as_bool() == Some(true) {
                format!("{head} wire::get_opt_vec(b, p, {e})")
            } else {
                format!("{head} wire::get_vec(b, p, {e})")
            }
        } else if let Some(m) = t["map"].as_array() {
            format!(
                "{head} wire::get_map(b, p, {}, {})",
                self.dec(&m[0], enum32),
                self.dec(&m[1], enum32)
            )
        } else if let Some(r) = t["ref"].as_str() {
            match self.kind(r) {
                "enum" if enum32 => {
                    format!("{head} wire::get::<i32>(b, p).map(|(v, p)| ({r}(v), p))")
                }
                "enum" => format!("{head} wire::get::<i8>(b, p).map(|(v, p)| ({r}(v as i32), p))"),
                _ => format!("{head} <{r} as Wire>::decode(b, p)"),
            }
        } else {
            panic!("dec {t}")
        }
    }

    /// Closure `|out, v: &T| -> Result<()>` encoding `t`.
    fn enc(&self, t: &Value, enum32: bool) -> String {
        let head = format!("|out: &mut {V}<u8>, v: &{}|", self.rty(t));
        if t.get("string").is_some() {
            format!("{head} wire::put_string(out, v)")
        } else if t.get("prim").is_some() {
            format!("{head} wire::put(out, *v)")
        } else if t.get("uuid").is_some() || t["fixed_helper"] == "uuid" {
            format!("{head} wire::put_uuid(out, v)")
        } else if t.get("fixed_helper").is_some() {
            format!("{head} wire::put_floats(out, v)")
        } else if t.get("array").is_some() {
            let e = self.enc(&t["array"], enum32);
            if t["nullable_elements"].as_bool() == Some(true) {
                format!("{head} wire::put_opt_vec(out, v, {e})")
            } else {
                format!("{head} wire::put_vec(out, v, {e})")
            }
        } else if let Some(m) = t["map"].as_array() {
            format!(
                "{head} wire::put_map(out, v, {}, {})",
                self.enc(&m[0], enum32),
                self.enc(&m[1], enum32)
            )
        } else if let Some(r) = t["ref"].as_str() {
            match self.kind(r) {
                "enum" if enum32 => format!("{head} wire::put(out, v.0)"),
                "enum" => format!("{head} wire::put(out, v.0 as i8)"),
                _ => format!("{head} v.encode(out)"),
            }
        } else {
            panic!("enc {t}")
        }
    }

    /// Rust type, decoder and encoder of a fixed-part field.
    fn fixed_codec(&self, enc: &Value) -> (String, String, String) {
        let head_d = "|b: &[u8], p: usize|";
        if let Some(p) = enc["prim"].as_str() {
            let t = Self::prim(p);
            (
                t.into(),
                format!("{head_d} wire::get::<{t}>(b, p)"),
                format!("|out: &mut {V}<u8>, v: &{t}| wire::put(out, *v)"),
            )
        } else if let Some(w) = enc["enum_width"].as_str() {
            let r = enc["ref"].as_str().unwrap();
            let v = Value::Object(serde_json::Map::from_iter([(
                "ref".to_string(),
                Value::String(r.into()),
            )]));
            (r.into(), self.dec(&v, w == "i32"), self.enc(&v, w == "i32"))
        } else if let Some(s) = enc["struct"].as_str() {
            (
                s.into(),
                format!("{head_d} <{s} as Wire>::decode(b, p)"),
                format!("|out: &mut {V}<u8>, v: &{s}| v.encode(out)"),
            )
        } else if let Some(h) = enc["fixed_helper"].as_str() {
            let v = Value::Object(serde_json::Map::from_iter([(
                "fixed_helper".to_string(),
                Value::String(h.into()),
            )]));
            (
                Self::helper_ty(h).into(),
                self.dec(&v, false),
                self.enc(&v, false),
            )
        } else if let Some(n) = enc["fixed_string"].as_u64() {
            (
                S.into(),
                format!("{head_d} wire::get_fixed_string(b, p, {n})"),
                format!("|out: &mut {V}<u8>, v: &{S}| wire::put_fixed_string(out, v, {n})"),
            )
        } else if let Some(p) = enc["fixed_prim_array"].as_str() {
            let t = Self::prim(p);
            let n = enc["count"].as_u64().unwrap();
            (
                format!("{V}<{t}>"),
                format!("{head_d} wire::get_fixed_scalars::<{t}>(b, p, {n})"),
                format!("|out: &mut {V}<u8>, v: &{V}<{t}>| wire::put_fixed_scalars(out, v, {n})"),
            )
        } else {
            panic!("fixed {enc}")
        }
    }

    // ------------------------------------------------------------ items

    fn gen_enum(&self, out: &mut String, name: &str, t: &Value) {
        writeln!(out, "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]\npub struct {name}(pub i32);").unwrap();
        writeln!(out, "impl {name} {{").unwrap();
        let mut used = HashSet::new();
        for v in t["values"].as_array().unwrap() {
            let (vn, n) = (v[0].as_str().unwrap(), v[1].as_i64().unwrap());
            let c = shout(vn);
            if used.insert(c.clone()) {
                writeln!(out, "    pub const {c}: {name} = {name}({n});").unwrap();
            }
        }
        // Assets name enum values the way the protocol does.
        writeln!(
            out,
            "    pub fn from_name(s: &str) -> Option<Self> {{\n        match s {{"
        )
        .unwrap();
        for v in t["values"].as_array().unwrap() {
            writeln!(
                out,
                "            {:?} => Some({name}({})),",
                v[0].as_str().unwrap(),
                v[1].as_i64().unwrap()
            )
            .unwrap();
        }
        writeln!(out, "            _ => None,\n        }}\n    }}").unwrap();
        writeln!(out, "}}").unwrap();
    }

    fn gen_poly(&self, out: &mut String, name: &str, t: &Value) {
        let vars: Vec<(i64, String)> = t["variants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (v[0].as_i64().unwrap(), v[1].as_str().unwrap().to_owned()))
            .collect();
        writeln!(
            out,
            "#[derive(Clone, Debug, PartialEq)]\npub enum {name} {{"
        )
        .unwrap();
        for (_, s) in &vars {
            writeln!(out, "    {s}({B}<{s}>),").unwrap();
        }
        writeln!(out, "}}").unwrap();
        writeln!(out, "impl Default for {name} {{ fn default() -> Self {{ {name}::{}(Default::default()) }} }}", vars[0].1).unwrap();
        let arms: Vec<String> = vars
            .iter()
            .map(|(_, s)| format!("{name}::{s}(v) => v.canon(),"))
            .collect();
        let checks: Vec<String> = vars
            .iter()
            .map(|(_, s)| format!("{name}::{s}(v) => v.check(errs),"))
            .collect();
        writeln!(
            out,
            "impl wire::Check for {name} {{ fn check(&self, errs: &mut {V}<{S}>) {{ match self {{ {} }} }} }}",
            checks.join(" ")
        )
        .unwrap();
        writeln!(
            out,
            "impl wire::Canon for {name} {{ fn canon(&mut self) {{ match self {{ {} }} }} }}",
            arms.join(" ")
        )
        .unwrap();
        writeln!(out, "impl Wire for {name} {{").unwrap();
        writeln!(
            out,
            "    fn encode(&self, out: &mut {V}<u8>) -> {R}<()> {{\n        match self {{"
        )
        .unwrap();
        for (id, s) in &vars {
            writeln!(
                out,
                "            {name}::{s}(v) => {{ wire::put_len(out, {id})?; v.encode(out) }}"
            )
            .unwrap();
        }
        writeln!(out, "        }}\n    }}").unwrap();
        writeln!(out, "    fn decode(b: &[u8], p: usize) -> {R}<(Self, usize)> {{\n        let (id, p) = wire::get_varint(b, p)?;\n        match id {{").unwrap();
        for (id, s) in &vars {
            writeln!(out, "            {id} => {{ let (v, e) = <{s} as Wire>::decode(b, p)?; Ok(({name}::{s}({B}::new(v)), e)) }}").unwrap();
        }
        writeln!(out, "            _ => Err(crate::Error::BadEnum {{ field: {name:?}, value: id as i64 }}),\n        }}\n    }}\n}}").unwrap();
    }

    /// Statement putting the maps inside `place` (a `&mut T`) in key order.
    fn canon_stmt(&self, t: &Value, place: &str, depth: usize) -> String {
        if let Some(m) = t["map"].as_array() {
            let inner = self.canon_stmt(&m[1], &format!("v{depth}"), depth + 1);
            // Struct keys have no order; such maps keep theirs.
            let sortable = m[0]["ref"].as_str().is_none_or(|r| self.kind(r) == "enum");
            let sort = if sortable {
                format!("wire::sort_pairs({place}); ")
            } else {
                String::new()
            };
            if inner.is_empty() {
                sort
            } else {
                format!("{sort}for (_, v{depth}) in {place}.iter_mut() {{ {inner} }}")
            }
        } else if t.get("array").is_some() {
            let inner = self.canon_stmt(&t["array"], &format!("e{depth}"), depth + 1);
            if inner.is_empty() {
                String::new()
            } else if t["nullable_elements"].as_bool() == Some(true) {
                format!("for e{depth} in {place}.iter_mut().flatten() {{ {inner} }}")
            } else {
                format!("for e{depth} in {place}.iter_mut() {{ {inner} }}")
            }
        } else if let Some(r) = t["ref"].as_str() {
            if self.kind(r) == "enum" {
                String::new()
            } else {
                format!("{place}.canon();")
            }
        } else {
            String::new()
        }
    }

    /// Statement checking the maps inside `place` (a `&T`) for duplicate
    /// keys, which the client refuses. `at` names the field in messages.
    fn check_stmt(&self, t: &Value, place: &str, at: &str, depth: usize) -> String {
        if let Some(m) = t["map"].as_array() {
            let inner = self.check_stmt(&m[1], &format!("v{depth}"), at, depth + 1);
            let sortable = m[0]["ref"].as_str().is_none_or(|r| self.kind(r) == "enum");
            let dup = if sortable {
                format!("wire::check_unique_keys({place}, {at:?}, errs); ")
            } else {
                String::new()
            };
            if inner.is_empty() {
                dup
            } else {
                format!("{dup}for (_, v{depth}) in {place}.iter() {{ {inner} }}")
            }
        } else if t.get("array").is_some() {
            let inner = self.check_stmt(&t["array"], &format!("e{depth}"), at, depth + 1);
            if inner.is_empty() {
                String::new()
            } else if t["nullable_elements"].as_bool() == Some(true) {
                format!("for e{depth} in {place}.iter().flatten() {{ {inner} }}")
            } else {
                format!("for e{depth} in {place}.iter() {{ {inner} }}")
            }
        } else if let Some(r) = t["ref"].as_str() {
            if self.kind(r) == "enum" {
                String::new()
            } else {
                format!("{place}.check(errs);")
            }
        } else {
            String::new()
        }
    }

    fn gen_struct(&self, out: &mut String, name: &str, t: &Value) {
        let vs = t["var_start"]
            .as_u64()
            .or(t["fixed_size"].as_u64())
            .unwrap_or(0);
        let fixed = t["fixed"].as_array().cloned().unwrap_or_default();
        let var = t["var"].as_array().cloned().unwrap_or_default();
        let bools = t["bools"].as_object().cloned().unwrap_or_default();
        let is_boxed = |f: &str| self.boxed.contains(&(name.to_owned(), f.to_owned()));

        // Rust fields, in declaration order
        let mut ty_of: BTreeMap<String, String> = BTreeMap::new();
        for (f, _) in &bools {
            ty_of.insert(f.clone(), "bool".into());
        }
        for x in &fixed {
            let Some(f) = x["field"].as_str() else {
                continue;
            };
            let (mut ty, _, _) = self.fixed_codec(&x["enc"]);
            if is_boxed(f) {
                ty = format!("{B}<{ty}>");
            }
            if x.get("null_bit").is_some() {
                ty = format!("{O}<{ty}>");
            }
            ty_of.insert(f.into(), ty);
        }
        for x in &var {
            let f = x["field"].as_str().unwrap();
            let mut ty = self.rty(&x["type"]);
            if is_boxed(f) {
                ty = format!("{B}<{ty}>");
            }
            if x.get("null_bit").is_some() {
                ty = format!("{O}<{ty}>");
            }
            ty_of.insert(f.into(), ty);
        }
        let order: Vec<String> = t["decl_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();

        writeln!(
            out,
            "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {name} {{"
        )
        .unwrap();
        for f in &order {
            writeln!(out, "    pub {}: {},", snake(f), ty_of[f]).unwrap();
        }
        writeln!(out, "}}").unwrap();

        // ---- canon: sort every map, at any depth
        let mut body = String::new();
        let mut each = |f: &str, stmt: String, optional: bool| {
            if stmt.is_empty() {
                return;
            }
            let sf = snake(f);
            if optional {
                writeln!(body, "        if let Some(x) = &mut self.{sf} {{ {stmt} }}").unwrap();
            } else {
                writeln!(body, "        {{ let x = &mut self.{sf}; {stmt} }}").unwrap();
            }
        };
        for x in &fixed {
            if let (Some(f), Some(_)) = (x["field"].as_str(), x["enc"]["struct"].as_str()) {
                each(f, "x.canon();".into(), x.get("null_bit").is_some());
            }
        }
        for x in &var {
            let f = x["field"].as_str().unwrap();
            each(
                f,
                self.canon_stmt(&x["type"], "x", 0),
                x.get("null_bit").is_some(),
            );
        }
        writeln!(out, "impl wire::Canon for {name} {{\n    #[allow(unused_variables)]\n    fn canon(&mut self) {{\n{body}    }}\n}}").unwrap();

        // ---- check: duplicate map keys at any depth; registry indexes in range
        let mut body = String::new();
        let mut each = |f: &str, stmt: String, optional: bool| {
            if stmt.is_empty() {
                return;
            }
            let sf = snake(f);
            if optional {
                writeln!(body, "        if let Some(x) = &self.{sf} {{ {stmt} }}").unwrap();
            } else {
                writeln!(body, "        {{ let x = &self.{sf}; {stmt} }}").unwrap();
            }
        };
        for x in &fixed {
            if let (Some(f), Some(_)) = (x["field"].as_str(), x["enc"]["struct"].as_str()) {
                each(f, "x.check(errs);".into(), x.get("null_bit").is_some());
            }
        }
        let has_max_id = order.iter().any(|f| f == "maxId");
        for x in &var {
            let f = x["field"].as_str().unwrap();
            let at = format!("{name}.{f}");
            let mut stmt = self.check_stmt(&x["type"], "x", &at, 0);
            let int_keys = x["type"]["map"]
                .as_array()
                .is_some_and(|m| m[0]["prim"].as_str() == Some("i32"));
            if has_max_id && int_keys {
                stmt = format!("wire::check_index_range(x, self.max_id, {at:?}, errs); {stmt}");
            }
            each(f, stmt, x.get("null_bit").is_some());
        }
        writeln!(out, "impl wire::Check for {name} {{\n    #[allow(unused_variables)]\n    fn check(&self, errs: &mut {V}<{S}>) {{\n{body}    }}\n}}").unwrap();

        // ---- encode
        let nb = t["null_bytes"].as_u64().unwrap_or(0);
        writeln!(
            out,
            "impl Wire for {name} {{\n    fn encode(&self, out: &mut {V}<u8>) -> {R}<()> {{"
        )
        .unwrap();
        writeln!(
            out,
            "        let base = out.len();\n        out.resize(base + {vs}, 0);"
        )
        .unwrap();
        if nb > 0 {
            writeln!(out, "        let mut nb = [0u8; {nb}];").unwrap();
        }
        for x in &fixed {
            if let Some(at) = x["packed_bools"].as_u64() {
                writeln!(out, "        {{ let mut x = 0u8;").unwrap();
                for (f, pos) in &bools {
                    if pos[0].as_u64() == Some(at) {
                        writeln!(
                            out,
                            "          if self.{} {{ x |= 1 << {}; }}",
                            snake(f),
                            pos[1]
                        )
                        .unwrap();
                    }
                }
                writeln!(out, "          out[base + {at}] = x; }}").unwrap();
                continue;
            }
            let f = x["field"].as_str().unwrap();
            let at = x["at"].as_u64().unwrap();
            let (_, _, enc) = self.fixed_codec(&x["enc"]);
            let deref = if is_boxed(f) { "&**" } else { "" };
            if let Some(bit) = x["null_bit"].as_u64() {
                writeln!(
                    out,
                    "        if let Some(v) = &self.{sf} {{ nb[{}] |= 1 << {}; let mut t = {V}::new(); ({enc})(&mut t, {deref}v)?; wire::place(out, base + {at}, &t); }}",
                    bit / 8,
                    bit % 8,
                    sf = snake(f)
                )
                .unwrap();
            } else {
                let r = if is_boxed(f) {
                    format!("&*self.{}", snake(f))
                } else {
                    format!("&self.{}", snake(f))
                };
                writeln!(out, "        {{ let mut t = {V}::new(); ({enc})(&mut t, {r})?; wire::place(out, base + {at}, &t); }}").unwrap();
            }
        }
        for x in &var {
            let f = x["field"].as_str().unwrap();
            let sf = snake(f);
            let enum32 = x["enum_int"].as_bool() == Some(true);
            let enc = self.enc(&x["type"], enum32);
            let slot = x["slot"].as_u64();
            let set_slot = |rel: &str| {
                slot.map(|s| {
                    format!(
                        "{{ let r: i32 = {rel}; wire::place(out, base + {s}, &r.to_le_bytes()); }} "
                    )
                })
                .unwrap_or_default()
            };
            let deref = if is_boxed(f) { "&**" } else { "" };
            let rel = format!("(out.len() - base - {vs}) as i32");
            if let Some(bit) = x["null_bit"].as_u64() {
                writeln!(
                    out,
                    "        if let Some(v) = &self.{sf} {{ nb[{}] |= 1 << {}; {}({enc})(out, {deref}v)?; }} else {{ {} }}",
                    bit / 8,
                    bit % 8,
                    set_slot(&rel),
                    set_slot("-1i32")
                )
                .unwrap();
            } else {
                let r = if is_boxed(f) {
                    format!("&*self.{sf}")
                } else {
                    format!("&self.{sf}")
                };
                writeln!(out, "        {}({enc})(out, {r})?;", set_slot(&rel)).unwrap();
            }
        }
        if nb > 0 {
            writeln!(out, "        wire::place(out, base, &nb);").unwrap();
        }
        writeln!(out, "        Ok(())\n    }}").unwrap();

        // ---- decode
        writeln!(
            out,
            "    fn decode(b: &[u8], base: usize) -> {R}<(Self, usize)> {{"
        )
        .unwrap();
        writeln!(out, "        wire::need(b, base + {vs})?;").unwrap();
        writeln!(
            out,
            "        #[allow(unused)] let bit = |i: usize| b[base + i / 8] >> (i % 8) & 1 != 0;"
        )
        .unwrap();
        for (f, pos) in &bools {
            writeln!(
                out,
                "        let f_{} = b[base + {}] >> {} & 1 != 0;",
                snake(f).trim_start_matches("r#"),
                pos[0],
                pos[1]
            )
            .unwrap();
        }
        for x in &fixed {
            let Some(f) = x["field"].as_str() else {
                continue;
            };
            let at = x["at"].as_u64().unwrap();
            let (_, dec, _) = self.fixed_codec(&x["enc"]);
            let wrap = if is_boxed(f) {
                format!("{B}::new(v)")
            } else {
                "v".into()
            };
            if let Some(bit) = x["null_bit"].as_u64() {
                writeln!(out, "        let f_{} = if bit({bit}) {{ let (v, _) = ({dec})(b, base + {at})?; Some({wrap}) }} else {{ None }};", snake(f).trim_start_matches("r#")).unwrap();
            } else {
                writeln!(
                    out,
                    "        let f_{} = {{ let (v, _) = ({dec})(b, base + {at})?; {wrap} }};",
                    snake(f).trim_start_matches("r#")
                )
                .unwrap();
            }
        }
        writeln!(
            out,
            "        #[allow(unused_mut)] let mut pos = base + {vs};"
        )
        .unwrap();
        for x in &var {
            let f = x["field"].as_str().unwrap();
            let sf = format!("f_{}", snake(f).trim_start_matches("r#"));
            let enum32 = x["enum_int"].as_bool() == Some(true);
            let dec = self.dec(&x["type"], enum32);
            let present = x["null_bit"]
                .as_u64()
                .map(|b| format!("bit({b})"))
                .unwrap_or_else(|| "true".into());
            let check = x["slot"]
                .as_u64()
                .map(|s| format!("wire::slot(b, base + {s}, present, pos - base - {vs}, {f:?})?; "))
                .unwrap_or_default();
            let wrap = if is_boxed(f) {
                format!("{B}::new(v)")
            } else {
                "v".into()
            };
            if x.get("null_bit").is_some() {
                writeln!(out, "        let {sf} = {{ let present = {present}; {check}if present {{ let (v, e) = ({dec})(b, pos)?; pos = e; Some({wrap}) }} else {{ None }} }};").unwrap();
            } else {
                writeln!(out, "        let {sf} = {{ let present = {present}; {check}let (v, e) = ({dec})(b, pos)?; pos = e; {wrap} }};").unwrap();
            }
        }
        let fields: Vec<String> = order
            .iter()
            .map(|f| format!("{}: f_{}", snake(f), snake(f).trim_start_matches("r#")))
            .collect();
        writeln!(
            out,
            "        Ok((Self {{ {} }}, pos))\n    }}\n}}",
            fields.join(", ")
        )
        .unwrap();
    }

    pub fn generate(&self, packet_names: &[(u32, String)]) -> String {
        let mut out = String::from(
            "// Generated by build_codegen.rs from protocol.json (tools/gen.py). Do not edit.\n\n",
        );
        for (name, t) in &self.types {
            match t["kind"].as_str().unwrap() {
                "enum" => self.gen_enum(&mut out, name, t),
                "poly" => self.gen_poly(&mut out, name, t),
                "struct" => self.gen_struct(&mut out, name, t),
                _ => {}
            }
        }
        // decode + re-encode by packet id, for tests and tools
        writeln!(out, "/// Decodes a packet payload by id and encodes it again.\npub fn reencode(id: u32, payload: &[u8]) -> {R}<{O}<{V}<u8>>> {{\n    match id {{").unwrap();
        for (id, n) in packet_names {
            if self.kind(n) == "struct" {
                writeln!(
                    out,
                    "        {id} => {{ let (v, end) = <{n} as Wire>::decode(payload, 0)?; if end != payload.len() {{ return Err(crate::Error::Short {{ need: end, have: payload.len() }}); }} let mut o = {V}::new(); v.encode(&mut o)?; Ok(Some(o)) }}"
                )
                .unwrap();
            }
        }
        writeln!(out, "        _ => Ok(None),\n    }}\n}}").unwrap();
        writeln!(out, "/// Decodes a packet payload by id into its debug text.\npub fn debug(id: u32, payload: &[u8]) -> {R}<{O}<{S}>> {{\n    match id {{").unwrap();
        for (id, n) in packet_names {
            if self.kind(n) == "struct" {
                writeln!(out, "        {id} => Ok(Some(format!(\"{{:#?}}\", <{n} as Wire>::decode(payload, 0)?.0))),").unwrap();
            }
        }
        writeln!(out, "        _ => Ok(None),\n    }}\n}}").unwrap();
        writeln!(out, "/// Decodes a packet payload by id and lists what the client would refuse\n/// (duplicate map keys, registry indexes out of range).\npub fn check(id: u32, payload: &[u8]) -> {R}<{O}<{V}<{S}>>> {{\n    match id {{").unwrap();
        for (id, n) in packet_names {
            if self.kind(n) == "struct" {
                writeln!(out, "        {id} => {{ let mut errs = {V}::new(); wire::Check::check(&<{n} as Wire>::decode(payload, 0)?.0, &mut errs); Ok(Some(errs)) }}").unwrap();
            }
        }
        writeln!(out, "        _ => Ok(None),\n    }}\n}}").unwrap();
        out
    }
}
