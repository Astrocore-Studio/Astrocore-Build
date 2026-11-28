use super::*;
use crate::solution::Solution;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Hydrogen
  name: reagent-name-hydrogen
  physicalDesc: reagent-physical-desc-gaseous
  specificHeat: 1
- type: reagent
  id: Oxygen
  name: reagent-name-oxygen
  physicalDesc: reagent-physical-desc-gaseous
  specificHeat: 1
- type: reagent
  id: Water
  name: reagent-name-water
  physicalDesc: reagent-physical-desc-translucent
  specificHeat: 1
- type: reagent
  id: Heavy
  name: reagent-name-heavy
  physicalDesc: reagent-physical-desc-translucent
  specificHeat: 2
- type: reagent
  id: Plasma
  name: reagent-name-plasma
  physicalDesc: reagent-physical-desc-gaseous
- type: reagent
  id: Ice
  name: reagent-name-ice
  physicalDesc: reagent-physical-desc-frosty
- type: reaction
  id: Water
  reactants:
    Hydrogen:
      amount: 2
    Oxygen:
      amount: 1
  products:
    Water: 3
- type: reaction
  id: HeavyWater
  reactants:
    Hydrogen:
      amount: 1
    Ice:
      amount: 1
  products:
    Heavy: 2
- type: reaction
  id: Ice
  maxTemp: 270
  reactants:
    Water:
      amount: 1
  products:
    Ice: 1
- type: reaction
  id: Mix
  requiredMixerCategories:
  - Stir
  reactants:
    Plasma:
      amount: 1
  products:
    Ice: 1
- type: reaction
  id: Bubbles
  quantized: true
  conserveEnergy: false
  reactants:
    Plasma:
      amount: 2
    Oxygen:
      amount: 1
      catalyst: true
  effects:
  - !type:SpawnEntity
    entity: Spark
  - !type:AdjustSolutionThermalEnergy
    delta: 100
    maxTemp: 400
  - !type:SomethingNotPorted
- type: entity
  id: Spark
- type: entity
  id: Flask
  components:
  - type: Solution
    id: flask
    solution:
      maxVol: 100
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let flask = spawn_at(&mut world, "Flask", grid, 3, 3);
    (world, flask)
}

fn add(world: &mut World, flask: Entity, reagent: &str, amount: i32) {
    containers::try_add_reagent(world, flask, ReagentId::new(reagent), n(amount), None);
}

fn quantity(world: &World, flask: Entity, reagent: &str) -> FixedPoint2 {
    containers::solution(world, flask)
        .unwrap()
        .prototype_quantity(reagent)
}

#[test]
fn reagents_react_by_the_lowest_ratio() {
    let (mut world, flask) = setup();
    add(&mut world, flask, "Hydrogen", 10);
    // Пока нет кислорода — ничего.
    assert_eq!(quantity(&world, flask, "Water"), FixedPoint2::ZERO);
    add(&mut world, flask, "Oxygen", 3);
    // 3 единицы: −6 водорода, −3 кислорода, +9 воды.
    assert_eq!(quantity(&world, flask, "Water"), n(9));
    assert_eq!(quantity(&world, flask, "Hydrogen"), n(4));
    assert_eq!(quantity(&world, flask, "Oxygen"), FixedPoint2::ZERO);
}

#[test]
fn temperature_limits_and_chains() {
    let (mut world, flask) = setup();
    add(&mut world, flask, "Water", 5);
    assert_eq!(quantity(&world, flask, "Ice"), FixedPoint2::ZERO);
    // Остудить — вода замерзает.
    containers::set_temperature(&mut world, flask, 260.0);
    assert_eq!(quantity(&world, flask, "Ice"), n(5));
    // Цепочка в холодной колбе: вода из водорода и кислорода сразу замерзает.
    let (mut world, flask) = setup();
    containers::set_temperature(&mut world, flask, 260.0);
    add(&mut world, flask, "Oxygen", 1);
    add(&mut world, flask, "Hydrogen", 2);
    assert_eq!(quantity(&world, flask, "Ice"), n(3));
    assert_eq!(quantity(&world, flask, "Water"), FixedPoint2::ZERO);
}

#[test]
fn mixer_categories_quantized_catalysts_and_effects() {
    let (mut world, flask) = setup();
    // Без мешалки реакция «Mix» не идёт, хотя квантованная «Bubbles» тоже ждёт кислорода.
    add(&mut world, flask, "Plasma", 5);
    assert_eq!(quantity(&world, flask, "Ice"), FixedPoint2::ZERO);
    fully_react_solution(&mut world, flask, Some(&["Stir".to_owned()]));
    assert_eq!(quantity(&world, flask, "Ice"), n(5));

    let (mut world, flask) = setup();
    let sparks_before = world
        .query::<&astrocore_ecs::MetaData>()
        .iter(&world)
        .count();
    add(&mut world, flask, "Oxygen", 1);
    add(&mut world, flask, "Plasma", 5);
    // Квантованная: 2 целых единицы, 1 плазма остаётся; кислород — катализатор.
    assert_eq!(quantity(&world, flask, "Plasma"), n(1));
    assert_eq!(quantity(&world, flask, "Oxygen"), n(1));
    // Эффекты: 2 искры (по одной на единицу), +200 тепла при теплоёмкости 2.
    let sparks_after = world
        .query::<&astrocore_ecs::MetaData>()
        .iter(&world)
        .count();
    assert_eq!(sparks_after - sparks_before, 2);
    let temperature = containers::solution(&world, flask).unwrap().temperature;
    assert!((temperature - (293.15 + 100.0)).abs() < 0.01);
    // Неизвестный эффект учтён.
    let registry = world.resource::<crate::entity_effects::EffectRegistry>();
    assert_eq!(registry.unknown.get("SomethingNotPorted"), Some(&1));
}

#[test]
fn reactions_keep_heat_and_order_by_priority() {
    let mut catalog = ReactionCatalog::default();
    let reaction = |id: &str, priority, products: usize| Reaction {
        id: id.into(),
        name: String::new(),
        reactants: vec![(
            "A".into(),
            Reactant {
                amount: n(1),
                catalyst: false,
            },
        )],
        min_temperature: 0.0,
        max_temperature: f32::INFINITY,
        conserve_energy: true,
        mixing_categories: None,
        products: (0..products).map(|i| (format!("P{i}"), n(1))).collect(),
        effects: Vec::new(),
        effect_kinds: Vec::new(),
        impact: LogImpact::Low,
        sound: None,
        quantized: false,
        priority,
    };
    catalog.reactions = vec![
        reaction("b", 0, 1),
        reaction("a", 0, 1),
        reaction("c", 0, 0),
        reaction("d", 5, 2),
    ];
    let mut keys: Vec<_> = catalog
        .reactions
        .iter()
        .map(|r| (r.order_key(), r.id.clone()))
        .collect();
    keys.sort();
    let order: Vec<String> = keys.into_iter().map(|(_, id)| id).collect();
    assert_eq!(order, ["d", "c", "a", "b"]);

    // Тепло сохраняется: 10 водорода и 10 льда при 300 К (теплоёмкость 20) дают 20
    // тяжёлой воды (теплоёмкость 40): 6000 / 40 = 150 К.
    let (mut world, flask) = setup();
    let mut hot = Solution::default();
    hot.add_reagent(ReagentId::new("Hydrogen"), n(10));
    hot.add_reagent(ReagentId::new("Ice"), n(10));
    hot.temperature = 300.0;
    containers::add_solution(&mut world, flask, &hot);
    let solution = containers::solution(&world, flask).unwrap();
    assert_eq!(solution.prototype_quantity("Heavy"), n(20));
    assert!((solution.temperature - 150.0).abs() < 0.01);
}
