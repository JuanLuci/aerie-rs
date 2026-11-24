//! Chat lines as the official server writes them (spec 28): a translation
//! key the client turns into its own language, with named parameters,
//! instead of finished text. Build one with `Msg::key("…")`, add parameters
//! with the method of the value's kind (the official server picks the kind
//! by the value's type, and the client formats each kind its own way), then
//! `packet()`.
//!
//! The official server keeps a message's parameters in two Java hash maps
//! (plain values, and values that are messages), so they go out in that
//! map's iteration order, not in the order they were added; setting a name
//! twice keeps the last value. `Msg` does the same when it is finished.
//!
//! ```ignore
//! Msg::key("server.commands.block.set.success")
//!     .int("x", x).int("y", y).int("z", z)
//!     .text("id", block)
//!     .packet()
//! ```

use aerie_protocol::proto;
use anyhow::Result;

use crate::world::{Packet, id::SERVER_MESSAGE, packet};

/// One formatted message: a key or raw text, its parameters, its style.
#[derive(Clone, Debug, Default)]
pub struct Msg(proto::FormattedMessage);

// Every kind of parameter is here, used yet or not.
#[allow(dead_code)]
impl Msg {
    /// A message the client translates from `key`.
    pub fn key(key: &str) -> Self {
        Msg(proto::FormattedMessage {
            message_id: Some(key.to_owned()),
            ..Default::default()
        })
    }

    /// Text shown as written (what players type, names, Aerie's own lines
    /// that the official server does not have).
    pub fn raw(text: &str) -> Self {
        Msg(proto::FormattedMessage {
            raw_text: Some(text.to_owned()),
            ..Default::default()
        })
    }

    fn param(mut self, name: &str, v: proto::ParamValue) -> Self {
        put(self.0.params.get_or_insert_with(Vec::new), name, v);
        self
    }

    /// A text parameter.
    pub fn text(self, name: &str, v: &str) -> Self {
        let v = proto::StringParamValue {
            value: Some(v.to_owned()),
        };
        self.param(name, proto::ParamValue::StringParamValue(Box::new(v)))
    }

    /// A 32-bit integer parameter.
    pub fn int(self, name: &str, v: i32) -> Self {
        let v = proto::IntParamValue { value: v };
        self.param(name, proto::ParamValue::IntParamValue(Box::new(v)))
    }

    /// A 64-bit integer parameter.
    pub fn long(self, name: &str, v: i64) -> Self {
        let v = proto::LongParamValue { value: v };
        self.param(name, proto::ParamValue::LongParamValue(Box::new(v)))
    }

    /// A floating point parameter.
    pub fn double(self, name: &str, v: f64) -> Self {
        let v = proto::DoubleParamValue { value: v };
        self.param(name, proto::ParamValue::DoubleParamValue(Box::new(v)))
    }

    /// A true/false parameter.
    pub fn bool(self, name: &str, v: bool) -> Self {
        let v = proto::BoolParamValue { value: v };
        self.param(name, proto::ParamValue::BoolParamValue(Box::new(v)))
    }

    /// A parameter that is itself a message (translated on its own, such as
    /// an item's name).
    pub fn msg(mut self, name: &str, m: Msg) -> Self {
        let m = m.into_inner();
        put(self.0.message_params.get_or_insert_with(Vec::new), name, m);
        self
    }

    /// A text colour (`#rrggbb`).
    pub fn color(mut self, c: &str) -> Self {
        self.0.color = Some(c.to_owned());
        self
    }

    /// Appends a message shown after this one.
    pub fn child(mut self, m: Msg) -> Self {
        let m = m.into_inner();
        self.0.children.get_or_insert_with(Vec::new).push(m);
        self
    }

    /// The finished message, its parameters in the official order.
    pub fn into_inner(mut self) -> proto::FormattedMessage {
        if let Some(p) = &mut self.0.params {
            hash_map_order(p);
        }
        if let Some(p) = &mut self.0.message_params {
            hash_map_order(p);
        }
        self.0
    }

    /// The `ServerMessage` packet carrying it as a chat line.
    pub fn packet(self) -> Result<Packet> {
        packet(
            SERVER_MESSAGE,
            &proto::ServerMessage {
                r#type: proto::ChatType::CHAT,
                message: Some(self.into_inner()),
            },
        )
    }
}

/// A Java map's `put`: a name already there keeps its place and takes the
/// new value.
fn put<T>(list: &mut Vec<(String, T)>, name: &str, v: T) {
    match list.iter_mut().find(|(k, _)| k == name) {
        Some(slot) => slot.1 = v,
        None => list.push((name.to_owned(), v)),
    }
}

/// Reorders entries as a Java `HashMap` filled in this order iterates them:
/// by bucket (`h ^ h >>> 16` of the key's `String.hashCode`, in a table of
/// 16 that doubles whenever it is more than three quarters full), and in
/// insertion order inside a bucket (a doubling keeps that order).
fn hash_map_order<T>(list: &mut [(String, T)]) {
    let mut table = 16usize;
    while list.len() * 4 > table * 3 {
        table *= 2;
    }
    let bucket = |k: &str| {
        let h = aerie_assets::chm::java_hash(k);
        ((h ^ (h >> 16)) as usize) & (table - 1)
    };
    list.sort_by_key(|(k, _)| bucket(k));
}

#[cfg(test)]
mod tests {
    use super::*;
    use aerie_protocol::wire::Wire;

    #[test]
    fn keeps_parameters_in_order_with_their_kinds() {
        let m = Msg::key("a.b")
            .int("x", 1)
            .text("id", "Rock")
            .msg("item", Msg::key("items.rock.name"))
            .color("#ff5555")
            .into_inner();
        let p = m.params.unwrap();
        // `x` (bucket 8) comes before `id` (bucket 11).
        assert_eq!(p[0].0, "x");
        assert!(matches!(p[0].1, proto::ParamValue::IntParamValue(_)));
        assert!(matches!(p[1].1, proto::ParamValue::StringParamValue(_)));
        assert_eq!(m.message_params.unwrap()[0].0, "item");
        assert_eq!(m.color.as_deref(), Some("#ff5555"));
    }

    #[test]
    fn orders_parameters_as_a_java_hash_map() {
        // The official chat line lists `message` before `username`.
        let m = Msg::key("server.chat.playerMessage")
            .text("username", "Ana")
            .text("message", "oi")
            .into_inner();
        let names: Vec<_> = m.params.unwrap().into_iter().map(|p| p.0).collect();
        assert_eq!(names, ["message", "username"]);
        // A name set twice keeps its place and the last value.
        let m = Msg::key("k")
            .int("a", 1)
            .int("b", 2)
            .int("a", 3)
            .into_inner();
        let p = m.params.unwrap();
        assert_eq!(p.len(), 2);
        assert!(matches!(&p[0].1, proto::ParamValue::IntParamValue(v) if v.value == 3));
    }

    #[test]
    fn makes_a_server_message() {
        let (id, bytes) = Msg::key("a.b").packet().unwrap();
        assert_eq!(id, SERVER_MESSAGE);
        let (m, _) = proto::ServerMessage::decode(&bytes, 0).unwrap();
        assert_eq!(m.message.unwrap().message_id.as_deref(), Some("a.b"));
    }
}
