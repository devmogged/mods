//! Adult Cindervane interaction, following, and rider movement.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use fastnbt::Value;
use moddedmc_api::{
    content::World,
    effects::SOUND_NEUTRAL,
    entities::{Entity, EntityChange, SET_BODY_YAW, SET_HEAD_YAW, SET_MOTION, SET_YAW},
    interaction::{UseMob, UseResult},
    mob_ai::goal_phase,
    mobs::{PLAYER_INPUT_JUMP, PLAYER_INPUT_SPRINT},
    tick_count,
};

use crate::{EFFECTS, ENTITIES, MOB_AI, MOBS, STACKS, TAGS, ai::random};

pub const RIDE_OR_SIT: u64 = 5;
pub const FOLLOW: u64 = 6;
const FOOD_TAG: &str = "saintsdragons:foods/cindervane";
pub const HEARTY_MEAL: &str = "saintsdragons:hearty_dragon_meal";
const SADDLE: &str = "minecraft:saddle";
const FEED_COOLDOWN: u64 = 44;

// Serializes player clicks and keeps the short feeding cooldown off disk.
static FED: Mutex<Option<HashMap<i32, u64>>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Follow,
    Sit,
    Wander,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tamed {
    owner: [u8; 16],
    order: Order,
    saddle: bool,
}

impl Tamed {
    fn decode(bytes: &[u8]) -> Option<Self> {
        let [1, order, saddle @ (0 | 1), owner @ ..] = bytes else {
            return None;
        };
        let order = match order {
            0 => Order::Follow,
            1 => Order::Sit,
            2 => Order::Wander,
            _ => return None,
        };
        Some(Self {
            owner: owner.try_into().ok()?,
            order,
            saddle: *saddle == 1,
        })
    }

    fn encode(self) -> [u8; 19] {
        let mut data = [0; 19];
        data[0] = 1;
        data[1] = match self.order {
            Order::Follow => 0,
            Order::Sit => 1,
            Order::Wander => 2,
        };
        data[2] = u8::from(self.saddle);
        data[3..].copy_from_slice(&self.owner);
        data
    }
}

fn tamed(mob: Entity<'_>) -> Option<Tamed> {
    Tamed::decode(&MOBS.get()?.state(mob)?)
}

pub fn is_tamed(mob: Entity<'_>) -> bool {
    tamed(mob).is_some()
}

pub fn is_kin(mob: Entity<'_>, other: Entity<'_>) -> bool {
    other.type_id() == crate::CINDERVANE_CARRIER.trim_start_matches("minecraft:")
        && MOBS
            .get()
            .is_some_and(|mobs| mobs.kind(other) == Some(crate::CINDERVANE))
        && tamed(mob).map(|state| state.owner) == tamed(other).map(|state| state.owner)
}

pub fn describe(bytes: &[u8]) -> &'static str {
    match Tamed::decode(bytes) {
        Some(state) if state.saddle => "tamed+saddled",
        Some(_) => "tamed",
        None => "wild",
    }
}

pub fn is_owner(mob: Entity<'_>, entity: Entity<'_>) -> bool {
    tamed(mob).is_some_and(|state| MOBS.get().and_then(|m| m.uuid(entity)) == Some(state.owner))
}

pub fn interact(click: &mut UseMob<'_>) -> UseResult {
    let (Some(player), Some(mob), Some(mobs), Some(ai), Some(entities), Some(tags)) = (
        click.player,
        click.mob,
        MOBS.get(),
        MOB_AI.get(),
        ENTITIES.get(),
        TAGS.get(),
    ) else {
        return UseResult::Pass;
    };
    let Some(uuid) = mobs.uuid(player) else {
        return UseResult::Pass;
    };
    if mobs.health(mob).is_none_or(|(health, _)| health <= 0.0) {
        return UseResult::Pass;
    }
    let held = click
        .stack
        .read()
        .and_then(|bytes| fastnbt::from_bytes::<Value>(&bytes).ok());
    let item = match held.as_ref() {
        Some(Value::Compound(nbt)) => match nbt.get("id") {
            Some(Value::String(id)) => id.as_str(),
            _ => "",
        },
        _ => "",
    };
    let mut lock = FED.lock().unwrap_or_else(PoisonError::into_inner);
    let fed = lock.get_or_insert_default();
    let state = tamed(mob);

    if tags.contains("item", FOOD_TAG, item) {
        // shortcut: breeding and hunger are separate mechanics. Crouching with food on a tame
        // dragon is reserved for breeding.
        if click.sneaking && state.is_some() {
            return UseResult::Consume;
        }
        let now = tick_count();
        fed.retain(|_, until| *until > now);
        if fed.contains_key(&mob.id()) {
            mobs.message(player, "Cindervane is still eating.", true);
            return UseResult::Consume;
        }
        fed.insert(mob.id(), now + FEED_COOLDOWN);
        if !click.creative {
            click.stack.shrink(1);
        }
        let hearty = item == HEARTY_MEAL;
        if hearty {
            mob.add_effect("minecraft:regeneration", 200, 1, false, true);
        }
        if state.is_some() {
            mobs.heal(mob, if hearty { 15.0 } else { 5.0 });
            feed_particles(click.world, mob, true);
        } else {
            let chance = match item {
                HEARTY_MEAL => 500_000,
                "minecraft:chicken" => 333_333,
                _ => 250_000,
            };
            let success = random(1_000_000) < chance
                && mobs.set_state(
                    mob,
                    &Tamed {
                        owner: uuid,
                        order: Order::Sit,
                        saddle: false,
                    }
                    .encode(),
                );
            if success {
                mobs.make_persistent(mob);
                ai.stop(mob);
                ai.set_target(click.world, mob, None);
            }
            feed_particles(click.world, mob, success);
        }
        return UseResult::Success;
    }

    let Some(mut state) = state.filter(|_| click.main_hand) else {
        return UseResult::Pass;
    };
    let owner = uuid == state.owner;
    if owner && item == SADDLE && !state.saddle {
        state.saddle = true;
        if !mobs.set_state(mob, &state.encode()) {
            return UseResult::Fail;
        }
        if !click.creative {
            click.stack.shrink(1);
        }
        if let (Some(effects), Some(view)) = (EFFECTS.get(), mob.view()) {
            effects.play_sound(
                click.world,
                view.position,
                "minecraft:entity.horse.saddle",
                SOUND_NEUTRAL,
                1.0,
                1.0,
            );
        }
        return UseResult::Success;
    }
    if click.sneaking {
        if !owner {
            return UseResult::Pass;
        }
        let (order, label) = match state.order {
            Order::Follow => (Order::Sit, "sitting"),
            Order::Sit => (Order::Wander, "wandering"),
            Order::Wander => (Order::Follow, "following"),
        };
        state.order = order;
        if !mobs.set_state(mob, &state.encode()) {
            return UseResult::Fail;
        }
        ai.stop(mob);
        mobs.message(player, &format!("Cindervane is {label}."), true);
        return UseResult::Success;
    }
    if !state.saddle {
        mobs.message(player, "Cindervane needs a saddle.", true);
        return UseResult::Consume;
    }

    let passengers = mobs.passengers(mob);
    let can_mount = if owner {
        passengers.is_empty()
    } else {
        let [first] = passengers.as_slice() else {
            return UseResult::Fail;
        };
        entities.with_entity(click.world, *first, |rider| mobs.uuid(rider))
            == Some(Some(state.owner))
    };
    if !can_mount || !player.start_riding(mob.id(), !owner) {
        return UseResult::Fail;
    }
    if owner {
        if state.order == Order::Sit {
            state.order = Order::Follow;
            mobs.set_state(mob, &state.encode());
        }
        ai.stop(mob);
        ai.set_target(click.world, mob, None);
    }
    UseResult::Success
}

fn feed_particles(world: World<'_>, mob: Entity<'_>, happy: bool) {
    let (Some(effects), Some(view)) = (EFFECTS.get(), mob.view()) else {
        return;
    };
    // The carrier may not react to the taming status packet.
    let [x, y, z] = view.position;
    let particle = if happy {
        "minecraft:heart"
    } else {
        "minecraft:smoke"
    };
    effects.particles(world, particle, [x, y + 1.5, z], [0.6, 0.5, 0.6], 0.0, 7);
}

pub fn death(world: World<'_>, mob: Entity<'_>) {
    crate::flight::forget(mob.id());
    crate::visuals::death(world, mob);
    {
        let mut lock = FED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(fed) = lock.as_mut() {
            fed.remove(&mob.id());
        }
        let (Some(mobs), Some(mut state)) = (MOBS.get(), tamed(mob).filter(|state| state.saddle))
        else {
            return;
        };
        state.saddle = false;
        if !mobs.set_state(mob, &state.encode()) {
            return;
        }
    }
    let (Some(stacks), Some(view)) = (STACKS.get(), mob.view()) else {
        return;
    };
    if let Ok(stack) = fastnbt::to_bytes(&HashMap::from([
        ("id", Value::String(SADDLE.to_owned())),
        ("count", Value::Int(1)),
    ])) {
        stacks.spawn(world, view.position, [0.0; 3], &stack);
    }
}

pub fn ride_or_sit(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let (Some(mobs), Some(ai), Some(entities), Some(state)) =
        (MOBS.get(), MOB_AI.get(), ENTITIES.get(), tamed(mob))
    else {
        return false;
    };
    let passengers = mobs.passengers(mob);
    let active = state.order == Order::Sit
        || passengers
            .first()
            .is_some_and(|&id| !crate::combat::holding(mob, id));
    match phase {
        goal_phase::CAN_START | goal_phase::CONTINUE => active,
        goal_phase::START => {
            ai.stop(mob);
            ai.set_target(world, mob, None);
            false
        }
        goal_phase::TICK => {
            if let Some(&first) = passengers.first() {
                entities.with_entity(world, first, |rider| {
                    if mobs.uuid(rider) == Some(state.owner) {
                        ride(mob, rider);
                    } else {
                        rider.stop_riding();
                    }
                });
            }
            false
        }
        _ => false,
    }
}

fn ride(mob: Entity<'_>, rider: Entity<'_>) {
    let (Some(mobs), Some(view), Some(look), Some(body)) =
        (MOBS.get(), mob.view(), rider.view(), rider.body())
    else {
        return;
    };
    let keys = mobs.player_input(rider).unwrap_or(0);
    let sprint = keys & PLAYER_INPUT_SPRINT != 0;
    let jump = keys & PLAYER_INPUT_JUMP != 0;
    let yaw = f64::from(look.yaw).to_radians();
    let strafe = body.input[0] * 0.4;
    let forward = body.input[2];
    // shortcut: stock clients use jump and look pitch for vertical control. The Java release
    // also sends custom descend, dive and flight mode packets.
    let air = jump || !view.on_ground;
    let speed = match (air, sprint) {
        (true, true) => 0.60 * 4.0,
        (true, false) => 0.60 * 3.0,
        (false, true) => 0.45,
        (false, false) => 0.18,
    };
    let desired_x = (strafe * yaw.cos() - forward * yaw.sin()) * speed;
    let desired_z = (strafe * yaw.sin() + forward * yaw.cos()) * speed;
    let vertical = if jump {
        0.45
    } else if air {
        let pitch = f64::from(look.pitch).to_radians();
        (-pitch.sin() * forward.abs() * 0.85).clamp(-0.85, 0.45) - 0.08
    } else {
        view.motion[1].min(0.0)
    };
    let horizontal = |current: f64, desired: f64| {
        if air {
            current + (desired - current) * 0.35
        } else {
            desired
        }
    };
    mob.change(&EntityChange {
        mask: SET_YAW | SET_BODY_YAW | SET_HEAD_YAW | SET_MOTION,
        yaw: look.yaw,
        body_yaw: look.yaw,
        head_yaw: look.yaw,
        motion: [
            horizontal(view.motion[0], desired_x),
            vertical,
            horizontal(view.motion[2], desired_z),
        ],
        ..EntityChange::default()
    });
    mob.reset_fall_distance();
    rider.reset_fall_distance();
}

pub fn follow(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let (Some(mobs), Some(ai), Some(entities), Some(state)) =
        (MOBS.get(), MOB_AI.get(), ENTITIES.get(), tamed(mob))
    else {
        return false;
    };
    if phase == goal_phase::STOP {
        ai.stop(mob);
        return false;
    }
    let Some(at) = mob.view().map(|view| view.position) else {
        return false;
    };
    let Some(owner) = mobs
        .by_uuid(world, &state.owner)
        .filter(|_| state.order == Order::Follow)
        .and_then(|id| {
            entities
                .with_entity(world, id, |e| e.view().map(|v| v.position))
                .flatten()
        })
    else {
        return false;
    };
    let distance = at
        .iter()
        .zip(owner)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>();
    match phase {
        goal_phase::CAN_START => distance > 6.0_f64.powi(2),
        goal_phase::CONTINUE => distance > 3.0_f64.powi(2),
        goal_phase::START | goal_phase::TICK => {
            if phase == goal_phase::START || tick_count().is_multiple_of(10) {
                ai.walk_to(mob, owner, 1.0);
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tame_state_round_trips_and_reads_old_wild_state() {
        assert_eq!(Tamed::decode(b"wild"), None);
        assert_eq!(Tamed::decode(&[]), None);
        for order in [Order::Follow, Order::Sit, Order::Wander] {
            let state = Tamed {
                owner: [0x5a; 16],
                order,
                saddle: true,
            };
            let data = state.encode();
            assert_eq!(Tamed::decode(&data), Some(state));
            assert_eq!(Tamed::decode(&data[..18]), None);
        }
    }
}
