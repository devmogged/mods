//! Saint's Dragons, ported from Saint's Dragons 0.9.88 (commit 13aeb2eb2c). See docs/P2-DRAGONS.md.
//!
//! Cindervane uses a vanilla carrier with its own saved state, AI, combat and rider controls.

mod abilities;
mod ai;
mod combat;
mod flight;
mod roster;
mod taming;
mod visuals;

use std::sync::OnceLock;

use moddedmc_api::{
    Command, InterfaceUnavailable, Mod, RegistrationError, Registry,
    commands::{self, Argument, Commands, Node},
    content::{self, Content, WorldExtension},
    effects::{self, Effects},
    entities::{self, Entities, EntitiesExtension},
    interaction::{self, Interaction, InteractionExtension},
    items::{self, Item, Items},
    mob_ai::{self, Goal, MobAi, MobAiExtension, goal_control},
    mobs::{self, Mob, Mobs, MobsExtension},
    stacks::{self, ItemStacks},
    tags::{self, Tags},
};

const CINDERVANE: &str = "saintsdragons:cindervane";
/// Stock clients see this until the model is drawn with displays (P2-DRAGONS.md).
const CINDERVANE_CARRIER: &str = "minecraft:ravager";
/// `CindervaneStatProfile` and its config defaults for an adult.
const CINDERVANE_ATTRIBUTES: &[(&str, f64)] = &[
    ("minecraft:max_health", 80.0),
    ("minecraft:armor", 4.0),
    ("minecraft:flying_speed", 0.60),
    ("minecraft:movement_speed", 0.30),
    ("minecraft:follow_range", 64.0),
];

const SPAWN: u64 = 1;
const FIND: u64 = 2;
/// How far `/cindervane find` looks from its position, in blocks.
const FIND_RADIUS: f64 = 16.0;

static CONTENT: OnceLock<Content> = OnceLock::new();
static EFFECTS: OnceLock<Effects> = OnceLock::new();
static COMMANDS: OnceLock<Commands> = OnceLock::new();
static ENTITIES: OnceLock<Entities> = OnceLock::new();
static MOBS: OnceLock<Mobs> = OnceLock::new();
static MOB_AI: OnceLock<MobAi> = OnceLock::new();
static INTERACTION: OnceLock<Interaction> = OnceLock::new();
static ITEMS: OnceLock<Items> = OnceLock::new();
static STACKS: OnceLock<ItemStacks> = OnceLock::new();
static TAGS: OnceLock<Tags> = OnceLock::new();

struct SaintsDragons;

impl Mod for SaintsDragons {
    fn start(&self) -> Result<(), InterfaceUnavailable> {
        let content = content::require()?;
        content.require_extension(WorldExtension::Survival)?;
        content.require_extension(WorldExtension::CollisionShape)?;
        content.require_extension(WorldExtension::FluidId)?;
        let _ = CONTENT.set(content);
        let _ = EFFECTS.set(effects::require()?);
        let _ = COMMANDS.set(commands::require()?);
        let entities = entities::require()?;
        entities.require_extension(EntitiesExtension::Carriers)?;
        let _ = ENTITIES.set(entities);
        let mobs = mobs::require()?;
        mobs.require_extension(MobsExtension::Taming)?;
        let _ = MOBS.set(mobs);
        let ai = mob_ai::require()?;
        ai.require_extension(MobAiExtension::Combat)?;
        let _ = MOB_AI.set(ai);
        let interaction = interaction::require()?;
        interaction.require_extension(InteractionExtension::MobUse)?;
        let _ = INTERACTION.set(interaction);
        let _ = ITEMS.set(items::require()?);
        let _ = STACKS.set(stacks::require()?);
        let _ = TAGS.set(tags::require()?);
        visuals::start()?;
        Ok(())
    }

    fn register(&self, registry: &mut Registry<'_>) -> Result<(), RegistrationError> {
        let missing = RegistrationError::InvalidDefinition;
        CONTENT
            .get()
            .ok_or(missing)?
            .on_tick(registry, visuals::sweep)?;
        visuals::register(*ITEMS.get().ok_or(missing)?, registry)?;
        ITEMS.get().ok_or(missing)?.add_item(
            registry,
            &Item {
                id: taming::HEARTY_MEAL,
                display_name: "Hearty Dragon Meal",
                // shortcut: the stock client uses a vanilla model and food components.
                carrier: "minecraft:cooked_chicken",
                model: "minecraft:item/cooked_chicken",
            },
        )?;
        for (name, display_name, carrier) in [
            ("ignivorus_heart", "Ignivorus Heart", "minecraft:paper"),
            ("ignivorus_tooth", "Ignivorus Tooth", "minecraft:paper"),
            (
                "ignivorus_wing_hide",
                "Ignivorus Wing Hide",
                "minecraft:paper",
            ),
            ("raevyx_wing_hide", "Raevyx Wing Hide", "minecraft:paper"),
            ("raevyx_wingtalon", "Raevyx Wingtalon", "minecraft:paper"),
            ("raw_mossback", "Raw Mossback", "minecraft:rabbit"),
        ] {
            ITEMS.get().ok_or(missing)?.add_item(
                registry,
                &Item {
                    id: &format!("saintsdragons:{name}"),
                    display_name,
                    carrier,
                    model: &format!("saintsdragons:item/{name}"),
                },
            )?;
        }
        MOBS.get().ok_or(missing)?.add_mob(
            registry,
            &Mob {
                id: CINDERVANE,
                carrier: CINDERVANE_CARRIER,
                attributes: CINDERVANE_ATTRIBUTES,
            },
        )?;
        let ai = MOB_AI.get().ok_or(missing)?;
        ai.on_goal(ai::HANDLER)?;
        ai.replace_goals(registry, CINDERVANE)?;
        ai.on_hurt(registry, CINDERVANE, combat::hurt)?;
        MOBS.get()
            .ok_or(missing)?
            .on_death(registry, CINDERVANE, taming::death)?;
        INTERACTION
            .get()
            .ok_or(missing)?
            .on_use_mob(registry, CINDERVANE, taming::interact)?;
        for (key, priority, controls) in [
            (
                taming::RIDE_OR_SIT,
                0,
                goal_control::MOVE | goal_control::LOOK | goal_control::JUMP,
            ),
            (
                combat::GROUND_COMBAT,
                1,
                goal_control::MOVE | goal_control::LOOK,
            ),
            (taming::FOLLOW, 2, goal_control::MOVE),
            (flight::GOAL, 2, goal_control::MOVE | goal_control::LOOK),
            (ai::IDLE_LOOK, 3, goal_control::LOOK),
            (ai::GROUND_WANDER, 4, goal_control::MOVE),
            (combat::TARGETING, 1, goal_control::TARGET),
            (ai::AMBIENT, 0, 0),
        ] {
            ai.add_goal(
                registry,
                &Goal {
                    kind: CINDERVANE,
                    key,
                    priority,
                    controls,
                },
            )?;
        }
        let at = |parent, tag| {
            [
                number(Some(parent), "x", None),
                number(Some(parent + 1), "y", None),
                number(Some(parent + 2), "z", Some(tag)),
            ]
        };
        let mut nodes = vec![literal("spawn")];
        nodes.extend(at(0, SPAWN));
        nodes.push(literal("find"));
        nodes.extend(at(4, FIND));
        COMMANDS.get().ok_or(missing)?.add(
            registry,
            "cindervane",
            "Spawn or find Cindervanes",
            2,
            &nodes,
        )?;
        roster::register(registry)
    }

    fn command(&self, tag: u64, command: Command<'_>) -> i32 {
        let (Some(content), Some(commands), Some(entities), Some(mobs), Some(ai)) = (
            CONTENT.get(),
            COMMANDS.get(),
            ENTITIES.get(),
            MOBS.get(),
            MOB_AI.get(),
        ) else {
            return 0;
        };
        let (Some(x), Some(y), Some(z), Some(world)) = (
            commands.integer(&command, "x"),
            commands.integer(&command, "y"),
            commands.integer(&command, "z"),
            content.command_world(&command),
        ) else {
            return 0;
        };
        let at = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
        if let Some(kind) = roster::spawn_kind(tag) {
            let Some(id) = roster::spawn(world, &kind, at) else {
                command.reply(&format!("{kind}: could not spawn"));
                return 0;
            };
            command.reply(&format!("{kind}: spawned {id}"));
            return 1;
        }
        match tag {
            SPAWN => {
                let Some(id) = mobs.spawn(world, CINDERVANE, at) else {
                    command.reply("cindervane: could not spawn");
                    return 0;
                };
                let tagged = entities
                    .with_entity(world, id, |mob| mobs.set_state(mob, b"wild"))
                    .unwrap_or(false);
                command.reply(&format!("cindervane: spawned {id}, state set {tagged}"));
                1
            }
            FIND => {
                let min = at.map(|c| c - FIND_RADIUS);
                let max = at.map(|c| c + FIND_RADIUS);
                let mut around = Vec::new();
                let found: Vec<String> = entities
                    .entities_in(world, min, max, -1)
                    .into_iter()
                    .filter_map(|id| {
                        entities.with_entity(world, id, |mob| {
                            let Some(kind) = mobs.kind(mob).filter(|kind| *kind == CINDERVANE) else {
                                if !visuals::is_display(id) {
                                    around.push(mob.type_id().to_owned());
                                }
                                return None;
                            };
                            let state = mobs.state(mob)?;
                            let target = ai.target(mob).and_then(|t| {
                                entities.with_entity(world, t, |e| e.type_id().to_owned())
                            });
                            let health = mobs.health(mob).map_or(0.0, |(current, _)| current);
                            Some(format!(
                                "{kind} {} state={} ai={} health={health:.1} flight={} attack={} fire={} target={}",
                                mob.type_id(),
                                taming::describe(&state),
                                if ai::seen(id) { "on" } else { "off" },
                                if flight::active(id) { "active" } else { "idle" },
                                combat::last(mob).unwrap_or("none"),
                                if combat::fire_active(mob) { "active" } else { "idle" },
                                target.as_deref().unwrap_or("none")
                            ))
                        })?
                    })
                    .collect();
                around.sort();
                around.dedup();
                command.reply(&format!(
                    "cindervane: found {}: {}; around: {}",
                    found.len(),
                    found.join(", "),
                    around.join(",")
                ));
                i32::try_from(found.len()).unwrap_or(i32::MAX)
            }
            _ => 0,
        }
    }
}

const fn literal(name: &str) -> Node<'_> {
    Node {
        parent: None,
        name,
        argument: Argument::Literal,
        tag: None,
    }
}

const fn number(parent: Option<u32>, name: &str, tag: Option<u64>) -> Node<'_> {
    Node {
        parent,
        name,
        argument: Argument::Integer {
            min: -30_000_000,
            max: 30_000_000,
        },
        tag,
    }
}

moddedmc_api::export_mod!(SaintsDragons);
