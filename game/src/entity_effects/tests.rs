use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::testing::{spawn_at, station, world_with};

static APPLIED: AtomicU32 = AtomicU32::new(0);

#[derive(Debug)]
struct Count {
    scaled: bool,
}

impl EffectKind for Count {
    fn apply(&self, _: &mut World, _: Entity, scale: f32, _: Option<Entity>) {
        APPLIED.fetch_add((scale * 10.0) as u32, Ordering::SeqCst);
    }

    fn scaling(&self) -> bool {
        self.scaled
    }
}

#[derive(Debug)]
struct Always;

impl ConditionKind for Always {
    fn check(&self, _: &mut World, _: Entity, _: Option<Entity>) -> bool {
        true
    }
}

fn effects(world: &mut World, yaml: &str) -> Vec<EntityEffect> {
    let node = astrocore_data::parse_yaml(yaml).unwrap();
    parse_effects(world, Some(&node))
}

#[test]
fn common_fields_gate_and_scale_effects() {
    let mut world = world_with("- type: entity\n  id: Thing\n");
    let grid = station(&mut world);
    let thing = spawn_at(&mut world, "Thing", grid, 1, 1);
    world.register_effect("Count", |_| Some(Arc::new(Count { scaled: true })));
    world.register_effect("CountOnce", |_| Some(Arc::new(Count { scaled: false })));
    world.register_condition("Always", |_| Some(Arc::new(Always)));
    let list = effects(
        &mut world,
        r#"
- !type:Count
  minScale: 2
- !type:CountOnce
- !type:Count
  conditions:
  - !type:Always
    inverted: true
- !type:Count
  conditions:
  - !type:Always
- !type:Count
  conditions:
  - !type:Unknown
- !type:Count
  probability: 0
"#,
    );
    // Эффект с неизвестным условием не разбирается.
    assert_eq!(list.len(), 5);
    APPLIED.store(0, Ordering::SeqCst);
    apply_effects(&mut world, thing, &list, 3.0, None);
    // minScale 2 при 3 — да (30), без scaling — 1 (10), перевёрнутое условие — нет,
    // выполненное условие — да (30), вероятность 0 — нет.
    assert_eq!(APPLIED.load(Ordering::SeqCst), 70);
    let registry = world.resource::<EffectRegistry>();
    assert_eq!(registry.unknown.get("Unknown"), Some(&1));
}
