//! Cindervane retaliation, attack selection and navigation.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use moddedmc_api::{
    content::World,
    entities::{ENTITY_ALIVE, Entity, EntityChange, SET_BODY_YAW, SET_YAW},
    mob_ai::goal_phase,
};

use crate::{
    ENTITIES, MOB_AI, MOBS,
    abilities::{self, Abilities, Attack},
};

pub const TARGETING: u64 = 3;
pub const GROUND_COMBAT: u64 = 4;

/// `Cindervane.hurt` ignores these.
const IMMUNE: &[&str] = &[
    "minecraft:in_fire",
    "minecraft:on_fire",
    "minecraft:lava",
    "minecraft:hot_floor",
    "minecraft:fall",
];

/// `CindervaneTargetingBehaviour.followRange`: `max(32, FOLLOW_RANGE)`; the attribute is 64.
const FOLLOW_RANGE: f64 = 64.0;
/// Cindervane's box width; its carrier's is narrower.
const WIDTH: f64 = 2.5;
/// `CindervaneStatProfile.GroundCombatBehaviour`.
const CHASE_SPEED: f64 = 1.15;
const MELEE_STOP_RANGE: f64 = 5.0;
const TURN_RANGE: f64 = 32.0;
const CLOSE_FACING_DOT: f64 = 0.35;
const DECISION_INTERVAL: i32 = 6;
const TURN_DEGREES: f32 = 12.0;
/// How often a chase re-paths to where the target is now.
const REPATH_TICKS: i32 = 5;

#[derive(Default)]
struct Combat {
    /// The last `getLastHurtByMobTimestamp` acted on.
    last_hurt: Option<i32>,
    decision: i32,
    repath: i32,
    abilities: Abilities,
}

// shortcut: per-mob entries stay until the server stops, like ai.rs's.
static COMBAT: Mutex<Option<HashMap<i32, Combat>>> = Mutex::new(None);

fn with_combat<T>(mob: Entity<'_>, f: impl FnOnce(&mut Combat) -> T) -> T {
    let mut all = COMBAT.lock().unwrap_or_else(PoisonError::into_inner);
    f(all.get_or_insert_default().entry(mob.id()).or_default())
}

pub fn hurt(_world: World<'_>, mob: Entity<'_>, damage_type: &str, _amount: f32) -> bool {
    let immune = IMMUNE.contains(&damage_type);
    if immune
        && damage_type != "minecraft:fall"
        && let Some(mobs) = MOBS.get()
    {
        mobs.set_on_fire(mob, 0);
    }
    !immune
}

/// Where a live, loaded entity is, and its box width.
fn locate(world: World<'_>, id: i32) -> Option<([f64; 3], f64)> {
    ENTITIES.get()?.with_entity(world, id, |e| {
        let (view, body) = (e.view()?, e.body()?);
        (e.flags() & ENTITY_ALIVE != 0).then_some((view.position, body.max[0] - body.min[0]))
    })?
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

pub fn targeting(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let Some(ai) = MOB_AI.get() else { return false };
    if phase != goal_phase::TICK {
        return matches!(phase, goal_phase::CAN_START | goal_phase::CONTINUE);
    }
    let Some(view) = mob.view() else { return false };
    let Some(entities) = ENTITIES.get() else {
        return false;
    };
    let usable = |id| {
        entities
            .with_entity(world, id, |target| {
                abilities::enemy(mob, target) || holding(mob, id)
            })
            .unwrap_or(false)
    };
    if let Some(target) = ai.target(mob) {
        let kept = usable(target)
            && locate(world, target)
                .is_some_and(|(at, _)| distance(at, view.position) <= FOLLOW_RANGE);
        if !kept {
            ai.set_target(world, mob, None);
        }
    }
    if let Some((attacker, timestamp)) = ai.last_hurt_by(mob) {
        let new = with_combat(mob, |c| c.last_hurt.replace(timestamp) != Some(timestamp));
        if new && usable(attacker) {
            ai.set_target(world, mob, Some(attacker));
        }
    }
    false
}

pub fn ground_combat(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let Some(ai) = MOB_AI.get() else { return false };
    let target = ai.target(mob).and_then(|id| Some((id, locate(world, id)?)));
    let Some(view) = mob.view() else { return false };
    let in_range = target.is_some_and(|(_, (at, _))| distance(at, view.position) <= FOLLOW_RANGE);
    match phase {
        goal_phase::CAN_START => in_range,
        goal_phase::CONTINUE => in_range || with_combat(mob, |c| c.abilities.busy()),
        goal_phase::START => {
            with_combat(mob, |c| {
                c.decision = 0;
                c.repath = 0;
            });
            false
        }
        goal_phase::STOP => {
            let mut attacks = with_combat(mob, |c| std::mem::take(&mut c.abilities));
            attacks.cancel(world, mob);
            with_combat(mob, |c| c.abilities = attacks);
            ai.stop(mob);
            false
        }
        goal_phase::TICK => {
            tick(world, mob, view.position, view.yaw, target);
            false
        }
        _ => false,
    }
}

type Located = (i32, ([f64; 3], f64));

fn tick(world: World<'_>, mob: Entity<'_>, position: [f64; 3], yaw: f32, target: Option<Located>) {
    let Some(ai) = MOB_AI.get() else { return };
    let mut attacks = with_combat(mob, |c| std::mem::take(&mut c.abilities));
    if attacks.busy() {
        attacks.tick_attack(world, mob);
        with_combat(mob, |c| c.abilities = attacks);
        ai.stop(mob);
        return;
    }
    with_combat(mob, |c| c.abilities = attacks);
    let Some((target, (at, width))) = target else {
        return;
    };
    ai.look_at(mob, [at[0], at[1] + 1.0, at[2]]);
    let gap = (distance(at, position) - (WIDTH + width) * 0.5).max(0.0);
    let (dx, dz) = (at[0] - position[0], at[2] - position[2]);
    if gap <= TURN_RANGE && dx * dx + dz * dz > 1.0e-6 {
        let toward = dz.atan2(dx).to_degrees() as f32 - 90.0;
        let turned = yaw + (wrap(toward - yaw)).clamp(-TURN_DEGREES, TURN_DEGREES);
        mob.change(&EntityChange {
            mask: SET_YAW | SET_BODY_YAW,
            yaw: turned,
            body_yaw: turned,
            ..EntityChange::default()
        });
    }
    // `SetWalkTargetToAttackTargetBehaviour`: walk until within the stop range.
    if gap > MELEE_STOP_RANGE {
        let repath = with_combat(mob, |c| {
            c.repath -= 1;
            if c.repath > 0 {
                return false;
            }
            c.repath = REPATH_TICKS;
            true
        });
        if repath || !ai.navigating(mob) {
            ai.walk_to(mob, at, CHASE_SPEED);
        }
    } else {
        ai.stop(mob);
    }
    let started = with_combat(mob, |c| {
        if c.decision > 0 {
            c.decision -= 1;
            return false;
        }
        c.decision = DECISION_INTERVAL;
        c.abilities
            .choose(moddedmc_api::tick_count(), target, gap, facing(yaw, dx, dz))
    });
    if started {
        ai.stop(mob);
    }
}

pub fn ambient(world: World<'_>, mob: Entity<'_>) {
    let (mut shots, fire) = with_combat(mob, |c| c.abilities.take_effects());
    if shots.is_empty() && !fire {
        return;
    }
    abilities::effects(world, mob, &mut shots, fire);
    with_combat(mob, |c| c.abilities.keep_shots(shots));
}

pub fn holding(mob: Entity<'_>, rider: i32) -> bool {
    with_combat(mob, |c| c.abilities.holding(rider))
}

pub fn last(mob: Entity<'_>) -> Option<&'static str> {
    with_combat(mob, |c| c.abilities.last.map(|attack| attack.name()))
}

pub fn fire_active(mob: Entity<'_>) -> bool {
    with_combat(mob, |c| c.abilities.fire_active())
}

pub fn active_attack(mob: Entity<'_>) -> Option<Attack> {
    with_combat(mob, |c| c.abilities.active())
}

/// `isFacingTarget` at close range: the yaw's direction against the horizontal way to it.
fn facing(yaw: f32, dx: f64, dz: f64) -> bool {
    let length = (dx * dx + dz * dz).sqrt();
    if length <= 1.0e-3 {
        return true;
    }
    let radians = f64::from(yaw).to_radians();
    (-radians.sin() * dx + radians.cos() * dz) / length >= CLOSE_FACING_DOT
}

/// `Mth.wrapDegrees`.
fn wrap(degrees: f32) -> f32 {
    let d = degrees % 360.0;
    if d >= 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_and_wrap() {
        // Yaw 0 looks along +z.
        assert!(facing(0.0, 0.0, 5.0));
        assert!(!facing(0.0, 0.0, -5.0));
        assert!(!facing(0.0, 5.0, 0.0));
        assert!(facing(-90.0, 5.0, 0.0));
        assert_eq!(wrap(350.0), -10.0);
        assert_eq!(wrap(-190.0), 170.0);
    }
}
