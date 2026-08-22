//! Entities (units & buildings) and the v2 turn-based balance tables.
//!
//! All combat/economy stats are plain integers so they behave identically on
//! every target. Entity ids are assigned in ascending creation order; every
//! resolution step iterates entities in ascending id order (part of the
//! determinism contract).
//!
//! The game is **turn-based**: there is no tick, no cooldown, no continuous
//! position. Units act once per own turn (`mp` movement points + one attack),
//! and combat resolves instantly with Advance-Wars-style damage scaling.

use serde::{Deserialize, Serialize};

/// Two players. Serialized as 0 / 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
#[repr(u8)]
pub enum Player {
    P0 = 0,
    P1 = 1,
}

impl Player {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn enemy(self) -> Player {
        match self {
            Player::P0 => Player::P1,
            Player::P1 => Player::P0,
        }
    }

    pub const ALL: [Player; 2] = [Player::P0, Player::P1];
}

/// Resource types that may appear as inexhaustible map deposits or in a
/// stockpile. A deposit's richness, not a remaining reserve, controls yield.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub enum ResourceType {
    Ore,
    Steel,
    Coal,
    Crystal,
}

impl ResourceType {
    pub const ALL: [ResourceType; 4] = [
        ResourceType::Ore,
        ResourceType::Steel,
        ResourceType::Coal,
        ResourceType::Crystal,
    ];

    pub const fn index(self) -> usize {
        match self {
            ResourceType::Ore => 0,
            ResourceType::Steel => 1,
            ResourceType::Coal => 2,
            ResourceType::Crystal => 3,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            ResourceType::Ore => "Ore",
            ResourceType::Steel => "Steel",
            ResourceType::Coal => "Coal",
            ResourceType::Crystal => "Crystal",
        }
    }
}

/// A player's stockpile or a blueprint's resource price.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub struct ResourceBundle {
    pub ore: i32,
    pub steel: i32,
    pub coal: i32,
    pub crystal: i32,
}

impl ResourceBundle {
    pub const fn new(ore: i32, steel: i32, coal: i32, crystal: i32) -> Self {
        ResourceBundle {
            ore,
            steel,
            coal,
            crystal,
        }
    }

    pub const fn zero() -> Self {
        ResourceBundle::new(0, 0, 0, 0)
    }

    pub fn can_afford(self, price: ResourceBundle) -> bool {
        self.ore >= price.ore
            && self.steel >= price.steel
            && self.coal >= price.coal
            && self.crystal >= price.crystal
    }

    pub fn checked_sub(self, price: ResourceBundle) -> Option<ResourceBundle> {
        if self.can_afford(price) {
            Some(ResourceBundle::new(
                self.ore - price.ore,
                self.steel - price.steel,
                self.coal - price.coal,
                self.crystal - price.crystal,
            ))
        } else {
            None
        }
    }

    pub fn saturating_add(self, amount: ResourceBundle) -> ResourceBundle {
        ResourceBundle::new(
            self.ore.saturating_add(amount.ore),
            self.steel.saturating_add(amount.steel),
            self.coal.saturating_add(amount.coal),
            self.crystal.saturating_add(amount.crystal),
        )
    }

    pub fn scaled_floor(self, numerator: i32, denominator: i32) -> ResourceBundle {
        if denominator <= 0 {
            return ResourceBundle::zero();
        }
        ResourceBundle::new(
            self.ore * numerator / denominator,
            self.steel * numerator / denominator,
            self.coal * numerator / denominator,
            self.crystal * numerator / denominator,
        )
    }

    /// A stable value used for timeout/fitness scoring.
    pub fn total_value(self) -> i32 {
        self.ore + self.steel + self.coal + self.crystal
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub enum BuildingType {
    Hq,
    PowerPlant,
    /// The generic extractor. It is placed directly on any live deposit and
    /// produces that tile's resource type.
    Refinery,
    /// Legacy wire/replay name retained for old matches. It follows the same
    /// generic on-tile extraction rules as `Refinery`.
    CrystalRefinery,
    Barracks,
    Factory,
    TechLab,
    Airfield,
    Radar,
    TeslaCoil,
    Turret,
    AATurret,
}

impl BuildingType {
    /// Whether this structure extracts the resource on its own tile.
    pub const fn is_refinery(self) -> bool {
        matches!(self, BuildingType::Refinery | BuildingType::CrystalRefinery)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub enum UnitType {
    Infantry,
    Scout,
    RocketTrooper,
    Tank,
    Artillery,
    MammothTank,
    Gunship,
    Interceptor,
    SamLauncher,
}

/// Unique entity id within a match.
pub type EntityId = u32;

/// Static, per-type balance data. Distances/radii are integer tiles,
/// durations in turns, ore in integer units.
#[derive(Clone, Copy, Debug)]
pub struct UnitStats {
    /// Legacy ore-equivalent value used by balance/timeout scoring.
    pub cost: i32,
    /// Actual spendable price across all stockpiles.
    pub resource_cost: ResourceBundle,
    pub hp: i32,
    pub damage: i32,
    /// Attack range in tiles (Chebyshev-free Euclidean on tile centers).
    pub range_tiles: i32,
    /// Minimum attack range in tiles (artillery cannot fire inside this).
    pub min_range_tiles: i32,
    /// Movement points per turn (one orthogonal or diagonal step costs 1).
    pub mp: i32,
    /// Vision radius in tiles.
    pub vision_tiles: i32,
    /// Production time in turns.
    pub build_time_turns: i32,
    /// Whether the unit flies: it ignores terrain passability and building
    /// blockers when moving.
    pub air: bool,
    /// Anti-air capable: deals full damage to air targets. Ground units
    /// without this deal only half damage to air units.
    pub aa: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct BuildingStats {
    /// Legacy ore-equivalent value used by balance/timeout scoring.
    pub cost: i32,
    /// Actual spendable price across all stockpiles.
    pub resource_cost: ResourceBundle,
    pub hp: i32,
    pub vision_tiles: i32,
    /// Attack damage (turrets only; fired once per own turn at end of turn).
    pub damage: i32,
    /// Attack range in tiles (turrets only).
    pub range_tiles: i32,
    /// Power produced (positive) or consumed (negative).
    pub power: i32,
    /// Construction duration in turns.
    pub build_time_turns: i32,
}

pub const fn unit_stats(ut: UnitType) -> UnitStats {
    use UnitType::*;
    match ut {
        Infantry => UnitStats {
            cost: 50,
            resource_cost: ResourceBundle::new(50, 10, 0, 0),
            hp: 90,
            damage: 55,
            range_tiles: 1,
            min_range_tiles: 0,
            mp: 3,
            vision_tiles: 4,
            build_time_turns: 1,
            air: false,
            aa: false,
        },
        // Fast, fragile recon: a long stride and wide eyes, poor in a fight.
        Scout => UnitStats {
            cost: 40,
            resource_cost: ResourceBundle::new(40, 8, 0, 0),
            hp: 60,
            damage: 30,
            range_tiles: 1,
            min_range_tiles: 0,
            mp: 6,
            vision_tiles: 6,
            build_time_turns: 1,
            air: false,
            aa: false,
        },
        // Anti-armor/anti-air infantry: a shoulder-mounted rocket launcher.
        // Rockets trade the rifleman's cheapness for a hard punch vs vehicles.
        RocketTrooper => UnitStats {
            cost: 120,
            resource_cost: ResourceBundle::new(120, 35, 0, 0),
            hp: 90,
            damage: 85,
            range_tiles: 2,
            min_range_tiles: 0,
            mp: 3,
            vision_tiles: 4,
            build_time_turns: 2,
            air: false,
            aa: true,
        },
        Tank => UnitStats {
            cost: 150,
            resource_cost: ResourceBundle::new(150, 60, 20, 0),
            hp: 260,
            damage: 105,
            range_tiles: 1,
            min_range_tiles: 0,
            mp: 5,
            vision_tiles: 5,
            build_time_turns: 2,
            air: false,
            aa: false,
        },
        // Long-range siege: cannot fire point blank (min range 2), fragile,
        // slow — but outranges every turret and hits from behind the line.
        Artillery => UnitStats {
            cost: 200,
            resource_cost: ResourceBundle::new(200, 80, 30, 0),
            hp: 120,
            damage: 110,
            range_tiles: 3,
            min_range_tiles: 2,
            mp: 3,
            vision_tiles: 6,
            build_time_turns: 2,
            air: false,
            aa: false,
        },
        MammothTank => UnitStats {
            cost: 350,
            resource_cost: ResourceBundle::new(350, 180, 60, 0),
            hp: 550,
            damage: 170,
            range_tiles: 1,
            min_range_tiles: 0,
            mp: 4,
            vision_tiles: 5,
            build_time_turns: 3,
            air: false,
            aa: true,
        },
        // Fast strike aircraft (airfield-built). Fragile but mobile: flies over
        // everything, strikes from range 2 where melee units cannot retaliate.
        Gunship => UnitStats {
            cost: 250,
            resource_cost: ResourceBundle::new(250, 100, 80, 0),
            hp: 140,
            damage: 105,
            range_tiles: 2,
            min_range_tiles: 0,
            mp: 7,
            vision_tiles: 5,
            build_time_turns: 2,
            air: true,
            aa: false,
        },
        Interceptor => UnitStats {
            cost: 200,
            resource_cost: ResourceBundle::new(200, 80, 100, 0),
            hp: 110,
            damage: 70,
            range_tiles: 2,
            min_range_tiles: 0,
            mp: 8,
            vision_tiles: 6,
            build_time_turns: 2,
            air: true,
            aa: false,
        },
        // Ground-based surface-to-air missile launcher: brutal vs aircraft,
        // fragile and short-ranged against ground targets (low damage, small
        // hp, slow). The dedicated answer to a gunship/interceptor fleet.
        // Ground-based surface-to-air missile launcher: brutal vs aircraft,
        // weak against ground targets (low damage, fragile, slow). The
        // dedicated answer to a gunship/interceptor fleet. Its ground damage
        // is deliberately low so it cannot double as a general-purpose unit.
        SamLauncher => UnitStats {
            cost: 180,
            resource_cost: ResourceBundle::new(180, 100, 50, 0),
            hp: 110,
            // Low base damage: the SAM is an AA specialist, not a brawler.
            // The anti-air rule in combat gives full damage to air targets;
            // ground targets take half of this already-low value.
            damage: 35,
            range_tiles: 4,
            min_range_tiles: 1,
            mp: 2,
            vision_tiles: 5,
            build_time_turns: 2,
            air: false,
            aa: true,
        },
    }
}

pub const fn building_stats(bt: BuildingType) -> BuildingStats {
    use BuildingType::*;
    match bt {
        Hq => BuildingStats {
            cost: 0,
            resource_cost: ResourceBundle::zero(),
            hp: 1500,
            // A slightly generous opening sightline reveals the first biome
            // ring while fog still protects the wider theatre.
            vision_tiles: 7,
            damage: 0,
            range_tiles: 0,
            power: 50,
            build_time_turns: 0,
        },
        PowerPlant => BuildingStats {
            cost: 150,
            resource_cost: ResourceBundle::new(150, 20, 50, 0),
            hp: 300,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: 100,
            build_time_turns: 1,
        },
        Refinery => BuildingStats {
            cost: 300,
            resource_cost: ResourceBundle::new(300, 50, 0, 0),
            hp: 400,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: -20,
            build_time_turns: 2,
        },
        // Compatibility alias for old replays. New commands should use the
        // generic Refinery on whichever resource tile they want to claim.
        CrystalRefinery => BuildingStats {
            cost: 350,
            resource_cost: ResourceBundle::new(350, 50, 0, 0),
            hp: 400,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: -25,
            build_time_turns: 2,
        },
        Barracks => BuildingStats {
            cost: 150,
            resource_cost: ResourceBundle::new(150, 40, 0, 0),
            hp: 300,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: -15,
            build_time_turns: 2,
        },
        Factory => BuildingStats {
            cost: 250,
            resource_cost: ResourceBundle::new(250, 100, 30, 0),
            hp: 400,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: -25,
            build_time_turns: 2,
        },
        TechLab => BuildingStats {
            cost: 200,
            resource_cost: ResourceBundle::new(200, 80, 50, 0),
            hp: 250,
            vision_tiles: 3,
            damage: 0,
            range_tiles: 0,
            power: -30,
            build_time_turns: 3,
        },
        Airfield => BuildingStats {
            cost: 250,
            resource_cost: ResourceBundle::new(250, 80, 100, 0),
            hp: 350,
            vision_tiles: 4,
            damage: 0,
            range_tiles: 0,
            power: -25,
            build_time_turns: 2,
        },
        // Long-range early-warning dish: reveals a huge swath of the map so
        // scouting happens passively. The tech payoff for map awareness.
        Radar => BuildingStats {
            cost: 150,
            resource_cost: ResourceBundle::new(150, 40, 30, 0),
            hp: 300,
            vision_tiles: 10,
            damage: 0,
            range_tiles: 0,
            power: -10,
            build_time_turns: 2,
        },
        // High-voltage coil defense: a turret that outranges and out-hits the
        // standard turret (range 4, 24 dmg), gated on the TechLab.
        TeslaCoil => BuildingStats {
            cost: 250,
            resource_cost: ResourceBundle::new(250, 100, 100, 0),
            hp: 260,
            vision_tiles: 4,
            damage: 24,
            range_tiles: 4,
            power: -30,
            build_time_turns: 2,
        },
        Turret => BuildingStats {
            cost: 100,
            resource_cost: ResourceBundle::new(100, 30, 10, 0),
            hp: 150,
            vision_tiles: 4,
            damage: 12,
            range_tiles: 3,
            power: -20,
            build_time_turns: 1,
        },
        // Anti-air defense: only engages air units, but hits them hard.
        AATurret => BuildingStats {
            cost: 200,
            resource_cost: ResourceBundle::new(200, 80, 50, 0),
            hp: 200,
            vision_tiles: 5,
            damage: 45,
            range_tiles: 4,
            power: -25,
            build_time_turns: 2,
        },
    }
}

/// Passive income: each player's HQ generates this much Ore per turn.
pub const HQ_INCOME_PER_TURN: i32 = 10;
/// Base extraction for a standard-richness, inexhaustible resource tile.
/// Richness tiers multiply this by 1/2/3; Crystal uses a slower base rate.
pub const REFINERY_BASE_YIELD_PER_TURN: i32 = 30;
pub const CRYSTAL_REFINERY_BASE_YIELD_PER_TURN: i32 = 15;
/// Legacy rate aliases retained for callers compiled against the old economy;
/// they are rates, never finite deposit capacities.
pub const REFINERY_ORE_PER_TURN: i32 = 60;
pub const CRYSTAL_REFINERY_PER_TURN: i32 = 25;
/// Each Tech Lab generates this many research points per own turn. Paced so
/// a tier-1 tech (150 pts) lands in ~6 turns — close to the old instant
/// upgrade — while the full 10-tech tree stays a late-game project.
pub const RESEARCH_PER_LAB_PER_TURN: i32 = 25;

/// Which units a building can train.
pub fn building_produces(bt: BuildingType) -> &'static [UnitType] {
    use BuildingType::*;
    use UnitType::*;
    match bt {
        Barracks => &[Infantry, Scout, RocketTrooper],
        Factory => &[Tank, Artillery, MammothTank, SamLauncher],
        Airfield => &[Gunship, Interceptor],
        _ => &[],
    }
}

/// The tech that gates a unit's production, if any.
pub fn unit_requires_tech(ut: UnitType) -> Option<crate::tech::TechId> {
    use crate::tech::TechId;
    match ut {
        UnitType::RocketTrooper | UnitType::SamLauncher => Some(TechId::RocketPropulsion),
        _ => None,
    }
}

/// Where a building may be placed: within this many tiles (center-to-center)
/// of at least one own building, so bases grow in connected clumps instead of
/// floating structures. Resource refineries are exempt: they must instead be
/// placed directly on a live deposit, which makes remote resource pockets the
/// expansion mechanic.
pub const PLACE_RADIUS_TILES: i32 = 5;

/// Fog memory: remembered enemy sightings drop after this many turns unseen.
pub const FOG_MEMORY_TURNS: i32 = 6;

/// Repair command: heals this fraction of a building's max HP...
pub const REPAIR_HP_NUM: i32 = 30;
pub const REPAIR_HP_DEN: i32 = 100;
/// ...at this fraction of its build cost (minimum 10 ore).
pub const REPAIR_COST_NUM: i32 = 20;
pub const REPAIR_COST_DEN: i32 = 100;
pub const REPAIR_MIN_COST: i32 = 10;

fn legacy_complete_progress() -> i32 {
    // Snapshots from before construction sites existed contain no progress
    // field. Treat those historical buildings as already completed.
    i32::MAX
}

/// A building in the world.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Building {
    pub id: EntityId,
    pub owner: Player,
    pub btype: BuildingType,
    pub tile: (u8, u8),
    pub hp: i32,
    pub max_hp: i32,
    /// Pending production queue (unit types).
    pub queue: Vec<UnitType>,
    /// Progress toward the current queue head, in turns.
    pub progress: i32,
    /// Progress of this building's own construction, in turns. A placed site
    /// starts at zero and advances only on its owner's start-of-turn.
    #[serde(default = "legacy_complete_progress")]
    pub construction_progress: i32,
    /// Attack cooldown (turrets fire once per own turn; unused otherwise).
    pub cooldown: i32,
    /// Whether this building was repaired already this turn.
    #[serde(default)]
    pub repaired_this_turn: bool,
}

impl Building {
    pub fn is_alive(&self) -> bool {
        self.hp > 0
    }

    pub fn construction_time(&self) -> i32 {
        building_stats(self.btype).build_time_turns
    }

    pub fn is_operational(&self) -> bool {
        self.is_alive() && self.construction_progress >= self.construction_time()
    }
}

/// A unit in the world. One move + one attack per own turn.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Unit {
    pub id: EntityId,
    pub owner: Player,
    pub utype: UnitType,
    pub tile: (u8, u8),
    pub hp: i32,
    pub max_hp: i32,
    /// Movement points remaining this turn.
    pub mp: i32,
    /// Durable destination. The sim automatically advances toward it at the
    /// start of this unit's next turn until it is reached or the order is
    /// replaced/cleared.
    #[serde(default)]
    pub move_target: Option<(u8, u8)>,
    /// Has moved this turn (a unit may move then attack, not attack then move).
    pub moved: bool,
    /// Has attacked this turn (ends the unit's activation).
    pub acted: bool,
}

impl Unit {
    pub fn is_alive(&self) -> bool {
        self.hp > 0
    }
}
