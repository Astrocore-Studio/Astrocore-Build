use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::{CELL_VOLUME, PLASMA};
use crate::atmos::{GasMixture, ensure_grid_atmosphere, is_hotspot_active, with_tile_mixture};
use crate::temperature::temperature_of;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Heat
  name: damage-type-heat
- type: entity
  id: Torch
  components:
  - type: Damageable
  - type: Injurable
  - type: Temperature
  - type: Appearance
  - type: Flammable
    fireSpread: true
    damage:
      types:
        Heat: 1.5
- type: entity
  id: Mob
  parent: Torch
  components:
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape:
          !type:PhysShapeCircle
          radius: 0.35
        density: 185
        mask:
        - MobMask
        layer:
        - MobLayer
- type: entity
  id: Lighter
  components:
  - type: IgnitionSource
    ignited: true
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn fill(world: &mut World, grid: Entity, x: i32, y: i32, mixture: GasMixture) {
    with_tile_mixture(world, grid, TileCoord::new(x, y, 0), |air| {
        air.copy_from(&mixture)
    });
}

fn flammable(world: &World, entity: Entity) -> Flammable {
    world.get::<Flammable>(entity).unwrap().clone()
}

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    (world, grid)
}

#[test]
fn tile_fire_ignites_and_burns() {
    let (mut world, grid) = setup();
    fill(
        &mut world,
        grid,
        3,
        3,
        GasMixture::standard_air(CELL_VOLUME),
    );
    let torch = spawn_at(&mut world, "Torch", grid, 3, 3);
    // Пламя на 200 К горячее порога воспламенения: две «стопки».
    world.raise(
        torch,
        &mut TileFire {
            temperature: 573.15,
            volume: 100.0,
        },
    );
    world.run_tick();
    let burning = flammable(&world, torch);
    assert!(burning.on_fire);
    assert!((burning.fire_stacks - 2.0).abs() < 0.001);
    assert_eq!(
        world.appearance(torch, ON_FIRE),
        Some(&astrocore_net::AppearanceValue::Bool(true))
    );
    // Через секунду: урон 1,5 × 2, нагрев 75 кДж на 3000 Дж/К, «стопки» −0,1.
    ticks(&mut world, 31);
    assert!((world.total_damage(torch).float() - 3.0).abs() < 0.01);
    assert!(temperature_of(&world, torch).unwrap() > 310.0);
    assert!((flammable(&world, torch).fire_stacks - 1.9).abs() < 0.001);
}

#[test]
fn no_oxygen_puts_fire_out() {
    let (mut world, grid) = setup();
    let torch = spawn_at(&mut world, "Torch", grid, 5, 5);
    set_fire_stacks(&mut world, torch, 3.0, true);
    assert!(flammable(&world, torch).on_fire);
    ticks(&mut world, 31);
    let out = flammable(&world, torch);
    assert!(!out.on_fire);
    assert_eq!(out.fire_stacks, 0.0);
}

#[test]
fn hot_item_ignites_and_burning_spreads() {
    let (mut world, grid) = setup();
    let torch = spawn_at(&mut world, "Torch", grid, 3, 3);
    let other = spawn_at(&mut world, "Torch", grid, 3, 3);
    let lighter = spawn_at(&mut world, "Lighter", grid, 3, 3);
    // Без «стопок» поджечь нельзя.
    let mut event = InteractUsing {
        user: lighter,
        used: lighter,
        target: torch,
        click: world.map_coordinates(torch).unwrap(),
        handled: false,
    };
    world.raise(torch, &mut event);
    assert!(event.handled);
    assert!(!flammable(&world, torch).on_fire);
    set_fire_stacks(&mut world, torch, 4.0, false);
    event.handled = false;
    world.raise(torch, &mut event);
    assert!(flammable(&world, torch).on_fire);
    // Касание горящего: «стопки» поровну (массы по 1).
    let (first, second) = if torch.index() < other.index() {
        (torch, other)
    } else {
        (other, torch)
    };
    world.raise(
        first,
        &mut StartCollide {
            our_entity: first,
            other_entity: second,
            our_fixture: "flammable".into(),
            other_fixture: "fix1".into(),
        },
    );
    assert!(flammable(&world, other).on_fire);
    assert!((flammable(&world, other).fire_stacks - 2.0).abs() < 0.001);
    assert!((flammable(&world, torch).fire_stacks - 2.0).abs() < 0.001);
}

#[test]
fn burning_body_ignites_fuel_around() {
    let (mut world, grid) = setup();
    let mut air = GasMixture::standard_air(CELL_VOLUME);
    air.adjust_moles(PLASMA, 20.0);
    fill(&mut world, grid, 3, 3, air);
    let torch = spawn_at(&mut world, "Torch", grid, 3, 3);
    set_fire_stacks(&mut world, torch, 3.0, true);
    ticks(&mut world, 33);
    assert!(world.get::<IgnitionSource>(torch).unwrap().ignited);
    assert!(is_hotspot_active(&world, grid, TileCoord::new(3, 3, 0)));
}

#[test]
fn plasma_fire_on_tile_sets_body_alight() {
    let (mut world, grid) = setup();
    let mut air = GasMixture::standard_air(CELL_VOLUME);
    air.adjust_moles(PLASMA, 15.0);
    fill(&mut world, grid, 3, 3, air);
    let mob = spawn_at(&mut world, "Mob", grid, 3, 3);
    hotspot_expose_at(&mut world, grid, TileCoord::new(3, 3, 0), 800.0, 50.0, true);
    // Пламя разгорается несколько шагов атмосферы.
    ticks(&mut world, 90);
    assert!(is_hotspot_active(&world, grid, TileCoord::new(3, 3, 0)));
    assert!(flammable(&world, mob).on_fire);
}

#[test]
fn fire_alert_shows_while_burning_and_resisting_puts_it_out_faster() {
    let prototypes = format!(
        "{PROTOTYPES}{}",
        r#"
- type: alert
  id: Fire
  icons: [ /Textures/Interface/Alerts/Fire/fire.png ]
  clickEvent: !type:ResistFireAlertEvent
"#
    );
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    fill(
        &mut world,
        grid,
        3,
        3,
        GasMixture::standard_air(CELL_VOLUME),
    );
    let mob = spawn_at(&mut world, "Mob", grid, 3, 3);
    world
        .entity_mut(mob)
        .insert(crate::alerts::Alerts::default());
    adjust_fire_stacks(&mut world, mob, 5.0, true);
    ticks(&mut world, 31);
    assert!(crate::alerts::is_showing_alert(&world, mob, "Fire"));
    let before = flammable(&world, mob).fire_stacks;
    // Нажатие на значок: две секунды «стопки» уходят в 15 раз быстрее.
    assert!(crate::alerts::activate_alert(&mut world, mob, "Fire"));
    assert!(flammable(&world, mob).resisting());
    ticks(&mut world, 31);
    let after = flammable(&world, mob).fire_stacks;
    assert!((before - after - 1.5).abs() < 0.01, "{before} → {after}");
    // Погас — значок снят.
    try_extinguish(&mut world, mob);
    ticks(&mut world, 31);
    assert!(!crate::alerts::is_showing_alert(&world, mob, "Fire"));
}

#[test]
fn fire_and_heat_effects_from_data() {
    let (mut world, grid) = setup();
    let torch = spawn_at(&mut world, "Torch", grid, 4, 4);
    let node = astrocore_data::parse_yaml(
        r#"
- !type:Flammable
  multiplier: 2
  multiplierOnExisting: 0.5
- !type:AdjustTemperature
  amount: 1000
"#,
    )
    .unwrap();
    let effects = crate::entity_effects::parse_effects(&mut world, Some(&node));
    assert_eq!(effects.len(), 2);
    let before = temperature_of(&world, torch).unwrap();
    crate::entity_effects::apply_effects(&mut world, torch, &effects, 1.0, None);
    // Без стопок — обычный множитель, со стопками — свой.
    assert_eq!(flammable(&world, torch).fire_stacks, 2.0);
    crate::entity_effects::apply_effects(&mut world, torch, &effects[..1], 1.0, None);
    assert_eq!(flammable(&world, torch).fire_stacks, 2.5);
    assert!(temperature_of(&world, torch).unwrap() > before);
    let ignite = astrocore_data::parse_yaml("- !type:Ignite").unwrap();
    let ignite = crate::entity_effects::parse_effects(&mut world, Some(&ignite));
    crate::entity_effects::apply_effects(&mut world, torch, &ignite, 1.0, None);
    assert!(flammable(&world, torch).on_fire);
}
