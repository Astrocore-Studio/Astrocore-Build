use std::collections::BTreeSet;

use super::*;
use crate::profiles::Gender;
use crate::stations::tests::test_station;
use crate::testing::{spawn_at, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: inventoryTemplate
  id: human
  slots:
  - name: shoes
    slotFlags: FEET
  - name: jumpsuit
    slotFlags: INNERCLOTHING
  - name: id
    slotFlags: IDCARD
    dependsOn: jumpsuit
  - name: back
    slotFlags: BACK
- type: itemSize
  id: Small
  weight: 2
- type: itemSize
  id: Huge
  weight: 16
- type: accessLevel
  id: Command
- type: accessLevel
  id: Engineering
- type: accessLevel
  id: Maintenance
- type: accessGroup
  id: AllAccess
  tags: [ Command, Engineering, Maintenance ]
- type: species
  id: Human
  name: species-name-human
  roundStart: true
  prototype: MobTestHuman
- type: species
  id: Moth
  name: species-name-moth
  roundStart: true
  prototype: MobTestHuman
- type: entity
  id: MobTestHuman
  components:
  - type: Inventory
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: MarkerBase
  abstract: true
- type: entity
  id: SpawnPointJobBase
  parent: MarkerBase
  abstract: true
  components:
  - type: SpawnPoint
    spawn_type: Job
- type: entity
  id: SpawnPointCaptain
  parent: SpawnPointJobBase
  components:
  - type: SpawnPoint
    job_id: Captain
- type: entity
  id: SpawnPointLatejoin
  parent: SpawnPointJobBase
  components:
  - type: SpawnPoint
    spawn_type: LateJoin
- type: entity
  id: Jumpsuit
  components:
  - type: Item
  - type: Clothing
    slots: [innerclothing]
- type: entity
  id: Jumpskirt
  parent: Jumpsuit
- type: entity
  id: MothJumpsuit
  parent: Jumpsuit
- type: entity
  id: Boots
  components:
  - type: Item
  - type: Clothing
    slots: [FEET]
- type: entity
  id: Backpack
  components:
  - type: Item
    size: Huge
  - type: Clothing
    slots: [BACK]
  - type: Storage
    grid:
    - 0,0,3,1
- type: entity
  id: Flash
  components:
  - type: Item
    size: Small
- type: entity
  id: Pen
  components:
  - type: Item
    size: Small
- type: entity
  id: CaptainIDCard
  components:
  - type: Item
  - type: IdCard
  - type: Access
- type: entity
  id: CaptainPDA
  components:
  - type: Item
  - type: Clothing
    slots: [IDCARD]
  - type: Pda
    id: CaptainIDCard
- type: entity
  id: BorgChassis
  components:
  - type: Item
- type: job
  id: Borg
  name: job-name-borg
  jobEntity: BorgChassis
- type: startingGear
  id: CaptainGear
  equipment:
    shoes: Boots
    id: CaptainPDA
  inhand: [ Pen ]
  storage:
    back: [ Flash ]
- type: roleLoadout
  id: JobCaptain
  groups: [ CaptainJumpsuit, CaptainBackpack, Trinkets ]
- type: loadoutGroup
  id: CaptainJumpsuit
  loadouts: [ MothOnlySuit, CaptainJumpsuit, CaptainJumpskirt ]
- type: loadoutGroup
  id: CaptainBackpack
  loadouts: [ CaptainBackpack ]
- type: loadoutGroup
  id: Trinkets
  minLimit: 0
  maxLimit: 3
  loadouts: [ TrinketPen ]
- type: loadoutEffectGroup
  id: MothOnly
  effects:
  - !type:SpeciesLoadoutEffect
    species: [ Moth ]
- type: loadout
  id: MothOnlySuit
  effects:
  - !type:GroupLoadoutEffect
    proto: MothOnly
  equipment:
    jumpsuit: MothJumpsuit
- type: loadout
  id: CaptainJumpsuit
  equipment:
    jumpsuit: Jumpsuit
- type: loadout
  id: CaptainJumpskirt
  equipment:
    jumpsuit: Jumpskirt
- type: loadout
  id: CaptainBackpack
  equipment:
    back: Backpack
- type: loadout
  id: TrinketPen
  inhand: [ Pen ]
"#;

fn prototypes() -> String {
    format!("{PROTOTYPES}{}", crate::stations::tests::PROTOTYPES)
}

fn prototype_of(world: &World, entity: Option<Entity>) -> Option<String> {
    world
        .get::<MetaData>(entity?)?
        .prototype
        .as_deref()
        .map(str::to_owned)
}

fn profile(species: &str) -> HumanoidCharacterProfile {
    HumanoidCharacterProfile {
        name: "Urist McHands".into(),
        species: species.into(),
        gender: Gender::Male,
        ..HumanoidCharacterProfile::default()
    }
}

#[test]
fn spawn_points_follow_job_and_round_stage() {
    let mut world = world_with(&prototypes());
    let (station, grid) = test_station(&mut world);
    let captain = spawn_at(&mut world, "SpawnPointCaptain", grid, 1, 1);
    let late = spawn_at(&mut world, "SpawnPointLatejoin", grid, 5, 5);
    // Поле родителя сохраняется при наследовании: точка капитана — тоже `Job`.
    assert_eq!(
        world.get::<SpawnPoint>(captain).unwrap(),
        &SpawnPoint {
            job: Some("Captain".into()),
            spawn_type: SpawnPointType::Job
        }
    );
    let at = |world: &World, e| world.coordinates(e).unwrap();
    assert_eq!(
        spawn_positions(&mut world, Some(station), Some("Captain"), false),
        [at(&world, captain)]
    );
    assert_eq!(
        spawn_positions(&mut world, Some(station), Some("Captain"), true),
        [at(&world, late)]
    );
    // Должности без своей точки — любая точка должности… а её нет: запасная — первая любая.
    let fallback = spawn_positions(&mut world, Some(station), Some("Passenger"), false);
    assert_eq!(fallback.len(), 1);
}

#[test]
fn default_loadouts_respect_limits_and_species() {
    let world = world_with(&prototypes());
    let prototypes = world.resource::<Prototypes>();
    let mut loadout = RoleLoadout {
        role: "JobCaptain".into(),
        ..RoleLoadout::default()
    };
    set_default_loadout(prototypes, &mut loadout, &profile("Human"));
    // Костюм только для мотылька пропущен; безделушки без минимума не выбраны.
    assert_eq!(loadout.selected["CaptainJumpsuit"], ["CaptainJumpsuit"]);
    assert_eq!(loadout.selected["CaptainBackpack"], ["CaptainBackpack"]);
    assert!(loadout.selected["Trinkets"].is_empty());
    let mut moth = RoleLoadout {
        role: "JobCaptain".into(),
        ..RoleLoadout::default()
    };
    set_default_loadout(prototypes, &mut moth, &profile("Moth"));
    assert_eq!(moth.selected["CaptainJumpsuit"], ["MothOnlySuit"]);
    // Выбор игрока не перезаписывается.
    let mut chosen = RoleLoadout {
        role: "JobCaptain".into(),
        selected: [(
            "CaptainJumpsuit".to_owned(),
            vec!["CaptainJumpskirt".to_owned()],
        )]
        .into(),
        ..RoleLoadout::default()
    };
    set_default_loadout(prototypes, &mut chosen, &profile("Human"));
    assert_eq!(chosen.selected["CaptainJumpsuit"], ["CaptainJumpskirt"]);
    let gear = starting_gear(prototypes, "CaptainGear").unwrap();
    assert_eq!(gear.equipment["id"], "CaptainPDA");
    assert_eq!(gear.inhand, ["Pen"]);
    assert_eq!(gear.storage["back"], ["Flash"]);
}

#[test]
fn captain_spawns_dressed_with_named_id_and_access() {
    let mut world = world_with(&prototypes());
    let (station, grid) = test_station(&mut world);
    spawn_at(&mut world, "SpawnPointCaptain", grid, 1, 1);
    let mut wanted = profile("Human");
    wanted.loadouts.insert(
        "JobCaptain".into(),
        RoleLoadout {
            role: "JobCaptain".into(),
            selected: [(
                "CaptainJumpsuit".to_owned(),
                vec!["CaptainJumpskirt".to_owned()],
            )]
            .into(),
            ..RoleLoadout::default()
        },
    );
    let body = spawn_player_character_on_station(
        &mut world,
        Some(station),
        Some("Captain"),
        Some(&wanted),
        false,
    )
    .unwrap();
    assert_eq!(
        world.get::<MetaData>(body).unwrap().name.as_deref(),
        Some("Urist McHands")
    );
    // Снаряжение роли: юбка из профиля и рюкзак по умолчанию; стартовое — обувь, КПК,
    // ручка в руке, вспышка в рюкзаке.
    let slot = |world: &World, name| prototype_of(world, world.slot_entity(body, name));
    assert_eq!(slot(&world, "jumpsuit").as_deref(), Some("Jumpskirt"));
    assert_eq!(slot(&world, "back").as_deref(), Some("Backpack"));
    assert_eq!(slot(&world, "shoes").as_deref(), Some("Boots"));
    assert_eq!(slot(&world, "id").as_deref(), Some("CaptainPDA"));
    let held: Vec<_> = world
        .held_items(body)
        .into_iter()
        .map(|e| prototype_of(&world, Some(e)))
        .collect();
    assert_eq!(held, [Some("Pen".to_owned())]);
    let backpack = world.slot_entity(body, "back").unwrap();
    let inside = world
        .get::<crate::storage::Storage>(backpack)
        .map(|s| s.stored_items.len())
        .unwrap_or_default();
    assert_eq!(inside, 1);
    // Карта в КПК: имя, должность, доступ группы должности.
    let card = worn_id_card(&world, body).unwrap();
    let id = world.get::<IdCard>(card).unwrap();
    assert_eq!(id.full_name.as_deref(), Some("Urist McHands"));
    assert_eq!(id.job_title.as_deref(), Some("job-name-captain"));
    assert_eq!(id.job_icon.as_deref(), Some("JobIconCaptain"));
    assert_eq!(
        world.get::<MetaData>(card).unwrap().name.as_deref(),
        Some("access-id-card-component-owner-full-name-job-title-text")
    );
    assert_eq!(
        world.get::<Access>(card).unwrap().tags,
        BTreeSet::from(["Command".into(), "Engineering".into(), "Maintenance".into()])
    );
}

#[test]
fn job_entities_and_extended_access() {
    let mut world = world_with(&prototypes());
    let (station, grid) = test_station(&mut world);
    let at = world.coordinates(grid).unwrap();
    // Должность со своей сущностью — она и появляется.
    let borg = spawn_player_mob(
        &mut world,
        at,
        Some("Borg"),
        Some(&profile("Human")),
        Some(station),
    );
    assert_eq!(prototype_of(&world, borg).as_deref(), Some("BorgChassis"));
    // Расширенный доступ добавляет уровни должности.
    let card = world.spawn_attached("CaptainIDCard", at).unwrap();
    set_access_to_job(&mut world, card, "Passenger", false);
    assert_eq!(
        world.get::<Access>(card).unwrap().tags,
        BTreeSet::from(["Maintenance".into()])
    );
    set_access_to_job(&mut world, card, "Passenger", true);
    assert_eq!(world.get::<Access>(card).unwrap().tags.len(), 2);
}
