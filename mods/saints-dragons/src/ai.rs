//! Cindervane's ground AI from `CindervaneBrain`: `DragonIdleLookBehaviour(8)` and
//! `DragonGroundWanderBehaviour(GROUND_WANDER_SPEED, 160)`; combat and owner behavior have
//! their own goals.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use moddedmc_api::{
    content::World,
    entities::{ENTITY_PLAYER, Entity},
    mob_ai::{GoalHandler, goal_phase},
};

use crate::{ENTITIES, MOB_AI};

pub const IDLE_LOOK: u64 = 1;
pub const GROUND_WANDER: u64 = 2;
pub const AMBIENT: u64 = 8;

/// `DragonIdleLookBehaviour` player range.
const LOOK_RANGE: f64 = 8.0;
/// `CindervaneStatProfile.Brain.GROUND_WANDER_SPEED`.
const WANDER_SPEED: f64 = 1.0;
/// `DragonGroundWanderBehaviour` interval: one chance in this many ticks.
const WANDER_INTERVAL: u64 = 160;
/// `DefaultRandomPos.getPos(dragon, 20, 8)`.
const WANDER_HORIZONTAL: i64 = 20;
const WANDER_VERTICAL: i64 = 8;
/// Vanilla's default eye height, as a share of the box height.
const EYE: f64 = 0.85;

#[derive(Default)]
struct Look {
    ticks: i32,
    player: Option<i32>,
    yaw: f32,
}

// shortcut: per-mob entries stay until the server stops; drop them on death once mods hear
// about it (step 3).
static LOOKS: Mutex<Option<HashMap<i32, Look>>> = Mutex::new(None);
static WANDER_TARGETS: Mutex<Option<HashMap<i32, [f64; 3]>>> = Mutex::new(None);
/// Mobs whose goals have run since the server started; `/cindervane find` reports it.
static SEEN: Mutex<Option<HashSet<i32>>> = Mutex::new(None);
static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

pub const HANDLER: GoalHandler = goal;

pub fn seen(id: i32) -> bool {
    SEEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .is_some_and(|seen| seen.contains(&id))
}

/// A random number below `bound` (xorshift; the dragon's own `RandomSource` in Java).
pub fn random(bound: u64) -> u64 {
    let mut next = 0;
    let _ = SEED.try_update(Ordering::Relaxed, Ordering::Relaxed, |mut x| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        next = x;
        Some(x)
    });
    next % bound.max(1)
}

fn goal(phase: u32, key: u64, world: World<'_>, mob: Entity<'_>) -> bool {
    SEEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_default()
        .insert(mob.id());
    match key {
        IDLE_LOOK => idle_look(phase, world, mob),
        GROUND_WANDER => ground_wander(phase, mob),
        crate::combat::TARGETING => crate::combat::targeting(phase, world, mob),
        crate::combat::GROUND_COMBAT => crate::combat::ground_combat(phase, world, mob),
        crate::taming::RIDE_OR_SIT => crate::taming::ride_or_sit(phase, world, mob),
        crate::taming::FOLLOW => crate::taming::follow(phase, world, mob),
        crate::flight::GOAL => crate::flight::goal(phase, world, mob),
        crate::roster::AMBIENT => crate::roster::ambient(phase, world, mob),
        AMBIENT => match phase {
            goal_phase::CAN_START | goal_phase::CONTINUE => true,
            goal_phase::TICK => {
                crate::combat::ambient(world, mob);
                crate::visuals::tick(world, mob);
                false
            }
            _ => false,
        },
        _ => false,
    }
}

fn idle_look(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let (Some(ai), Some(entities)) = (MOB_AI.get(), ENTITIES.get()) else {
        return false;
    };
    if phase != goal_phase::TICK {
        return matches!(phase, goal_phase::CAN_START | goal_phase::CONTINUE);
    }
    let (Some(view), Some(body)) = (mob.view(), mob.body()) else {
        return false;
    };
    if ai.target(mob).is_some() {
        return false;
    }
    let eye = [
        view.position[0],
        view.position[1] + (body.max[1] - body.min[1]) * EYE,
        view.position[2],
    ];
    let near = |p: [f64; 3]| {
        let d = [p[0] - eye[0], p[1] - view.position[1], p[2] - eye[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= LOOK_RANGE * LOOK_RANGE
    };
    let mut looks = LOOKS.lock().unwrap_or_else(PoisonError::into_inner);
    let look = looks.get_or_insert_default().entry(mob.id()).or_default();
    look.ticks -= 1;
    if look.ticks < 0 {
        let min = view.position.map(|c| c - LOOK_RANGE);
        let max = view.position.map(|c| c + LOOK_RANGE);
        // shortcut: nearest player by distance without vanilla's line-of-sight check; add one
        // when the host exposes `canSee`.
        look.player = entities
            .entities_in(world, min, max, mob.id())
            .into_iter()
            .filter_map(|id| {
                entities.with_entity(world, id, |e| {
                    let v = e.view()?;
                    (v.kind & ENTITY_PLAYER != 0 && near(v.position)).then_some((id, v.position))
                })?
            })
            .min_by(|(_, a), (_, b)| {
                let d = |p: &[f64; 3]| (p[0] - eye[0]).powi(2) + (p[2] - eye[2]).powi(2);
                d(a).total_cmp(&d(b))
            })
            .map(|(id, _)| id);
        look.ticks = 40 + random(40) as i32;
        look.yaw = view.yaw + random(1 << 24) as f32 / (1 << 24) as f32 * 180.0 - 90.0;
    }
    let player = look.player.and_then(|id| {
        entities.with_entity(world, id, |e| {
            let v = e.view()?;
            near(v.position).then_some([v.position[0], v.position[1] + 1.62, v.position[2]])
        })?
    });
    let at = player.unwrap_or_else(|| {
        let radians = f64::from(look.yaw).to_radians();
        [
            view.position[0] - radians.sin() * 4.0,
            eye[1],
            view.position[2] + radians.cos() * 4.0,
        ]
    });
    drop(looks);
    ai.look_at(mob, at);
    false
}

fn ground_wander(phase: u32, mob: Entity<'_>) -> bool {
    let Some(ai) = MOB_AI.get() else { return false };
    let mut targets = WANDER_TARGETS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let targets = targets.get_or_insert_default();
    match phase {
        goal_phase::CAN_START => {
            if random(WANDER_INTERVAL) != 0 {
                return false;
            }
            let Some(view) = mob.view() else { return false };
            let offset = |range: i64| (random(2 * range as u64 + 1) as i64 - range) as f64;
            // shortcut: a uniform offset; `DefaultRandomPos` also weighs and lifts it out of
            // solid blocks. The navigator rejects unreachable ones.
            let target = [
                view.position[0] + offset(WANDER_HORIZONTAL),
                view.position[1] + offset(WANDER_VERTICAL),
                view.position[2] + offset(WANDER_HORIZONTAL),
            ];
            targets.insert(mob.id(), target);
            true
        }
        goal_phase::CONTINUE => ai.navigating(mob),
        goal_phase::START => {
            if let Some(target) = targets.get(&mob.id()) {
                ai.walk_to(mob, *target, WANDER_SPEED);
            }
            false
        }
        goal_phase::STOP => {
            targets.remove(&mob.id());
            ai.stop(mob);
            false
        }
        _ => false,
    }
}
