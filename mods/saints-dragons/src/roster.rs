//! The other 0.9.88 dragon entities and their baseline movement.

use moddedmc_api::{
    Pos, RegistrationError, Registry,
    commands::{Argument, Node},
    content::World,
    entities::{Entity, EntityView},
    interaction::{UseOnBlock, UseResult},
    items::Item,
    mob_ai::{Goal, goal_control, goal_phase},
    mobs::Mob,
};

use crate::{COMMANDS, ENTITIES, INTERACTION, ITEMS, MOB_AI, MOBS, ai, flight, taming, visuals};

pub const AMBIENT: u64 = 9;
const SPAWN_TAG: u64 = 100;

struct Species {
    name: &'static str,
    display: &'static str,
    carrier: &'static str,
    health: f64,
    speed: f64,
    walk: &'static str,
    flight: Option<(&'static str, &'static str)>,
    swim: Option<&'static str>,
    aquatic: bool,
}

// Adult health and ground movement speed come from the release's stat profiles.
const SPECIES: [Species; 9] = [
    Species {
        name: "raevyx",
        display: "Raevyx",
        carrier: "ravager",
        health: 180.0,
        speed: 0.30,
        walk: "walk",
        flight: Some(("flap", "fly_glide")),
        swim: None,
        aquatic: false,
    },
    Species {
        name: "stegonaut",
        display: "Stegonaut",
        carrier: "ravager",
        health: 100.0,
        speed: 0.28,
        walk: "walk",
        flight: None,
        swim: None,
        aquatic: false,
    },
    Species {
        name: "varasuchus",
        display: "Varasuchus",
        carrier: "ravager",
        health: 200.0,
        speed: 0.33,
        walk: "walk",
        flight: None,
        swim: Some("swim_move"),
        aquatic: false,
    },
    Species {
        name: "ignivorus",
        display: "Ignivorus",
        carrier: "ravager",
        health: 450.0,
        speed: 0.30,
        walk: "walk",
        flight: Some(("flap", "glide")),
        swim: None,
        aquatic: false,
    },
    Species {
        name: "volitans",
        display: "Volitans",
        carrier: "ravager",
        health: 160.0,
        speed: 0.30,
        walk: "walk",
        flight: Some(("flap", "fly_glide")),
        swim: Some("swim"),
        aquatic: false,
    },
    Species {
        name: "nulljaw",
        display: "Nulljaw",
        carrier: "spider",
        health: 70.0,
        speed: 0.20,
        walk: "hover",
        flight: Some(("hover", "hover")),
        swim: None,
        aquatic: false,
    },
    Species {
        name: "atroxiia",
        display: "Atroxiia",
        carrier: "ravager",
        health: 200.0,
        speed: 0.33,
        walk: "walk",
        flight: None,
        swim: Some("swim"),
        aquatic: false,
    },
    Species {
        name: "moop",
        display: "Moop",
        carrier: "cod",
        health: 3.0,
        speed: 0.8,
        walk: "on_land",
        flight: None,
        swim: Some("swim"),
        aquatic: true,
    },
    Species {
        name: "mossback",
        display: "Mossback",
        carrier: "wolf",
        health: 10.0,
        speed: 0.15,
        walk: "walk",
        flight: None,
        swim: None,
        aquatic: false,
    },
];

impl Species {
    fn clip(&self, view: EntityView, wet: bool) -> &'static str {
        let moving = view.motion[0].hypot(view.motion[2]) > 0.035;
        if wet && let Some(swim) = self.swim {
            return if self.aquatic && !moving {
                "idle"
            } else {
                swim
            };
        }
        if !view.on_ground
            && let Some((flap, cruise)) = self.flight
        {
            return if view.motion[1] > 0.1 { flap } else { cruise };
        }
        if moving || self.aquatic {
            self.walk
        } else {
            "idle"
        }
    }
}

pub fn register(registry: &mut Registry<'_>) -> Result<(), RegistrationError> {
    let missing = RegistrationError::InvalidDefinition;
    let (mobs, items, ai, commands) = (
        MOBS.get().ok_or(missing)?,
        ITEMS.get().ok_or(missing)?,
        MOB_AI.get().ok_or(missing)?,
        COMMANDS.get().ok_or(missing)?,
    );
    for species in &SPECIES {
        let kind = format!("saintsdragons:{}", species.name);
        mobs.add_mob(
            registry,
            &Mob {
                id: &kind,
                carrier: &format!("minecraft:{}", species.carrier),
                attributes: &[
                    ("minecraft:max_health", species.health),
                    ("minecraft:movement_speed", species.speed),
                ],
            },
        )?;
        mobs.on_death(registry, &kind, taming::death)?;
        items.add_item(
            registry,
            &Item {
                id: &format!("{kind}_display"),
                display_name: species.display,
                carrier: "minecraft:paper",
                model: if species.aquatic {
                    "minecraft:item/cod"
                } else {
                    "minecraft:item/dragon_head"
                },
            },
        )?;
        if !species.aquatic {
            ai.replace_goals(registry, &kind)?;
            for (key, priority, controls) in [
                (ai::IDLE_LOOK, 3, goal_control::LOOK),
                (ai::GROUND_WANDER, 4, goal_control::MOVE),
            ] {
                ai.add_goal(
                    registry,
                    &Goal {
                        kind: &kind,
                        key,
                        priority,
                        controls,
                    },
                )?;
            }
        }
        if species.flight.is_some() {
            ai.add_goal(
                registry,
                &Goal {
                    kind: &kind,
                    key: flight::GOAL,
                    priority: 2,
                    controls: goal_control::MOVE | goal_control::LOOK,
                },
            )?;
        }
        ai.add_goal(
            registry,
            &Goal {
                kind: &kind,
                key: AMBIENT,
                priority: 0,
                controls: 0,
            },
        )?;
    }
    for (name, display) in names() {
        items.add_item(
            registry,
            &Item {
                id: &format!("saintsdragons:{name}_spawn_egg"),
                display_name: &format!("{display} Spawn Egg"),
                carrier: "minecraft:paper",
                model: "minecraft:item/chicken_spawn_egg",
            },
        )?;
    }
    INTERACTION
        .get()
        .ok_or(missing)?
        .on_use_on_block(registry, use_egg)?;
    let mut nodes = vec![crate::literal("spawn")];
    for (index, (name, _)) in names().enumerate() {
        let base = nodes.len() as u32;
        nodes.push(Node {
            parent: Some(0),
            name,
            argument: Argument::Literal,
            tag: None,
        });
        nodes.extend([
            crate::number(Some(base), "x", None),
            crate::number(Some(base + 1), "y", None),
            crate::number(Some(base + 2), "z", Some(SPAWN_TAG + index as u64)),
        ]);
    }
    commands.add(
        registry,
        "saintsdragons",
        "Spawn Saint's Dragons creatures",
        2,
        &nodes,
    )
}

fn names() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once(("cindervane", "Cindervane")).chain(
        SPECIES
            .iter()
            .map(|species| (species.name, species.display)),
    )
}

pub fn spawn_kind(tag: u64) -> Option<String> {
    let index = usize::try_from(tag.checked_sub(SPAWN_TAG)?).ok()?;
    if index == 0 {
        return Some(crate::CINDERVANE.to_owned());
    }
    Some(format!("saintsdragons:{}", SPECIES.get(index - 1)?.name))
}

pub fn spawn(world: World<'_>, kind: &str, at: [f64; 3]) -> Option<i32> {
    let mobs = MOBS.get()?;
    let id = mobs.spawn(world, kind, at)?;
    if let Some(entities) = ENTITIES.get() {
        entities.with_entity(world, id, |mob| mobs.set_state(mob, b"wild"));
    }
    Some(id)
}

fn use_egg(context: &mut UseOnBlock<'_>) -> UseResult {
    let Some(kind) = context.item.strip_suffix("_spawn_egg") else {
        return UseResult::Pass;
    };
    let Some(name) = kind.strip_prefix("saintsdragons:") else {
        return UseResult::Pass;
    };
    if !names().any(|(known, _)| name == known) || context.stack.count() == 0 {
        return UseResult::Pass;
    }
    let Some(&[dx, dy, dz]) = [
        [0, -1, 0],
        [0, 1, 0],
        [0, 0, -1],
        [0, 0, 1],
        [-1, 0, 0],
        [1, 0, 0],
    ]
    .get(usize::from(context.face)) else {
        return UseResult::Fail;
    };
    let world = context.world;
    let clicked = context.position;
    let adjacent = Pos {
        x: clicked.x + dx,
        y: clicked.y + dy,
        z: clicked.z + dz,
    };
    let empty = |pos| {
        world
            .collision_shape(pos)
            .is_some_and(|shape| shape.is_empty())
    };
    let pos = if empty(clicked) {
        clicked
    } else if empty(adjacent) {
        adjacent
    } else {
        return UseResult::Fail;
    };
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    if spawn(world, kind, at).is_none() {
        return UseResult::Fail;
    }
    if !context.creative {
        context.stack.shrink(1);
    }
    UseResult::Success
}

pub fn ambient(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    if phase != goal_phase::TICK {
        return matches!(phase, goal_phase::CAN_START | goal_phase::CONTINUE);
    }
    let (Some(kind), Some(view)) = (MOBS.get().and_then(|mobs| mobs.kind(mob)), mob.view()) else {
        return false;
    };
    let Some(name) = kind.strip_prefix("saintsdragons:") else {
        return false;
    };
    let Some(species) = SPECIES.iter().find(|species| species.name == name) else {
        return false;
    };
    let wet = species.swim.is_some()
        && matches!(
            world.fluid_id(flight::cell(view.position)).as_deref(),
            Some("minecraft:water" | "minecraft:flowing_water")
        );
    visuals::tick_species(world, mob, name, view, species.clip(view, wet));
    false
}
