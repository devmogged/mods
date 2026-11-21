//! Cindervane's double bite, slash grab, Magma Volley and Fire Body.

use moddedmc_api::{
    content::World,
    entities::{
        ENTITY_ALIVE, ENTITY_LIVING, ENTITY_PLAYER, Entity, EntityBody, EntityChange, SET_MOTION,
    },
    tick_count,
};

use crate::{
    EFFECTS, ENTITIES, MOB_AI, MOBS,
    flight::{add, length_squared, offset},
    taming,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack {
    Bite,
    DoubleBite,
    SlashGrab,
    MagmaVolley,
}

impl Attack {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bite => "bite",
            Self::DoubleBite => "double-bite",
            Self::SlashGrab => "slash-grab",
            Self::MagmaVolley => "magma-volley",
        }
    }

    pub const fn timers(self) -> (u64, u64, u64) {
        match self {
            Self::Bite => (12, 20, 24),
            Self::DoubleBite => (16, 28, 80),
            Self::SlashGrab => (35, 40, 140),
            Self::MagmaVolley => (29, 55, 400),
        }
    }
}

const ATTACKS: [Attack; 4] = [
    Attack::Bite,
    Attack::DoubleBite,
    Attack::SlashGrab,
    Attack::MagmaVolley,
];

#[derive(Default)]
pub struct Abilities {
    running: Option<Running>,
    cooldown: [u64; 4],
    cadence: u64,
    fire_until: u64,
    fire_cooldown: u64,
    shots: Vec<Shot>,
    pub last: Option<Attack>,
}

struct Running {
    kind: Attack,
    target: i32,
    start: u64,
    held: bool,
}

pub struct Shot {
    position: [f64; 3],
    motion: [f64; 3],
    age: u16,
}

pub fn enemy(mob: Entity<'_>, target: Entity<'_>) -> bool {
    let (Some(mobs), Some(view)) = (MOBS.get(), target.view()) else {
        return false;
    };
    target.id() != mob.id()
        && view.kind & ENTITY_LIVING != 0
        && target.flags() & ENTITY_ALIVE != 0
        && !matches!(mobs.game_mode(target), Some(1 | 3))
        && (view.kind & ENTITY_PLAYER == 0 || !taming::is_owner(mob, target))
        && !taming::is_kin(mob, target)
        && !mob.has_indirect_passenger(target.id())
}

impl Abilities {
    pub const fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn active(&self) -> Option<Attack> {
        self.running.as_ref().map(|action| action.kind)
    }

    pub const fn holding(&self, id: i32) -> bool {
        matches!(&self.running, Some(action) if action.held && action.target == id)
    }

    pub fn fire_active(&self) -> bool {
        tick_count() < self.fire_until
    }

    pub fn choose(&mut self, now: u64, target: i32, gap: f64, facing: bool) -> bool {
        if gap <= 3.25 && now >= self.fire_cooldown {
            self.fire_until = now + 100;
            self.fire_cooldown = now + 240;
        }
        if !facing || now < self.cadence {
            return false;
        }
        let index = ATTACKS
            .into_iter()
            .enumerate()
            .filter_map(|(i, kind)| {
                if now < self.cooldown[i] {
                    return None;
                }
                let score = match kind {
                    Attack::Bite if gap <= 4.5 => 56.0 + (4.5 - gap) * 3.0,
                    Attack::DoubleBite if gap <= 4.75 => 92.0 + (4.75 - gap) * 6.0,
                    Attack::SlashGrab if (2.0..=6.25).contains(&gap) => {
                        84.0 + if gap >= 3.0 { 8.0 } else { -8.0 }
                    }
                    Attack::MagmaVolley if (8.0..=32.0).contains(&gap) => {
                        68.0 + (gap - 8.0).mul_add(0.8, 0.0).min(18.0)
                    }
                    _ => return None,
                };
                Some((i, score - if self.last == Some(kind) { 26.0 } else { 0.0 }))
            })
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(i, _)| i);
        let Some(i) = index else { return false };
        let kind = ATTACKS[i];
        let (_, cadence, cooldown) = kind.timers();
        self.running = Some(Running {
            kind,
            target,
            start: now,
            held: false,
        });
        self.cadence = now + cadence;
        self.cooldown[i] = now + cooldown;
        self.last = Some(kind);
        true
    }

    pub fn cancel(&mut self, world: World<'_>, mob: Entity<'_>) {
        if let Some(action) = self.running.take()
            && action.held
        {
            release(world, mob, action.target, false);
        }
        self.fire_until = 0;
    }

    pub fn tick_attack(&mut self, world: World<'_>, mob: Entity<'_>) {
        let Some(action) = self.running.as_mut() else {
            return;
        };
        let age = tick_count().saturating_sub(action.start);
        match (action.kind, age) {
            (Attack::Bite, 5) => melee(world, mob, action.target, 12.0, 4.5, false),
            (Attack::DoubleBite, 4 | 11) => melee(world, mob, action.target, 15.0, 4.75, age == 11),
            (Attack::SlashGrab, 16) => action.held = slash(world, mob, action.target, 5.0, true),
            (Attack::SlashGrab, 26) => {
                let excluded = if action.held {
                    release(world, mob, action.target, true);
                    action.target
                } else {
                    -1
                };
                slash(world, mob, excluded, 7.0, false);
                action.held = false;
            }
            (Attack::MagmaVolley, 3 | 13 | 23) => {
                self.shots.extend(volley(world, mob, action.target))
            }
            _ => {}
        }
        if age >= action.kind.timers().0 {
            self.running = None;
        }
    }

    pub fn take_effects(&mut self) -> (Vec<Shot>, bool) {
        (std::mem::take(&mut self.shots), self.fire_active())
    }

    pub fn keep_shots(&mut self, shots: Vec<Shot>) {
        self.shots = shots;
    }
}

fn melee(world: World<'_>, mob: Entity<'_>, id: i32, damage: f32, reach: f64, repeat: bool) {
    let (Some(ai), Some(mobs), Some(entities), Some(view)) =
        (MOB_AI.get(), MOBS.get(), ENTITIES.get(), mob.view())
    else {
        return;
    };
    entities.with_entity(world, id, |target| {
        let (Some(at), Some(body)) = (target.view(), target.body()) else {
            return;
        };
        let gap = length_squared(offset(at.position, view.position)).sqrt()
            - (2.5 + body.max[0] - body.min[0]) * 0.5;
        if !enemy(mob, target) || gap > reach {
            return;
        }
        if repeat {
            mobs.reset_hurt_cooldown(target);
        }
        if ai.attack(world, mob, id, damage) {
            let angle = f64::from(view.yaw).to_radians();
            target.change(&EntityChange {
                mask: SET_MOTION,
                motion: add(at.motion, [-angle.sin() * 0.3, 0.05, angle.cos() * 0.3]),
                ..EntityChange::default()
            });
        }
    });
}

fn slash(world: World<'_>, mob: Entity<'_>, primary: i32, damage: f32, grab: bool) -> bool {
    let (Some(ai), Some(entities), Some(mobs), Some(view)) =
        (MOB_AI.get(), ENTITIES.get(), MOBS.get(), mob.view())
    else {
        return false;
    };
    let (sin, cos) = f64::from(view.yaw).to_radians().sin_cos();
    let center = add(
        view.position,
        [-sin * 5.25 - cos * 1.6, 1.15, cos * 5.25 - sin * 1.6],
    );
    let mut holding = false;
    for id in entities.entities_in(
        world,
        add(center, [-3.0; 3]),
        add(center, [3.0; 3]),
        mob.id(),
    ) {
        if !grab && id == primary {
            continue;
        }
        entities.with_entity(world, id, |target| {
            if !enemy(mob, target) || target.vehicle().is_some() {
                return;
            }
            if !grab {
                mobs.reset_hurt_cooldown(target);
            }
            let hit = ai.attack(world, mob, id, damage);
            if grab
                && id == primary
                && hit
                && target.flags() & ENTITY_ALIVE != 0
                && mobs.passengers(mob).is_empty()
                && mobs.passengers(target).is_empty()
                && target
                    .body()
                    .is_some_and(|body| (0..3).all(|i| body.max[i] - body.min[i] <= 4.30))
            {
                holding = target.start_riding(mob.id(), true);
            }
        });
    }
    holding
}

fn release(world: World<'_>, mob: Entity<'_>, id: i32, damage: bool) {
    let (Some(ai), Some(entities), Some(mobs)) = (MOB_AI.get(), ENTITIES.get(), MOBS.get()) else {
        return;
    };
    entities.with_entity(world, id, |target| {
        if target.vehicle() == Some(mob.id())
            && target.stop_riding()
            && damage
            && target.vehicle() != Some(mob.id())
        {
            mobs.reset_hurt_cooldown(target);
            ai.attack(world, mob, id, 7.0);
            target.change(&EntityChange {
                mask: SET_MOTION,
                motion: [0.0, -0.65, 0.0],
                ..EntityChange::default()
            });
        }
    });
}

fn volley(world: World<'_>, mob: Entity<'_>, id: i32) -> Vec<Shot> {
    let (Some(entities), Some(view)) = (ENTITIES.get(), mob.view()) else {
        return Vec::new();
    };
    let Some(at) = entities.with_entity(world, id, |e| e.view()).flatten() else {
        return Vec::new();
    };
    let [dx, dy, dz] = offset(at.position, view.position);
    let angle = dz.atan2(dx);
    // shortcut: a ballistic lead from the current position. The Java AI takes off first.
    let duration = (dx.hypot(dz) / 0.50).clamp(4.0, 70.0);
    let vertical = ((dy - 1.5) / duration + 0.022 * duration).clamp(-0.4, 1.4);
    [-0.18, 0.0, 0.18]
        .into_iter()
        .map(|spread| {
            let (sin, cos) = (angle + spread).sin_cos();
            Shot {
                position: add(view.position, [cos * 2.0, 1.5, sin * 2.0]),
                motion: [cos * 0.55, vertical, sin * 0.55],
                age: 0,
            }
        })
        .collect()
}

fn area(
    world: World<'_>,
    mob: Entity<'_>,
    center: [f64; 3],
    radius: f64,
    vertical: f64,
    damage: f32,
    fire: u32,
) {
    let (Some(ai), Some(entities), Some(mobs)) = (MOB_AI.get(), ENTITIES.get(), MOBS.get()) else {
        return;
    };
    for id in entities.entities_in(
        world,
        add(center, [-radius, -vertical, -radius]),
        add(center, [radius, vertical, radius]),
        mob.id(),
    ) {
        entities.with_entity(world, id, |entity| {
            if enemy(mob, entity)
                && entity.view().is_some_and(|view| {
                    let [x, y, z] = offset(view.position, center);
                    x.hypot(z) <= radius && y.abs() <= vertical
                })
            {
                ai.attack(world, mob, id, damage);
                if fire > 0 {
                    mobs.set_on_fire(entity, fire);
                }
            }
        });
    }
}

pub fn effects(world: World<'_>, mob: Entity<'_>, shots: &mut Vec<Shot>, fire: bool) {
    let Some(visuals) = EFFECTS.get() else { return };
    if fire
        && tick_count().is_multiple_of(5)
        && let Some(view) = mob.view()
        && !world
            .fluid_id(crate::flight::cell(view.position))
            .is_some_and(|fluid| fluid.ends_with("water"))
    {
        visuals.particles(
            world,
            "minecraft:flame",
            add(view.position, [0.0, 1.0, 0.0]),
            [1.3, 0.8, 1.3],
            0.06,
            12,
        );
        area(world, mob, view.position, 3.5, 2.5, 3.0, 80);
    }
    shots.retain_mut(|shot| {
        shot.age += 1;
        shot.motion[1] -= 0.04;
        let next = add(shot.position, shot.motion);
        let lower = std::array::from_fn(|i| shot.position[i].min(next[i]) - 0.3);
        let upper = std::array::from_fn(|i| shot.position[i].max(next[i]) + 0.3);
        let targets: Vec<EntityBody> = ENTITIES.get().map_or_else(Vec::new, |entities| {
            entities
                .entities_in(world, lower, upper, mob.id())
                .into_iter()
                .filter_map(|id| {
                    entities
                        .with_entity(world, id, |entity| {
                            if enemy(mob, entity) {
                                entity.body()
                            } else {
                                None
                            }
                        })
                        .flatten()
                })
                .collect()
        });
        let steps = ((length_squared(shot.motion).sqrt() * 4.0).ceil() as u32).max(1);
        for i in 1..=steps {
            let point = add(
                shot.position,
                shot.motion.map(|v| v * f64::from(i) / f64::from(steps)),
            );
            let Some(shape) = world.collision_shape(crate::flight::cell(point)) else {
                return false;
            };
            if !shape.is_empty()
                || shot.age >= 200
                || targets.iter().any(|body| {
                    (0..3).all(|axis| {
                        (body.min[axis] - 0.3..=body.max[axis] + 0.3).contains(&point[axis])
                    })
                })
            {
                visuals.particles(world, "minecraft:lava", point, [1.0, 0.45, 1.0], 0.0, 12);
                area(world, mob, point, 7.0, 3.5, 20.0, 0);
                return false;
            }
        }
        visuals.particles(world, "minecraft:flame", next, [0.2; 3], 0.03, 4);
        shot.position = next;
        shot.motion = shot.motion.map(|v| v * 0.98);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_selects_all_primary_attacks_with_cooldown() {
        for (gap, expected) in [
            (1.0, Attack::DoubleBite),
            (5.0, Attack::SlashGrab),
            (12.0, Attack::MagmaVolley),
        ] {
            let mut state = Abilities::default();
            assert!(state.choose(0, 1, gap, true));
            assert_eq!(state.last, Some(expected));
            assert!(!state.choose(0, 1, gap, true));
        }
        let mut state = Abilities::default();
        assert!(state.choose(0, 1, 1.0, true));
        assert!(state.choose(28, 1, 1.0, true));
        assert_eq!(state.last, Some(Attack::Bite));
    }
}
