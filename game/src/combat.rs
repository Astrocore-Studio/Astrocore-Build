//! Ближний бой: режим боя включается действием с горячей панели; в режиме боя клик бьёт
//! оружием в руке, перчатками или кулаком вместо взаимодействия. Образец —
//! `Content.Shared/CombatMode` и `Content.Shared/Weapons/Melee` SS14 @ 2f4a2aad
//! (`CombatModeComponent`, `SharedCombatModeSystem`, `MeleeWeaponComponent`,
//! `SharedMeleeWeaponSystem`: лёгкий удар, `MeleeSoundSystem`). Размашистый удар,
//! обезоруживание, выпад и вспышка цели — фаза 7.3.

use std::collections::BTreeMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_input::{BoundKeyState, InputWorldExt};
use astrocore_map::{MapCoordinates, MapId};
use astrocore_math::{Angle, Vec2};
use astrocore_net::{NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetRegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::can_attack;
use crate::actions::{ActionEvent, ActionsWorldExt};
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt};
use crate::damage_other_on_hit::highest_damage_sound;
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::hands::HandsWorldExt;
use crate::interaction::{InteractionWorldExt, functions as interaction_functions};
use crate::inventory::InventoryWorldExt;
use crate::item::Item;
use crate::popups::{PopupType, PopupWorldExt};

/// Прототип действия режима боя по умолчанию.
pub const COMBAT_TOGGLE_ACTION: &str = "ActionCombatModeToggle";
/// Разброс высоты звука удара (`DamagePitchVariation`).
pub const DAMAGE_PITCH_VARIATION: f32 = 0.05;

fn toggle_action() -> String {
    COMBAT_TOGGLE_ACTION.into()
}

fn disarm_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/thudswoosh.ogg")
}

fn disarm_fail() -> f32 {
    0.75
}

fn yes() -> bool {
    true
}

/// Режим боя. В данных — компонент `CombatMode`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatMode {
    #[serde(default)]
    pub can_disarm: Option<bool>,
    #[serde(default = "disarm_sound")]
    pub disarm_success_sound: SoundSpecifier,
    #[serde(default = "disarm_fail", rename = "disarmFailChance")]
    pub base_disarm_fail_chance: f32,
    #[serde(default = "toggle_action")]
    pub combat_toggle_action: String,
    #[serde(skip)]
    pub combat_toggle_action_entity: Option<Entity>,
    #[serde(default)]
    pub is_in_combat_mode: bool,
    #[serde(default = "yes")]
    pub toggle_mouse_rotator: bool,
}

impl Default for CombatMode {
    fn default() -> Self {
        Self {
            can_disarm: None,
            disarm_success_sound: disarm_sound(),
            base_disarm_fail_chance: disarm_fail(),
            combat_toggle_action: toggle_action(),
            combat_toggle_action_entity: None,
            is_in_combat_mode: false,
            toggle_mouse_rotator: true,
        }
    }
}

fn swing_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Weapons/punchmiss.ogg")
}

fn weak_hit() -> SoundSpecifier {
    SoundSpecifier::collection("WeakHit")
}

fn one() -> f32 {
    1.0
}

fn range() -> f32 {
    1.5
}

fn sixty_degrees() -> Angle {
    Angle::degrees(60.0)
}

fn thrust() -> String {
    "WeaponArcThrust".into()
}

fn slash() -> String {
    "WeaponArcSlash".into()
}

fn half() -> FixedPoint2 {
    FixedPoint2::from_f32(0.5)
}

fn one_fixed() -> FixedPoint2 {
    FixedPoint2::new(1)
}

fn five() -> i32 {
    5
}

/// Оружие ближнего боя (и кулаки существа). В данных — компонент `MeleeWeapon`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeleeWeapon {
    #[serde(default = "yes")]
    pub alt_disarm: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Время следующего удара, секунды игры.
    #[serde(skip)]
    pub next_attack: f64,
    #[serde(default = "yes")]
    pub reset_on_hand_selected: bool,
    /// Ударов в секунду.
    #[serde(default = "one")]
    pub attack_rate: f32,
    #[serde(default)]
    pub attacking: bool,
    #[serde(default)]
    pub auto_attack: bool,
    #[serde(default)]
    pub resistance_bypass: bool,
    #[serde(default)]
    pub damage: DamageSpecifier,
    #[serde(default = "half")]
    pub blunt_stamina_damage_factor: FixedPoint2,
    #[serde(default = "one_fixed")]
    pub click_damage_modifier: FixedPoint2,
    #[serde(default = "range")]
    pub range: f32,
    #[serde(default = "sixty_degrees")]
    pub angle: Angle,
    #[serde(default = "thrust")]
    pub animation: String,
    #[serde(default = "slash")]
    pub wide_animation: String,
    #[serde(default)]
    pub wide_animation_rotation: Angle,
    #[serde(default)]
    pub swing_left: bool,
    #[serde(default = "yes")]
    pub swing_beverage: bool,
    #[serde(default = "one")]
    pub animation_offset: f32,
    #[serde(default = "swing_sound", rename = "soundSwing")]
    pub swing_sound: SoundSpecifier,
    #[serde(default, rename = "soundHit")]
    pub hit_sound: Option<SoundSpecifier>,
    #[serde(default = "weak_hit", rename = "soundNoDamage")]
    pub no_damage_sound: SoundSpecifier,
    #[serde(default)]
    pub must_be_equipped_to_use: bool,
    #[serde(default = "five")]
    pub undamaged_alert_threshold: i32,
}

/// Звук удара по сущности по типу или группе урона. В данных — компонент `MeleeSound`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeleeSound {
    #[serde(default)]
    pub sound_groups: Option<BTreeMap<String, SoundSpecifier>>,
    #[serde(default)]
    pub sound_types: Option<BTreeMap<String, SoundSpecifier>>,
    #[serde(default)]
    pub no_damage_sound: Option<SoundSpecifier>,
}

/// Чем бьёт сущность, если не обычным путём (`GetMeleeWeaponEvent`). На атакующем.
#[derive(Debug, Clone, Copy, Default)]
pub struct GetMeleeWeapon {
    pub weapon: Option<Entity>,
    pub handled: bool,
}

/// Урон оружия можно изменить (`GetMeleeDamageEvent`). На оружии.
#[derive(Debug, Clone)]
pub struct GetMeleeDamage {
    pub weapon: Entity,
    pub user: Entity,
    pub damage: DamageSpecifier,
    pub resistance_bypass: bool,
}

/// Можно ли махнуть (`AttemptMeleeEvent`). На оружии.
#[derive(Debug, Clone, Default)]
pub struct AttemptMelee {
    pub cancelled: bool,
    pub message: Option<String>,
}

/// Удар нанесён или промах (`MeleeHitEvent`). На оружии.
#[derive(Debug, Clone)]
pub struct MeleeHit {
    pub hit_entities: Vec<Entity>,
    pub user: Entity,
    pub weapon: Entity,
    pub base_damage: DamageSpecifier,
    pub bonus_damage: DamageSpecifier,
    pub hit_sound_override: Option<SoundSpecifier>,
    pub handled: bool,
}

/// По цели ударили (`AttackedEvent`). На цели.
#[derive(Debug, Clone)]
pub struct Attacked {
    pub used: Entity,
    pub user: Entity,
    pub bonus_damage: DamageSpecifier,
}

/// Атакующий ударил (`MeleeAttackEvent`). На атакующем.
#[derive(Debug, Clone, Copy)]
pub struct MeleeAttack {
    pub weapon: Entity,
}

/// Лёгкий удар от клиента (`LightAttackEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightAttack {
    pub target: Option<NetEntity>,
    pub weapon: NetEntity,
    pub coordinates: (MapId, Vec2, i16),
}

/// Бой на `World`.
pub trait CombatWorldExt {
    fn is_in_combat_mode(&self, entity: Entity) -> bool;
    /// Включить или выключить режим боя (`SetInCombatMode`).
    fn set_in_combat_mode(&mut self, entity: Entity, value: bool);
    /// Чем бьёт сущность (`TryGetWeapon`).
    fn melee_weapon_of(&mut self, entity: Entity) -> Option<Entity>;
    /// Урон оружия с поправками (`GetDamage`).
    fn melee_damage(&mut self, weapon: Entity, user: Entity) -> (DamageSpecifier, bool);
    /// Лёгкий удар по сущности (`AttemptLightAttack`); без цели — промах в точку.
    fn attempt_light_attack(
        &mut self,
        user: Entity,
        weapon: Entity,
        target: Option<Entity>,
        at: MapCoordinates,
    ) -> bool;
}

impl CombatWorldExt for World {
    fn is_in_combat_mode(&self, entity: Entity) -> bool {
        self.get::<CombatMode>(entity)
            .is_some_and(|c| c.is_in_combat_mode)
    }

    fn set_in_combat_mode(&mut self, entity: Entity, value: bool) {
        let Some(mut component) = self.get_mut::<CombatMode>(entity) else {
            return;
        };
        if component.is_in_combat_mode == value {
            return;
        }
        component.is_in_combat_mode = value;
        if let Some(action) = component.combat_toggle_action_entity {
            self.set_toggled(action, value);
        }
    }

    fn melee_weapon_of(&mut self, entity: Entity) -> Option<Entity> {
        let mut event = GetMeleeWeapon::default();
        self.raise(entity, &mut event);
        if event.handled {
            return event
                .weapon
                .filter(|weapon| self.get::<MeleeWeapon>(*weapon).is_some());
        }
        if let Some(held) = self.active_item(entity) {
            if self
                .get::<MeleeWeapon>(held)
                .is_some_and(|weapon| !weapon.must_be_equipped_to_use)
            {
                return Some(held);
            }
            // Рука занята настоящим предметом без оружия — бить нечем.
            self.get::<crate::virtual_item::VirtualItem>(held)?;
        }
        if let Some(gloves) = self.slot_entity(entity, "gloves")
            && self.get::<MeleeWeapon>(gloves).is_some()
        {
            return Some(gloves);
        }
        self.get::<MeleeWeapon>(entity).is_some().then_some(entity)
    }

    fn melee_damage(&mut self, weapon: Entity, user: Entity) -> (DamageSpecifier, bool) {
        let Some(component) = self.get::<MeleeWeapon>(weapon).cloned() else {
            return (DamageSpecifier::default(), false);
        };
        let mut event = GetMeleeDamage {
            weapon,
            user,
            damage: component.damage.clone(),
            resistance_bypass: component.resistance_bypass,
        };
        self.raise(weapon, &mut event);
        (event.damage, event.resistance_bypass)
    }

    fn attempt_light_attack(
        &mut self,
        user: Entity,
        weapon: Entity,
        target: Option<Entity>,
        at: MapCoordinates,
    ) -> bool {
        let Some(component) = self.get::<MeleeWeapon>(weapon).cloned() else {
            return false;
        };
        let now = self.resource::<GameTime>().cur_time().as_secs_f64();
        if component.next_attack > now || !self.is_in_combat_mode(user) {
            return false;
        }
        if !can_attack(self, user, target, Some(weapon), false) || target == Some(weapon) {
            return false;
        }
        // Удары по темпу оружия: пропущенные за время между тактами наносятся сразу.
        let rate = component.attack_rate.max(0.001);
        let fire_rate = 1.0 / f64::from(rate);
        let mut next = component.next_attack.max(now);
        let mut swings = 0;
        while next <= now {
            next += fire_rate;
            swings += 1;
        }
        let mut attempt = AttemptMelee::default();
        if let Some(mut stored) = self.get_mut::<MeleeWeapon>(weapon) {
            stored.next_attack = next;
            if stored.swing_beverage {
                stored.swing_left = !stored.swing_left;
            }
        }
        self.raise(weapon, &mut attempt);
        if attempt.cancelled {
            if let Some(message) = attempt.message {
                self.popup_entity_for(&message, weapon, user, PopupType::Small);
            }
            return false;
        }
        for _ in 0..swings {
            do_light_attack(self, user, weapon, target, at);
        }
        self.raise(user, &mut MeleeAttack { weapon });
        if let Some(mut stored) = self.get_mut::<MeleeWeapon>(weapon) {
            stored.attacking = true;
        }
        true
    }
}

/// Один лёгкий удар (`DoLightAttack`).
fn do_light_attack(
    world: &mut World,
    user: Entity,
    weapon: Entity,
    target: Option<Entity>,
    _at: MapCoordinates,
) {
    let Some(component) = world.get::<MeleeWeapon>(weapon).cloned() else {
        return;
    };
    let (damage, resistance_bypass) = world.melee_damage(weapon, user);
    let damage = scale_fixed(&damage, component.click_damage_modifier);
    let reachable = target.filter(|target| {
        world.is_alive(*target)
            && world.get::<Damageable>(*target).is_some()
            && world.in_range_unobstructed(user, *target, component.range)
    });
    let Some(target) = reachable else {
        let mut miss = MeleeHit {
            hit_entities: Vec::new(),
            user,
            weapon,
            base_damage: damage,
            bonus_damage: DamageSpecifier::default(),
            hit_sound_override: None,
            handled: false,
        };
        world.raise(weapon, &mut miss);
        world.play_entity(&component.swing_sound, weapon, None);
        return;
    };
    let mut hit = MeleeHit {
        hit_entities: vec![target],
        user,
        weapon,
        base_damage: damage.clone(),
        bonus_damage: DamageSpecifier::default(),
        hit_sound_override: None,
        handled: false,
    };
    world.raise(weapon, &mut hit);
    if hit.handled {
        return;
    }
    let mut attacked = Attacked {
        used: weapon,
        user,
        bonus_damage: DamageSpecifier::default(),
    };
    world.raise(target, &mut attacked);
    let modified = &(&damage + &hit.bonus_damage) + &attacked.bonus_damage;
    world.change_damage(target, &modified, resistance_bypass, true, Some(user));
    let kind = highest_damage_sound(world, &modified);
    play_hit_sound(
        world,
        target,
        kind.as_deref(),
        hit.hit_sound_override.as_ref(),
        &component,
    );
}

fn scale_fixed(damage: &DamageSpecifier, factor: FixedPoint2) -> DamageSpecifier {
    DamageSpecifier {
        types: damage
            .types
            .iter()
            .map(|(k, v)| (k.clone(), *v * factor))
            .collect(),
    }
}

/// Звук попадания (`MeleeSoundSystem.PlayHitSound`).
fn play_hit_sound(
    world: &mut World,
    target: Entity,
    damage_type: Option<&str>,
    hit_override: Option<&SoundSpecifier>,
    weapon: &MeleeWeapon,
) {
    if !world.is_alive(target) {
        return;
    }
    if let Some(sounds) = world.get::<MeleeSound>(target).cloned() {
        let specific = match damage_type {
            None => sounds.no_damage_sound.clone(),
            Some(kind) => sounds
                .sound_types
                .as_ref()
                .and_then(|types| types.get(kind))
                .or_else(|| {
                    sounds
                        .sound_groups
                        .as_ref()
                        .and_then(|groups| groups.get(kind))
                })
                .cloned(),
        };
        if let Some(sound) = specific {
            world.play_entity(&sound, target, None);
            return;
        }
    }
    let sound = hit_override
        .or(weapon.hit_sound.as_ref())
        .unwrap_or(&weapon.no_damage_sound)
        .clone();
    world.play_entity(&sound, target, None);
}

/// В режиме боя клик рукой проходит, только если рука пуста и цель — предмет
/// (`CombatModeCanHandInteract`).
pub fn combat_mode_can_hand_interact(
    world: &mut World,
    user: Entity,
    target: Option<Entity>,
) -> bool {
    let Some(target) = target else {
        return false;
    };
    if world.get::<crate::hands::Hands>(user).is_none() || world.active_item(user).is_some() {
        return false;
    }
    if world.get::<Item>(target).is_none() {
        return false;
    }
    let mut event = CombatModeShouldHandInteract { cancelled: false };
    world.raise(target, &mut event);
    !event.cancelled
}

/// Можно ли в режиме боя взять цель рукой (`CombatModeShouldHandInteractEvent`). На цели.
#[derive(Debug, Clone, Copy)]
pub struct CombatModeShouldHandInteract {
    pub cancelled: bool,
}

fn receive(world: &mut World, event: &mut NetEvent<LightAttack>) {
    let Some(user) = crate::hands::player(world, event.session) else {
        return;
    };
    let request = event.event.clone();
    let Some(weapon) = world.melee_weapon_of(user) else {
        return;
    };
    let entities = world.get_resource::<NetEntities>();
    if entities.and_then(|all| all.entity(request.weapon)) != Some(weapon) {
        return;
    }
    let target = request
        .target
        .and_then(|net| world.get_resource::<NetEntities>()?.entity(net));
    let (map, position, z) = request.coordinates;
    world.attempt_light_attack(user, weapon, target, MapCoordinates { map, position, z });
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<CombatMode>();
    world.register_data_component::<MeleeWeapon>();
    world.register_data_component::<MeleeSound>();
    world.register_net_serde::<CombatMode>();
    world.register_net_serde::<MeleeWeapon>();
    world.register_net_event::<LightAttack>();
    if !side.is_server() {
        // В режиме боя клик бьёт: клиент просит лёгкий удар (`MeleeWeaponSystem.Update`).
        world.bind_input(interaction_functions::USE, |world, args| {
            if args.state != BoundKeyState::Down {
                return false;
            }
            let Some(player) = args.player else {
                return false;
            };
            if !world.is_in_combat_mode(player) {
                return false;
            }
            let Some(weapon) = world.melee_weapon_of(player) else {
                return false;
            };
            let (Some(at), Some(net)) = (args.coordinates, world.get::<NetEntity>(weapon).copied())
            else {
                return false;
            };
            let target = args.target.and_then(|t| world.get::<NetEntity>(t).copied());
            world.send_net_event_to_server(&LightAttack {
                target,
                weapon: net,
                coordinates: (at.map, at.position, at.z),
            });
            false
        });
        return;
    }
    world.subscribe::<CombatMode, MapInit>(|world, entity, _| {
        let Some(prototype) = world
            .get::<CombatMode>(entity)
            .map(|c| c.combat_toggle_action.clone())
        else {
            return;
        };
        let action = world.add_action(entity, &prototype, None);
        if let Some(mut component) = world.get_mut::<CombatMode>(entity) {
            component.combat_toggle_action_entity = action;
        }
    });
    // Действие режима боя (`OnActionPerform`).
    world.subscribe::<CombatMode, ActionEvent>(|world, entity, event| {
        if event.handled || event.kind != "ToggleCombatActionEvent" {
            return;
        }
        event.handled = true;
        let value = !world.is_in_combat_mode(entity);
        world.set_in_combat_mode(entity, value);
        let key = if value {
            "action-popup-combat-enabled"
        } else {
            "action-popup-combat-disabled"
        };
        let text = loc(world, key, &[]);
        world.popup_entity_for(&text, event.performer, event.performer, PopupType::Small);
    });
    world.subscribe_net_event::<LightAttack>(receive);
}

#[cfg(test)]
mod tests;
