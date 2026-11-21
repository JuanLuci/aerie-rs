//! `UpdateViewBobbing` (76) from `Server/Camera/ViewBobbing` (spec 07).
//! Keyed by movement type: the file name is a `MovementType` name.
//! `FirstPerson` → `first_person` (a shake config, see `camera`).

use aerie_protocol::proto::{MovementType, UpdateViewBobbing, ViewBobbing};

use super::camera::shake;
use crate::Result;
use crate::ctx::{Ctx, invalid};

pub const PATH: &str = "Camera/ViewBobbing";

pub fn build(ctx: &Ctx) -> Result<UpdateViewBobbing> {
    let store = ctx.store(PATH)?;
    let mut profiles = Vec::new();
    for (k, a) in &store.assets {
        let t = MovementType::from_name(k).ok_or_else(|| invalid(k, "not a movement type"))?;
        profiles.push((
            t,
            ViewBobbing {
                first_person: shake(a.get("FirstPerson")),
            },
        ));
    }
    Ok(UpdateViewBobbing {
        r#type: super::INIT,
        profiles,
    })
}
