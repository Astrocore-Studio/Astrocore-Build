use astrocore_map::{EntityCoordinates, TransformWorldExt};
use astrocore_math::Vec2;

use astrocore_ecs::LifecycleWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Blood
- type: entity
  id: Solution
  components:
  - type: Solution
- type: damageType
  id: Slash
  name: damage-type-slash
- type: damageModifierSet
  id: BloodlossHuman
  coefficients:
    Slash: 0.25
- type: entity
  id: Patient
  components:
  - type: Damageable
  - type: Injurable
  - type: MobState
  - type: Temperature
  - type: Bloodstream
- type: entity
  id: Analyzer
  components:
  - type: HealthAnalyzer
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn setup() -> (World, Entity, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let patient = spawn_at(&mut world, "Patient", grid, 3, 3);
    let analyzer = spawn_at(&mut world, "Analyzer", grid, 4, 3);
    (world, grid, patient, analyzer)
}

fn analyzer_of(world: &World, analyzer: Entity) -> HealthAnalyzer {
    world.get::<HealthAnalyzer>(analyzer).unwrap().clone()
}

#[test]
fn state_reports_blood_bleeding_and_temperature() {
    let (mut world, _, patient, _) = setup();
    assert_eq!(
        ui_state(&world, None, true),
        HealthAnalyzerUiState::default()
    );
    let state = ui_state(&world, Some(patient), true);
    assert_eq!(
        state.target_entity,
        world.get::<NetEntity>(patient).copied()
    );
    assert_eq!(state.blood_level, Some(1.0));
    assert!(!state.bleeding);
    assert!(state.temperature.is_some());
    assert!(state.scan_mode);
    world
        .get_mut::<crate::bloodstream::Bloodstream>(patient)
        .unwrap()
        .bleed_amount = 2.0;
    assert!(ui_state(&world, Some(patient), false).bleeding);
}

#[test]
fn tracking_pauses_out_of_range_and_stops_on_drop() {
    let (mut world, grid, patient, analyzer) = setup();
    begin_analyzing(&mut world, analyzer, patient);
    assert_eq!(analyzer_of(&world, analyzer).scanned_entity, Some(patient));
    assert!(analyzer_of(&world, analyzer).is_analyzer_active);
    // Пациент ушёл дальше 2,5 тайла — неактивно, но связь остаётся.
    world
        .set_coordinates(
            patient,
            EntityCoordinates::new(grid, Vec2::new(10.5, 3.5)),
            None,
        )
        .unwrap();
    ticks(&mut world, 40);
    let component = analyzer_of(&world, analyzer);
    assert!(!component.is_analyzer_active);
    assert_eq!(component.scanned_entity, Some(patient));
    // Вернулся — снова активно.
    world
        .set_coordinates(
            patient,
            EntityCoordinates::new(grid, Vec2::new(3.5, 3.5)),
            None,
        )
        .unwrap();
    ticks(&mut world, 40);
    assert!(analyzer_of(&world, analyzer).is_analyzer_active);
    // Выпустили из рук — слежение кончилось.
    world.raise(analyzer, &mut Dropped { user: patient });
    let component = analyzer_of(&world, analyzer);
    assert_eq!(component.scanned_entity, None);
    assert!(!component.is_analyzer_active);
}

#[test]
fn deleted_patient_stops_tracking() {
    let (mut world, _, patient, analyzer) = setup();
    begin_analyzing(&mut world, analyzer, patient);
    world.delete_entity(patient);
    ticks(&mut world, 40);
    assert_eq!(analyzer_of(&world, analyzer).scanned_entity, None);
}
