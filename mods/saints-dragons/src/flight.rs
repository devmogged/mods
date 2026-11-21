//! Autonomous flight for an adult wild Cindervane.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use moddedmc_api::{
    Pos,
    content::World,
    entities::{Entity, EntityBody, EntityChange, SET_BODY_YAW, SET_HEAD_YAW, SET_MOTION, SET_YAW},
    mob_ai::goal_phase,
    tick_count,
};

use crate::{MOB_AI, MOBS, ai::random, taming};

pub const GOAL: u64 = 7;
const CRUISE_SPEED: f64 = 1.25;
const CLEARANCE: i32 = 8;
const CRUISE_HEIGHT: f64 = 10.0;
const CRUISE_RADIUS: f64 = 16.0;
const LANDING_COOLDOWN: u64 = 40;

struct Flight {
    home: [f64; 3],
    heading: f64,
    start: u64,
    landing: bool,
}

#[derive(Default)]
struct FlightState {
    active: Option<Flight>,
    retry: u64,
}

static FLIGHTS: Mutex<Option<HashMap<i32, FlightState>>> = Mutex::new(None);

pub fn forget(id: i32) {
    if let Some(states) = FLIGHTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        states.remove(&id);
    }
}

pub fn active(id: i32) -> bool {
    landing(id).is_some()
}

/// Whether autonomous flight is heading back to land, if a flight is active.
pub fn landing(id: i32) -> Option<bool> {
    FLIGHTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|states| states.get(&id))
        .and_then(|state| state.active.as_ref())
        .map(|flight| flight.landing)
}

pub fn cell(point: [f64; 3]) -> Pos {
    let [x, y, z] = point.map(|coordinate| coordinate.floor() as i32);
    Pos { x, y, z }
}

fn clear_body(world: World<'_>, body: EntityBody, offset: [f64; 3]) -> bool {
    let min = cell(add(body.min, offset));
    let max = cell(add(body.max.map(|v| v - 1.0e-5), offset));
    (min.x..=max.x).all(|x| {
        (min.y..=max.y).all(|y| {
            (min.z..=max.z).all(|z| {
                world
                    .collision_shape(Pos { x, y, z })
                    .is_some_and(|shape| shape.is_empty())
            })
        })
    })
}

pub fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}

pub fn offset(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

pub fn length_squared(v: [f64; 3]) -> f64 {
    v.into_iter().map(|component| component * component).sum()
}

pub fn goal(phase: u32, world: World<'_>, mob: Entity<'_>) -> bool {
    let (Some(ai), Some(mobs), Some(view)) = (MOB_AI.get(), MOBS.get(), mob.view()) else {
        return false;
    };
    let now = tick_count();
    let mut flights = FLIGHTS.lock().unwrap_or_else(PoisonError::into_inner);
    let state = flights.get_or_insert_default().entry(mob.id()).or_default();
    match phase {
        goal_phase::CAN_START => {
            if !view.on_ground
                || now < state.retry
                || ai.target(mob).is_some()
                || taming::is_tamed(mob)
                || world
                    .day_time()
                    .is_none_or(|time| (13000..23000).contains(&time.ticks.rem_euclid(24000)))
            {
                return false;
            }
            state.retry = now + 8;
            if random(40) != 0 {
                return false;
            }
            mob.body().is_some_and(|body| {
                (0..=CLEARANCE).all(|y| clear_body(world, body, [0.0, f64::from(y), 0.0]))
            }) && world.fluid_id(cell(view.position)).as_deref() == Some("minecraft:empty")
        }
        goal_phase::START => {
            state.active = Some(Flight {
                home: view.position,
                heading: f64::from(view.yaw).to_radians(),
                start: now,
                landing: false,
            });
            ai.stop(mob);
            false
        }
        goal_phase::CONTINUE => {
            state.active.is_some()
                && !taming::is_tamed(mob)
                && ai.target(mob).is_none()
                && mobs.passengers(mob).is_empty()
        }
        goal_phase::STOP => {
            state.active = None;
            state.retry = now + LANDING_COOLDOWN;
            false
        }
        goal_phase::TICK => {
            if let Some(flight) = state.active.as_mut()
                && !fly(world, mob, view.position, view.on_ground, flight, now)
            {
                state.active = None;
            }
            false
        }
        _ => false,
    }
}

fn fly(
    world: World<'_>,
    mob: Entity<'_>,
    at: [f64; 3],
    grounded: bool,
    flight: &mut Flight,
    now: u64,
) -> bool {
    let Some(body) = mob.body() else { return false };
    let age = now.saturating_sub(flight.start);
    if age >= 600 || (flight.landing && grounded) {
        return false;
    }
    // shortcut: orbit a loaded clearing and return to it. The original plans long cruises and
    // picks landing approaches with a separate navigation planner and weather profile.
    flight.landing |= age >= 260
        || world
            .day_time()
            .is_some_and(|time| (13000..23000).contains(&time.ticks.rem_euclid(24000)));
    let [x, y, z] = flight.home;
    let altitude = y + CRUISE_HEIGHT;
    let goal = if flight.landing {
        let horizontal = (x - at[0]).hypot(z - at[2]);
        [x, if horizontal < 1.0 { y - 1.0 } else { altitude }, z]
    } else if age < 24 {
        [x, altitude, z]
    } else {
        let point = [
            x - flight.heading.sin() * CRUISE_RADIUS,
            altitude,
            z + flight.heading.cos() * CRUISE_RADIUS,
        ];
        if length_squared(offset(point, at)) <= 9.0 {
            flight.heading += std::f64::consts::FRAC_PI_2;
        }
        point
    };
    let [dx, dy, dz] = offset(goal, at);
    let horizontal = dx.hypot(dz).max(CRUISE_SPEED);
    let motion = [
        dx / horizontal * CRUISE_SPEED,
        dy.clamp(-0.42, 0.45),
        dz / horizontal * CRUISE_SPEED,
    ];
    let motion = if motion[1] >= 0.0 && !clear_body(world, body, motion) {
        if flight.landing {
            return false;
        }
        flight.landing = true;
        [0.0, -0.18, 0.0]
    } else if motion[1] < 0.0 && !clear_body(world, body, [motion[0], 0.0, motion[2]]) {
        [0.0, motion[1], 0.0]
    } else {
        motion
    };
    let yaw = if dx.hypot(dz) > 0.1 {
        dz.atan2(dx).to_degrees() as f32 - 90.0
    } else {
        flight.heading.to_degrees() as f32
    };
    mob.change(&EntityChange {
        mask: SET_MOTION | SET_YAW | SET_BODY_YAW | SET_HEAD_YAW,
        motion,
        yaw,
        body_yaw: yaw,
        head_yaw: yaw,
        ..EntityChange::default()
    });
    true
}
