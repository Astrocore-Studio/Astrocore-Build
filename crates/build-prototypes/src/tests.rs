use super::*;

const RON_SRC: &str = r#"[
    (id: "base_item", abstract: true, components: {"Item": (size: 2, weight: 1.5)}),
    (id: "crowbar", parents: ["base_item"], name: Some("монтировка"),
     components: {"Item": (size: 3), "Tool": (qualities: ["Prying"])}),
]"#;

#[test]
fn ron_prototypes_inherit_and_merge_fields() {
    let mut r = PrototypeRegistry::new();
    assert_eq!(r.load_ron_str(RON_SRC, "t").unwrap(), 2);
    r.validate().unwrap();
    let c = r.spawnable("crowbar").unwrap();
    let item = c.component("Item").unwrap();
    assert_eq!(
        item.get("size").unwrap().as_int(),
        Some(3),
        "потомок перекрывает"
    );
    assert_eq!(
        item.get("weight").unwrap().as_f64(),
        Some(1.5),
        "унаследовано"
    );
    assert!(c.component("Tool").is_some());
    assert_eq!(c.name.as_deref(), Some("монтировка"));
    assert!(matches!(
        r.spawnable("base_item"),
        Err(ProtoError::Abstract(_))
    ));
    assert!(matches!(r.spawnable("nope"), Err(ProtoError::Unknown(_))));
}

#[test]
fn multiple_parents_apply_left_to_right_and_child_wins() {
    let mut r = PrototypeRegistry::new();
    r.load_ron_str(
        r#"[
        (id: "a", abstract: true, components: {"X": (v: 1, a: 1)}),
        (id: "b", abstract: true, components: {"X": (v: 2, b: 2)}),
        (id: "c", parents: ["a", "b"], components: {"X": (c: 3)}),
        (id: "d", parents: ["a", "b"], components: {"X": (v: 9)}),
    ]"#,
        "t",
    )
    .unwrap();
    let c = r.resolve("c").unwrap();
    let x = c.component("X").unwrap();
    assert_eq!(
        x.get("v").unwrap().as_int(),
        Some(2),
        "поздний родитель перекрывает ранний"
    );
    assert_eq!(x.get("a").unwrap().as_int(), Some(1));
    assert_eq!(x.get("b").unwrap().as_int(), Some(2));
    assert_eq!(x.get("c").unwrap().as_int(), Some(3));
    assert_eq!(
        r.resolve("d")
            .unwrap()
            .component("X")
            .unwrap()
            .get("v")
            .unwrap()
            .as_int(),
        Some(9)
    );
}

#[test]
fn broken_graphs_are_typed_errors() {
    let mut r = PrototypeRegistry::new();
    r.load_ron_str(
        r#"[(id: "a", parents: ["b"]), (id: "b", parents: ["a"])]"#,
        "t",
    )
    .unwrap();
    assert!(matches!(r.validate(), Err(ProtoError::Cycle(_))));
    let mut r = PrototypeRegistry::new();
    r.load_ron_str(r#"[(id: "a", parents: ["ghost"])]"#, "t")
        .unwrap();
    assert!(matches!(r.validate(), Err(ProtoError::UnknownParent(..))));
    let mut r = PrototypeRegistry::new();
    assert!(matches!(
        r.load_ron_str(r#"[(id: "a"), (id: "a")]"#, "t"),
        Err(ProtoError::Duplicate(_))
    ));
    assert!(matches!(
        r.load_ron_str(r#"[(id: "плохо id")]"#, "t"),
        Err(ProtoError::BadId(_))
    ));
    assert!(matches!(
        r.load_ron_str("(((", "t"),
        Err(ProtoError::Parse(..))
    ));
    // Слишком глубокая цепочка.
    let mut r = PrototypeRegistry::new();
    for i in 0..40 {
        let parent = if i == 0 {
            String::new()
        } else {
            format!("\"p{}\"", i - 1)
        };
        r.load_ron_str(&format!("[(id: \"p{i}\", parents: [{parent}])]"), "t")
            .unwrap();
    }
    assert!(matches!(r.resolve("p39"), Err(ProtoError::TooDeep(_))));
}

const SS14_SRC: &str = r#"
- type: entity
  abstract: true
  id: BaseItem
  name: item
  components:
  - type: Item
    size: Small
  - type: Sprite
    sprite: Objects/Tools/crowbar.rsi
    state: icon

- type: entity
  parent: BaseItem
  id: Crowbar
  name: crowbar
  description: A multipurpose tool.
  components:
  - type: Item
    size: Normal
    weight: 1.5
  - type: Tool
    qualities:
    - Prying

- type: entity
  id: Combined
  parent: [BaseItem, Crowbar]

- type: reagent
  id: Water
  name: water
"#;

#[test]
fn ss14_yaml_entities_convert_and_inherit_like_ron() {
    let mut r = PrototypeRegistry::new();
    let report = r.load_ss14_yaml_str(SS14_SRC, "ss14").unwrap();
    assert_eq!(
        report,
        Ss14Report {
            entities: 3,
            skipped_other_types: 1
        }
    );
    r.validate().unwrap();
    let c = r.spawnable("Crowbar").unwrap();
    let item = c.component("Item").unwrap();
    assert_eq!(item.get("size").unwrap().as_str(), Some("Normal"));
    assert_eq!(item.get("weight").unwrap().as_f64(), Some(1.5));
    let sprite = c.component("Sprite").unwrap();
    assert_eq!(
        sprite.get("state").unwrap().as_str(),
        Some("icon"),
        "от BaseItem"
    );
    assert!(matches!(
        c.component("Tool").unwrap().get("qualities").unwrap(),
        Data::List(l) if l == &[Data::Str("Prying".into())]
    ));
    assert_eq!(c.name.as_deref(), Some("crowbar"));
    assert!(r.spawnable("Combined").is_ok(), "parent-список");
    assert!(matches!(
        r.spawnable("BaseItem"),
        Err(ProtoError::Abstract(_))
    ));
}

#[test]
fn ss14_yaml_tags_and_bad_input_do_not_panic() {
    let mut r = PrototypeRegistry::new();
    r.load_ss14_yaml_str(
        "- type: entity\n  id: T\n  components:\n  - type: Damageable\n    damage: !type:DamageSpecifier\n      types:\n        Blunt: 5\n",
        "t",
    )
    .unwrap();
    let t = r.resolve("T").unwrap();
    let blunt = t
        .component("Damageable")
        .unwrap()
        .get("damage")
        .unwrap()
        .get("types")
        .unwrap()
        .get("Blunt")
        .unwrap();
    assert_eq!(blunt.as_int(), Some(5));
    for bad in ["{a: 1}", "- 5", "- type: entity\n", ":::"] {
        let mut r = PrototypeRegistry::new();
        assert!(r.load_ss14_yaml_str(bad, "t").is_err(), "{bad:?}");
    }
    assert!(PrototypeRegistry::new().load_ss14_yaml_str("", "t").is_ok());
}

#[test]
fn directories_load_both_formats_deterministically_and_refuse_huge_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.ron"), r#"[(id: "from_ron")]"#).unwrap();
    std::fs::write(
        dir.path().join("sub/b.yml"),
        "- type: entity\n  id: from_yaml\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "игнорируется").unwrap();
    let mut r = PrototypeRegistry::new();
    assert_eq!(r.load_dir(dir.path()).unwrap(), 2);
    assert_eq!(r.ids().collect::<Vec<_>>(), vec!["from_ron", "from_yaml"]);
    let big = tempfile::tempdir().unwrap();
    let f = std::fs::File::create(big.path().join("x.ron")).unwrap();
    f.set_len(MAX_FILE_BYTES + 1).unwrap();
    assert!(matches!(
        PrototypeRegistry::new().load_dir(big.path()),
        Err(ProtoError::TooLarge(_))
    ));
}
