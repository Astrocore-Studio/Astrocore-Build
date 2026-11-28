//! Случаи из `SolutionTests` SS14 @ 2f4a2aad.

use super::*;

const WATER: &str = "water";
const FIRE: &str = "fire";
const EARTH: &str = "earth";

fn yaml<T: serde::de::DeserializeOwned>(text: &str) -> T {
    let node = astrocore_data::parse_yaml(text).unwrap();
    astrocore_data::from_node(&node).unwrap()
}

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn two(water: i32, fire: i32) -> Solution {
    let mut solution = Solution::default();
    solution.add_reagent(ReagentId::new(WATER), n(water));
    solution.add_reagent(ReagentId::new(FIRE), n(fire));
    solution
}

#[test]
fn split_keeps_ratios_and_rounds_towards_the_taker() {
    let mut solution = two(1000, 2000);
    let split = solution.split(n(750));
    assert_eq!(solution.prototype_quantity(WATER), n(750));
    assert_eq!(solution.prototype_quantity(FIRE), n(1500));
    assert_eq!(split.prototype_quantity(WATER), n(250));
    assert_eq!(split.volume, n(750));

    let mut solution = two(1, 2);
    let split = solution.split(n(1));
    assert_eq!(
        solution.prototype_quantity(WATER),
        FixedPoint2::from_cents(66)
    );
    assert_eq!(
        solution.prototype_quantity(FIRE),
        FixedPoint2::from_cents(134)
    );
    assert_eq!(split.prototype_quantity(WATER), FixedPoint2::from_cents(34));
    assert_eq!(split.prototype_quantity(FIRE), FixedPoint2::from_cents(66));

    let mut solution = two(1, 2);
    let split = solution.split(n(2));
    assert_eq!(
        solution.prototype_quantity(WATER),
        FixedPoint2::from_cents(33)
    );
    assert_eq!(
        solution.prototype_quantity(FIRE),
        FixedPoint2::from_cents(67)
    );
    assert_eq!(split.prototype_quantity(WATER), FixedPoint2::from_cents(67));
    assert_eq!(split.volume, n(2));

    for (initial, reduce, remainder) in [(3, 1, 2), (3, 2, 1)] {
        let mut solution = Solution::of(WATER, FixedPoint2::from_cents(initial));
        let split = solution.split(FixedPoint2::from_cents(reduce));
        assert_eq!(solution.volume, FixedPoint2::from_cents(remainder));
        assert_eq!(split.volume, FixedPoint2::from_cents(reduce));
    }
    for amount in [2, 10, 100, 1000] {
        let mut solution = Solution::default();
        for reagent in ["foo", "bar", "baz"] {
            solution.add_reagent(ReagentId::new(reagent), n(amount));
        }
        assert_eq!(solution.split(n(5)).volume, n(5));
    }
    let mut solution = Solution::of(WATER, n(800));
    assert_eq!(solution.split(n(1000)).volume, n(800));
    assert!(solution.is_empty());
}

#[test]
fn scale_remove_and_add() {
    let mut solution = two(20, 30);
    solution.scale_int(2);
    assert_eq!(solution.volume, n(100));
    let mut solution = two(20, 30);
    solution.scale(0.3);
    assert_eq!(solution.prototype_quantity(WATER), n(6));
    assert_eq!(solution.prototype_quantity(FIRE), n(9));
    assert_eq!(solution.volume, n(15));
    let mut solution = two(20, 30);
    solution.scale(0.0);
    assert!(solution.contents.is_empty());

    let mut solution = two(1000, 2000);
    solution.remove_solution(n(1500));
    assert_eq!(solution.prototype_quantity(WATER), n(500));
    assert_eq!(solution.prototype_quantity(FIRE), n(1000));

    let mut solution = two(1000, 2000);
    assert_eq!(solution.remove_prototype(WATER, n(5000)), n(1000));
    assert_eq!(solution.volume, n(2000));
    assert_eq!(solution.remove_prototype(EARTH, n(5)), FixedPoint2::ZERO);

    let mut one = two(1000, 2000);
    let mut other = Solution::default();
    other.add_reagent(ReagentId::new(WATER), n(500));
    other.add_reagent(ReagentId::new(EARTH), n(1000));
    one.add_solution(&other, &ReagentCatalog::default());
    assert_eq!(one.prototype_quantity(WATER), n(1500));
    assert_eq!(one.prototype_quantity(EARTH), n(1000));
    assert_eq!(one.volume, n(4500));
}

#[test]
fn reagents_with_different_data_stay_apart() {
    let mut solution = Solution::of("Blood", n(10));
    let dna = ReagentId {
        prototype: "Blood".into(),
        data: vec![ReagentData::Dna { dna: "ACGT".into() }],
    };
    solution.add_reagent(dna.clone(), n(5));
    assert_eq!(solution.contents.len(), 2);
    assert_eq!(solution.reagent_quantity(&dna), n(5));
    assert_eq!(solution.prototype_quantity("Blood"), n(15));
    // Без учёта данных уходит пропорционально из обеих записей.
    let removed = solution.remove_reagent(&ReagentId::new("Blood"), n(3), true);
    assert_eq!(removed, n(3));
    assert_eq!(solution.prototype_quantity("Blood"), n(12));
}

#[test]
fn mixing_temperatures_keeps_heat() {
    let mut catalog = ReagentCatalog::default();
    for (id, heat) in [(WATER, 2.0), (FIRE, 1.0)] {
        catalog.reagents.insert(
            id.into(),
            ReagentInfo {
                id: id.into(),
                name: String::new(),
                group: String::new(),
                desc: String::new(),
                physical_desc: String::new(),
                color: Color::WHITE,
                specific_heat: heat,
                boiling_point: None,
                melting_point: None,
                recognizable: false,
                metabolism_rate: None,
                price_per_unit: 0.0,
            },
        );
    }
    let mut cold = Solution::of(WATER, n(10));
    cold.temperature = 300.0;
    let mut hot = Solution::of(FIRE, n(20));
    hot.temperature = 400.0;
    cold.add_solution(&hot, &catalog);
    // (10·2·300 + 20·1·400) / (10·2 + 20·1) = 350.
    assert!((cold.temperature - 350.0).abs() < 1e-3);
}

#[test]
fn data_form_matches_ss14() {
    let solution: Solution = yaml(
        "maxVol: 60\nreagents:\n- ReagentId: Leporazine\n  data: []\n  Quantity: 60\ncanReact: True\n",
    );
    assert_eq!(solution.volume, n(60));
    assert_eq!(solution.max_volume, n(60));
    let empty: Solution = yaml("reagents:\n- ReagentId: Water\n  Quantity: 15\n");
    // Без вместимости — по объёму.
    assert_eq!(empty.max_volume, n(15));
    assert_eq!(empty.temperature, DEFAULT_TEMPERATURE);
}
