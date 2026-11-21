//! `UpdateCameraShake` (77) from `Server/Camera/CameraShake` (spec 07).
//! Indexed, no built-in entries. `FirstPerson`, `ThirdPerson` → shake configs.

use aerie_protocol::proto::{CameraShake, UpdateCameraShake};

use super::camera::shake;
use crate::{Result, ctx::Ctx};

pub const PATH: &str = "Camera/CameraShake";

pub fn build(ctx: &Ctx) -> Result<UpdateCameraShake> {
    let store = ctx.store(PATH)?;
    let order = ctx.order(PATH, &[])?;
    let profiles = order
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let a = &store.assets[k];
            (
                i as i32,
                CameraShake {
                    first_person: shake(a.get("FirstPerson")),
                    third_person: shake(a.get("ThirdPerson")),
                },
            )
        })
        .collect();
    Ok(UpdateCameraShake {
        r#type: super::INIT,
        profiles: Some(profiles),
    })
}
