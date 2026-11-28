use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::{CARBON_DIOXIDE, CELL_VOLUME, OXYGEN, PLASMA};
use crate::atmos::{ensure_grid_atmosphere, tile_mixture, with_tile_mixture};
use crate::damage::Damageable;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Asphyxiation
  name: damage-type-asphyxiation
- type: damageType
  id: Poison
  name: damage-type-poison
- type: gas
  id: Oxygen
  name: gas-oxygen
  reagent: Oxygen
- type: gas
  id: Plasma
  name: gas-plasma
  reagent: Plasma
- type: gas
  id: CarbonDioxide
  name: gas-carbon-dioxide
  reagent: CarbonDioxide
- type: reagent
  id: Oxygen
  metabolisms:
    Respiration:
      effects:
      - !type:Oxygenate
        conditions:
        - !type:MetabolizerTypeCondition
          type: [ Human, Animal ]
      - !type:ModifyLungGas
        conditions:
        - !type:MetabolizerTypeCondition
          type: [ Vox ]
          inverted: true
        ratios:
          CarbonDioxide: 1.0
          Oxygen: -1.0
      - !type:HealthChange
        conditions:
        - !type:MetabolizerTypeCondition
          type: [ Vox ]
        damage:
          types:
            Poison: 0.7
      - !type:AdjustAlert
        alertType: Toxins
- type: reagent
  id: Plasma
  metabolisms:
    Respiration:
      effects:
      - !type:HealthChange
        ignoreResistances: true
        damage:
          types:
            Poison: 0.1
- type: reagent
  id: CarbonDioxide
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  id: TestLungs
  components:
  - type: Organ
    category: Lungs
  - type: Lung
  - type: Metabolizer
    stages: [ Respiration ]
    metabolizerTypes: [ Human ]
- type: entity
  id: Breather
  components:
  - type: Body
  - type: InitialBody
    organs:
      Lungs: TestLungs
  - type: Damageable
  - type: Injurable
  - type: Respirator
    damage:
      types:
        Asphyxiation: 1.0
    damageRecovery:
      types:
        Asphyxiation: -1.0
- type: entity
  id: NoLungs
  components:
  - type: Damageable
  - type: Injurable
  - type: Respirator
    damage:
      types:
        Asphyxiation: 1.0
    damageRecovery:
      types:
        Asphyxiation: -1.0
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn setup(air: Option<GasMixture>) -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    if let Some(air) = air {
        // Вся станция с воздухом, чтобы он не уходил в соседние пустые клетки.
        for x in 0..16 {
            for y in 0..16 {
                with_tile_mixture(&mut world, grid, TileCoord::new(x, y, 0), |tile| {
                    tile.copy_from(&air)
                });
            }
        }
    }
    (world, grid)
}

fn damage(world: &World, entity: Entity, kind: &str) -> f32 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get(kind))
        .map_or(0.0, |v| v.float())
}

fn saturation(world: &World, entity: Entity) -> f32 {
    world.get::<Respirator>(entity).unwrap().saturation
}

#[test]
fn breathing_air_keeps_saturation() {
    let (mut world, grid) = setup(Some(GasMixture::standard_air(CELL_VOLUME)));
    let breather = spawn_at(&mut world, "Breather", grid, 8, 8);
    assert_eq!(lungs_of(&world, breather).len(), 1);
    // 0,5 л воздуха — 0,0208 моля, кислорода 21 %: 0,00437 моля × 1144 ≈ 5 насыщения на
    // вдох раз в 4 с при расходе 2 каждые 2 с: насыщение ходит между 1 и 5.
    // Воздух тестовой станции уходит в космос через края: клетку дышащего подпитываем.
    for _ in 0..36 {
        with_tile_mixture(&mut world, grid, TileCoord::new(8, 8, 0), |tile| {
            tile.copy_from(&GasMixture::standard_air(CELL_VOLUME))
        });
        ticks(&mut world, 10);
        assert!(
            saturation(&world, breather) >= 0.9,
            "{}",
            saturation(&world, breather)
        );
    }
    assert_eq!(damage(&world, breather, "Asphyxiation"), 0.0);
}

#[test]
fn oxygen_turns_into_carbon_dioxide() {
    let (mut world, grid) = setup(Some(GasMixture::standard_air(CELL_VOLUME)));
    let breather = spawn_at(&mut world, "Breather", grid, 8, 8);
    // Вдох на 2-й секунде, переработка на 3-й: в воздухе лёгких кислород стал углекислым
    // газом.
    ticks(&mut world, 30 * 3 + 5);
    let lung = lungs_of(&world, breather)[0];
    let air = &world.get::<Lung>(lung).unwrap().air;
    assert!(air.moles[CARBON_DIOXIDE] > 0.004, "{:?}", air.moles);
    assert!(air.moles[OXYGEN] < 0.0001, "{:?}", air.moles);
    // Выдох на 4-й секунде отдаёт его в клетку.
    ticks(&mut world, 30);
    let tile = tile_mixture(&world, grid, TileCoord::new(8, 8, 0)).unwrap();
    assert!(tile.moles[CARBON_DIOXIDE] > 0.0);
}

#[test]
fn vacuum_suffocates() {
    let (mut world, grid) = setup(None);
    let breather = spawn_at(&mut world, "Breather", grid, 8, 8);
    // Насыщение 5, минус 2 за цикл: ниже нуля на третьем цикле (6 с), дальше 1 урона за цикл.
    ticks(&mut world, 30 * 5 + 5);
    assert_eq!(damage(&world, breather, "Asphyxiation"), 0.0);
    ticks(&mut world, 30 * 6);
    let asphyxiation = damage(&world, breather, "Asphyxiation");
    assert!((2.0..=4.0).contains(&asphyxiation), "{asphyxiation}");
    // Без лёгких не помогает и воздух.
    let (mut world, grid) = setup(Some(GasMixture::standard_air(CELL_VOLUME)));
    let lungless = spawn_at(&mut world, "NoLungs", grid, 8, 8);
    for _ in 0..36 {
        with_tile_mixture(&mut world, grid, TileCoord::new(8, 8, 0), |tile| {
            tile.copy_from(&GasMixture::standard_air(CELL_VOLUME))
        });
        ticks(&mut world, 10);
    }
    assert!(damage(&world, lungless, "Asphyxiation") > 0.0);
}

#[test]
fn plasma_poisons() {
    let mut air = GasMixture::standard_air(CELL_VOLUME);
    air.adjust_moles(PLASMA, 20.0);
    let (mut world, grid) = setup(Some(air));
    let breather = spawn_at(&mut world, "Breather", grid, 8, 8);
    ticks(&mut world, 30 * 4);
    // 0,5 л: доля плазмы 20 / 123,9 → 0,00336 моля × 1144 ≈ 3,8 единицы × 0,1 = 0,38 яда.
    let poison = damage(&world, breather, "Poison");
    assert!((0.3..0.45).contains(&poison), "{poison}");
}

#[test]
fn effect_data_parses() {
    let mut world = world_with(PROTOTYPES);
    let oxygen = crate::metabolism::reagent_metabolisms(&mut world, "Oxygen");
    let effects = &oxygen.stages[RESPIRATION_STAGE].effects;
    let names: Vec<&str> = effects.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["Oxygenate", "ModifyLungGas", "HealthChange", "AdjustAlert"]
    );
    assert!(effects[1].conditions[0].inverted);
    let dioxide = crate::metabolism::reagent_metabolisms(&mut world, "CarbonDioxide");
    assert!(dioxide.stages.is_empty());
}

#[test]
fn lungs_hold_breath_in_a_solution() {
    let (mut world, grid) = setup(Some(GasMixture::standard_air(CELL_VOLUME)));
    let breather = spawn_at(&mut world, "Breather", grid, 8, 8);
    let lung = lungs_of(&world, breather)[0];
    let soln = containers::try_get_solution(&mut world, lung, LUNG_SOLUTION).unwrap();
    let solution = containers::solution(&world, soln).unwrap();
    assert_eq!(solution.max_volume, FixedPoint2::new(100));
    assert!(!solution.can_react);
    // Вдох на 2-й секунде: кислород в растворе до переработки на 3-й.
    ticks(&mut world, 30 * 2 + 5);
    let oxygen = containers::solution(&world, soln)
        .unwrap()
        .prototype_quantity("Oxygen");
    assert!(oxygen > FixedPoint2::new(4), "{oxygen}");
}
