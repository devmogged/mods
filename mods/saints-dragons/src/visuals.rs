//! Dragon animation state and stock-client model attached to the living carrier.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, OnceLock, PoisonError},
};

use fastnbt::Value;
use moddedmc_api::{
    InterfaceUnavailable, RegistrationError, Registry,
    content::{ContentEvent, World},
    entities::{Entity, EntityView},
    items::{Item, Items},
    stacks::{self, Displays, ItemDisplay},
    tick_count,
};

use crate::{ENTITIES, abilities::Attack};

const PARTS: usize = 5;
const SCALE: f64 = 1.85;
const MODELS: [(&str, &str, [f64; 3]); PARTS] = [
    (
        "saintsdragons:cindervane_display",
        "saintsdragons:item/cindervane_body",
        [0.0, 0.0, 0.0],
    ),
    (
        "saintsdragons:cindervane_head",
        "saintsdragons:item/cindervane_head",
        [0.0, 4.0, 8.0],
    ),
    (
        "saintsdragons:cindervane_tail",
        "saintsdragons:item/cindervane_tail",
        [0.0, 0.0, -6.0],
    ),
    (
        "saintsdragons:cindervane_wing_left",
        "saintsdragons:item/cindervane_wing_left",
        [-4.0, 2.0, 3.0],
    ),
    (
        "saintsdragons:cindervane_wing_right",
        "saintsdragons:item/cindervane_wing_right",
        [4.0, 2.0, 3.0],
    ),
];
const REFRESH: u64 = 20;
const LEASE: u64 = 40;

struct Rig {
    ids: [i32; PARTS],
    seen: u64,
    frames: [ItemDisplay; PARTS],
    pose: Pose,
    clip: &'static str,
}

struct Pose {
    grounded: bool,
    transition: Option<(&'static str, u64)>,
    attack: Option<(Attack, &'static str)>,
}

impl Pose {
    fn new(view: EntityView) -> Self {
        Self {
            grounded: view.on_ground,
            transition: None,
            attack: None,
        }
    }

    fn clip(
        &mut self,
        view: EntityView,
        now: u64,
        autonomous_landing: Option<bool>,
        attack: Option<Attack>,
    ) -> &'static str {
        if self.grounded != view.on_ground {
            self.grounded = view.on_ground;
            self.transition = if view.on_ground {
                Some(("landed", now + 33))
            } else if view.motion[1] > 0.08 {
                Some(("takeoff", now + 25))
            } else {
                None
            };
        }
        if let Some(attack) = attack {
            self.transition = None;
            if let Some((previous, clip)) = self.attack
                && attack == previous
            {
                return clip;
            }
            let clip = match (attack, view.on_ground) {
                (Attack::Bite, true) => "bite",
                (Attack::Bite, false) => "bite_air",
                (Attack::DoubleBite, true) => "double_bite",
                (Attack::DoubleBite, false) => "double_bite_air",
                (Attack::SlashGrab, _) => "cindervane_slash_left",
                (Attack::MagmaVolley, _) => "magma_volley",
            };
            self.attack = Some((attack, clip));
            return clip;
        }
        self.attack = None;
        if let Some((clip, _)) = self.transition.filter(|(_, end)| now < *end) {
            return clip;
        }
        self.transition = None;
        let speed = view.motion[0].hypot(view.motion[2]);
        let vertical = view.motion[1];
        if view.on_ground {
            if speed > 0.32 {
                "run"
            } else if speed > 0.035 {
                "walk"
            } else {
                "idle"
            }
        } else if autonomous_landing == Some(true) && vertical < -0.08 {
            "landing"
        } else if speed < 0.13 && vertical < -0.2 {
            "falling"
        } else if speed > 1.6 && vertical >= -0.15 {
            "sprint_flap"
        } else if vertical > 0.1 {
            "flap"
        } else if speed < 0.13 {
            "fly_idle"
        } else {
            "glide"
        }
    }
}

static DISPLAYS: OnceLock<Displays> = OnceLock::new();
// A mob changing dimension can briefly have an expiring rig in its previous world.
static RIGS: LazyLock<Mutex<HashMap<i32, Vec<Rig>>>> = LazyLock::new(Mutex::default);
static STACKS: LazyLock<[Vec<u8>; PARTS]> = LazyLock::new(|| {
    MODELS.map(|(id, _, _)| {
        fastnbt::to_bytes(&HashMap::from([
            ("id", Value::String(id.to_owned())),
            ("count", Value::Int(1)),
        ]))
        .unwrap_or_default()
    })
});

fn body_stack(species: &str, clip: &str) -> Vec<u8> {
    fastnbt::to_bytes(&HashMap::from([
        (
            "id",
            Value::String(format!("saintsdragons:{species}_display")),
        ),
        ("count", Value::Int(1)),
        (
            "components",
            Value::Compound(HashMap::from([(
                "minecraft:custom_data".to_owned(),
                Value::Compound(HashMap::from([(
                    "moddedmc:rig_clip".to_owned(),
                    Value::String(format!("animation.{species}.{clip}")),
                )])),
            )])),
        ),
    ]))
    .expect("Cindervane display stack has fixed NBT fields")
}

pub fn register(items: Items, registry: &mut Registry<'_>) -> Result<(), RegistrationError> {
    for (id, model, _) in MODELS {
        items.add_item(
            registry,
            &Item {
                id,
                model,
                display_name: "Cindervane Model",
                carrier: "minecraft:paper",
            },
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Rotation([f64; 4]);

impl Rotation {
    fn axis(axis: usize, angle: f64) -> Self {
        let (sin, cos) = (angle * 0.5).sin_cos();
        let mut rotation = [0.0, 0.0, 0.0, cos];
        rotation[axis] = sin;
        Self(rotation)
    }

    fn then(self, local: Self) -> Self {
        let [x, y, z, w] = self.0;
        let [a, b, c, d] = local.0;
        Self([
            w * a + x * d + y * c - z * b,
            w * b - x * c + y * d + z * a,
            w * c + x * b - y * a + z * d,
            w * d - x * a - y * b - z * c,
        ])
    }

    fn point(self, [a, b, c]: [f64; 3]) -> [f64; 3] {
        let [x, y, z, w] = self.0;
        let [u, v, t] = [
            2.0 * (y * c - z * b),
            2.0 * (z * a - x * c),
            2.0 * (x * b - y * a),
        ];
        [
            a + w * u + y * t - z * v,
            b + w * v + z * u - x * t,
            c + w * t + x * v - y * u,
        ]
    }
}

fn frames(view: EntityView, time: u64, striking: bool) -> [ItemDisplay; PARTS] {
    let elapsed = (time % 20_000) as f64;
    let speed = view.motion[0].hypot(view.motion[2]);
    let air = !view.on_ground;
    let pitch = if air {
        (-view.motion[1] * 0.36).clamp(-0.27, 0.27)
    } else {
        0.0
    };
    let body = Rotation::axis(1, -f64::from(view.yaw).to_radians()).then(Rotation::axis(0, pitch));
    let bob = if air {
        0.065 * (elapsed * 0.32).sin()
    } else {
        0.03 * (elapsed * 0.58).sin() * (speed * 6.0).min(1.0)
    };
    let relative_head = f64::from(view.head_yaw - view.yaw).to_radians();
    let head_yaw = relative_head
        .sin()
        .atan2(relative_head.cos())
        .clamp(-0.66, 0.66);
    let head_pitch = f64::from(view.pitch).to_radians().clamp(-0.48, 0.48);
    let wing = if air {
        0.62 * (elapsed * 0.34).sin() + 0.08
    } else {
        0.51 + 0.06 * (elapsed * 0.16).sin()
    };
    let locals = [
        Rotation::axis(0, 0.0),
        Rotation::axis(1, -head_yaw).then(Rotation::axis(
            0,
            head_pitch + if striking { 0.24 } else { 0.0 },
        )),
        Rotation::axis(1, (elapsed * if air { 0.28 } else { 0.14 }).sin() * 0.23),
        Rotation::axis(2, -wing),
        Rotation::axis(2, wing),
    ];
    let root = [
        view.position[0],
        view.position[1] + 1.15 + bob,
        view.position[2],
    ];
    std::array::from_fn(|i| {
        let pivot = body.point(MODELS[i].2.map(|value| value * SCALE / 16.0));
        ItemDisplay {
            position: std::array::from_fn(|axis| root[axis] + pivot[axis]),
            rotation: body.then(locals[i]).0.map(|value| value as f32),
            scale: SCALE as f32,
            context: 0,
            interpolation: 2,
        }
    })
}

pub fn start() -> Result<(), InterfaceUnavailable> {
    let _ = DISPLAYS.set(stacks::require_displays()?);
    Ok(())
}

pub fn tick(world: World<'_>, mob: Entity<'_>) {
    let Some(view) = mob.view() else { return };
    let now = tick_count();
    let attack = crate::combat::active_attack(mob);
    let landing = crate::flight::landing(mob.id());
    let descriptions = frames(view, now + (mob.id() as u64 % 47), attack.is_some());
    draw(world, mob, "cindervane", view, &descriptions, |pose| {
        pose.clip(view, now, landing, attack)
    });
}

pub fn tick_species(
    world: World<'_>,
    mob: Entity<'_>,
    species: &str,
    view: EntityView,
    clip: &'static str,
) {
    let description = ItemDisplay {
        position: view.position,
        rotation: Rotation::axis(1, -f64::from(view.yaw).to_radians())
            .0
            .map(|value| value as f32),
        scale: 1.0,
        context: 0,
        interpolation: 2,
    };
    draw(world, mob, species, view, &[description], |_| clip);
}

fn draw(
    world: World<'_>,
    mob: Entity<'_>,
    species: &str,
    view: EntityView,
    descriptions: &[ItemDisplay],
    select: impl FnOnce(&mut Pose) -> &'static str,
) {
    let (Some(displays), Some(entities)) = (DISPLAYS.get(), ENTITIES.get()) else {
        return;
    };
    let now = tick_count();
    let refresh = {
        let mut all = RIGS.lock().unwrap_or_else(PoisonError::into_inner);
        let rigs = all.entry(mob.id()).or_default();
        if let Some(rig) = rigs.iter_mut().find(|rig| {
            rig.ids
                .iter()
                .any(|&id| id != 0 && entities.with_entity(world, id, |_| ()).is_some())
        }) {
            rig.seen = now;
            let clip = select(&mut rig.pose);
            if clip != rig.clip && displays.set_item(world, rig.ids[0], &body_stack(species, clip))
            {
                rig.clip = clip;
            }
            if (now + mob.id() as u64).is_multiple_of(2) {
                for (i, &description) in descriptions.iter().enumerate() {
                    if rig.frames[i].position != description.position
                        || rig.frames[i].rotation != description.rotation
                    {
                        if !displays.update(world, rig.ids[i], &description) {
                            let body;
                            let stack = if i == 0 {
                                body = body_stack(species, clip);
                                &body
                            } else {
                                &STACKS[i]
                            };
                            let Some(id) = displays.spawn_item(world, stack, &description) else {
                                continue;
                            };
                            rig.ids[i] = id;
                            if i == 0 {
                                rig.clip = clip;
                            }
                        }
                        rig.frames[i] = description;
                    }
                }
            }
            (now + mob.id() as u64).is_multiple_of(REFRESH)
        } else {
            let mut pose = Pose::new(view);
            let clip = select(&mut pose);
            let body = body_stack(species, clip);
            let mut ids = [0; PARTS];
            for (i, description) in descriptions.iter().enumerate() {
                if let Some(id) =
                    displays.spawn_item(world, if i == 0 { &body } else { &STACKS[i] }, description)
                {
                    ids[i] = id;
                } else {
                    for id in ids.into_iter().take(i) {
                        displays.remove(world, id);
                    }
                    return;
                }
            }
            rigs.push(Rig {
                ids,
                seen: now,
                frames: std::array::from_fn(|i| *descriptions.get(i).unwrap_or(&descriptions[0])),
                pose,
                clip,
            });
            true
        }
    };
    if refresh {
        mob.add_effect("minecraft:invisibility", LEASE as i32, 0, true, false);
    }
}

pub fn sweep(world: World<'_>, time: u64, _events: &[ContentEvent]) {
    let (Some(displays), Some(entities)) = (DISPLAYS.get(), ENTITIES.get()) else {
        return;
    };
    if !time.is_multiple_of(REFRESH) {
        return;
    }
    let now = tick_count();
    RIGS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|_, rigs| {
            rigs.retain(|rig| {
                let age = now.saturating_sub(rig.seen);
                if age < LEASE {
                    return true;
                }
                if rig
                    .ids
                    .iter()
                    .any(|&id| id != 0 && entities.with_entity(world, id, |_| ()).is_some())
                {
                    for id in rig.ids.into_iter().filter(|&id| id != 0) {
                        displays.remove(world, id);
                    }
                    return false;
                }
                age < LEASE * 3
            });
            !rigs.is_empty()
        });
}

pub fn death(world: World<'_>, mob: Entity<'_>) {
    let (Some(displays), Some(entities)) = (DISPLAYS.get(), ENTITIES.get()) else {
        return;
    };
    let mut all = RIGS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(rigs) = all.get_mut(&mob.id()) {
        rigs.retain(|rig| {
            if rig
                .ids
                .iter()
                .any(|&id| id != 0 && entities.with_entity(world, id, |_| ()).is_some())
            {
                for id in rig.ids.into_iter().filter(|&id| id != 0) {
                    displays.remove(world, id);
                }
                return false;
            }
            true
        });
    }
}

pub fn is_display(id: i32) -> bool {
    id != 0
        && RIGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .flatten()
            .any(|rig| rig.ids.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_flight_and_attack_clips_then_resumes_movement() {
        let ground = EntityView {
            on_ground: true,
            ..EntityView::default()
        };
        let rising = EntityView {
            on_ground: false,
            motion: [0.3, 0.3, 0.0],
            ..ground
        };
        let cruising = EntityView {
            motion: [1.2, 0.0, 0.0],
            ..rising
        };
        let descending = EntityView {
            motion: [0.0, -0.42, 0.0],
            ..rising
        };
        let mut pose = Pose::new(ground);
        assert_eq!(pose.clip(ground, 0, None, None), "idle");
        assert_eq!(pose.clip(rising, 1, Some(false), None), "takeoff");
        assert_eq!(pose.clip(rising, 26, Some(false), None), "flap");
        assert_eq!(pose.clip(cruising, 37, Some(false), None), "glide");
        assert_eq!(pose.clip(descending, 60, Some(true), None), "landing");
        assert_eq!(pose.clip(ground, 70, None, None), "landed");
        for (attack, clip) in [
            (Attack::Bite, "bite"),
            (Attack::DoubleBite, "double_bite"),
            (Attack::SlashGrab, "cindervane_slash_left"),
            (Attack::MagmaVolley, "magma_volley"),
        ] {
            assert_eq!(pose.clip(ground, 71, None, Some(attack)), clip);
        }
        assert_eq!(pose.clip(ground, 100, None, None), "idle");

        assert_eq!(
            pose.clip(ground, 101, None, Some(Attack::DoubleBite)),
            "double_bite"
        );
        assert_eq!(
            pose.clip(rising, 102, None, Some(Attack::DoubleBite)),
            "double_bite"
        );
        pose.clip(cruising, 103, None, None);
        assert_eq!(
            pose.clip(cruising, 104, None, Some(Attack::DoubleBite)),
            "double_bite_air"
        );
    }

    #[test]
    fn display_stack_keeps_the_body_item_and_companion_clip() {
        let stack: HashMap<String, Value> =
            fastnbt::from_bytes(&body_stack("cindervane", "takeoff")).unwrap();
        assert_eq!(stack["id"], Value::String(MODELS[0].0.to_owned()));
        let Value::Compound(components) = &stack["components"] else {
            panic!("display stack is missing its components");
        };
        let Value::Compound(custom) = &components["minecraft:custom_data"] else {
            panic!("display stack is missing its custom data");
        };
        assert_eq!(
            custom["moddedmc:rig_clip"],
            Value::String("animation.cindervane.takeoff".to_owned())
        );
    }

    #[test]
    fn flight_moves_wing_rotations_without_moving_their_hinges() {
        let view = EntityView {
            yaw: 90.0,
            ..EntityView::default()
        };
        let first = frames(view, 0, false);
        let next = frames(view, 5, false);
        assert_ne!(first[3].rotation, next[3].rotation);
        assert_ne!(first[4].rotation, next[4].rotation);
        for sample in [first, next] {
            assert_eq!(sample[3].position[1], sample[4].position[1]);
            assert!((sample[3].position[2] - sample[4].position[2]).abs() > 0.9);
        }
    }
}
