//! The editor tools every player starts with (spec 14): the items named
//! by the tools menu assets (`Server/Item/PlayerToolsMenuConfig`, each with
//! a `BuilderToolItems` list). The server puts them, asset by asset (sorted
//! by asset name) and in list order, into the tools section on join.
//!
//! Text lines for the start-up cache: `BT item`.

use crate::Result;
use crate::ctx::Ctx;

pub const PATH: &str = "Item/PlayerToolsMenuConfig";

pub fn table(ctx: &Ctx) -> Result<String> {
    let store = ctx.store(PATH)?;
    let mut keys: Vec<&String> = store.own.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        let a = store.resolved(k, "BuilderToolItemReferenceAsset");
        for item in a
            .get("BuilderToolItems")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            if !item.is_empty() && !item.contains(char::is_whitespace) {
                out += &format!("BT {item}\n");
            }
        }
    }
    Ok(out)
}
