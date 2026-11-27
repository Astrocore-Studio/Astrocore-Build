use astrocore_ecs::GameTime;

use super::*;
use crate::testing::world_with;

pub(crate) const PROTOTYPES: &str = r#"
- type: alertOrder
  id: Order
  order:
  - category: Health
  - alertType: Fire
  - category: Pressure
- type: alert
  id: Fire
  icons: [ /Textures/Interface/Alerts/Fire/fire.png ]
  clickEvent: !type:ResistFireAlertEvent
  name: alerts-on-fire-name
- type: alert
  id: LowPressure
  category: Pressure
  icons:
  - sprite: /Textures/Interface/Alerts/pressure.rsi
    state: lowpressure1
  - sprite: /Textures/Interface/Alerts/pressure.rsi
    state: lowpressure2
  maxSeverity: 2
- type: alert
  id: HighPressure
  category: Pressure
  icons:
  - sprite: /Textures/Interface/Alerts/pressure.rsi
    state: highpressure1
  - sprite: /Textures/Interface/Alerts/pressure.rsi
    state: highpressure2
  maxSeverity: 2
- type: alert
  id: HumanHealth
  category: Health
  icons:
  - sprite: /Textures/Interface/Alerts/human_alive.rsi
    state: health0
  - sprite: /Textures/Interface/Alerts/human_alive.rsi
    state: health1
  minSeverity: 0
  maxSeverity: 1
- type: entity
  id: TestMob
  components:
  - type: Alerts
"#;

fn mob(world: &mut World) -> Entity {
    world.spawn(Alerts::default()).id()
}

#[test]
fn prototypes_give_icons_by_severity_and_order() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let fire = alert_prototype(prototypes, "Fire").unwrap();
    assert_eq!(
        fire.icon(None),
        Some(&AlertIcon::Texture(
            "/Textures/Interface/Alerts/Fire/fire.png".into()
        ))
    );
    assert_eq!(fire.click_event.as_deref(), Some("ResistFireAlertEvent"));
    assert_eq!(fire.min_severity(), -1);
    let low = alert_prototype(prototypes, "LowPressure").unwrap();
    assert_eq!(
        low.icon(Some(2)),
        Some(&AlertIcon::Rsi {
            sprite: "/Textures/Interface/Alerts/pressure.rsi".into(),
            state: "lowpressure2".into()
        })
    );
    let health = alert_prototype(prototypes, "HumanHealth").unwrap();
    assert_eq!(health.min_severity(), 0);
    let order = AlertOrder::from_prototypes(prototypes);
    assert_eq!(order.compare(&health, &fire), Ordering::Less);
    assert_eq!(order.compare(&low, &fire), Ordering::Greater);
}

#[test]
fn category_keeps_one_alert_and_clears_together() {
    let mut world = world_with(PROTOTYPES);
    let entity = mob(&mut world);
    show_alert(&mut world, entity, "LowPressure", Some(1));
    show_alert(&mut world, entity, "HighPressure", Some(2));
    let alerts = active_alerts(&world, entity).unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts["category:Pressure"].alert_type, "HighPressure");
    assert!(is_showing_alert(&world, entity, "LowPressure"));
    assert!(is_showing_alert_category(&world, entity, "Pressure"));
    show_alert(&mut world, entity, "Fire", None);
    clear_alert_category(&mut world, entity, "Pressure");
    assert!(!is_showing_alert_category(&world, entity, "Pressure"));
    assert!(is_showing_alert(&world, entity, "Fire"));
    clear_alert(&mut world, entity, "Fire");
    assert!(active_alerts(&world, entity).unwrap().is_empty());
}

#[test]
fn auto_removed_alerts_expire_and_clicks_raise_events() {
    let mut world = world_with(PROTOTYPES);
    let entity = mob(&mut world);
    update_alert(
        &mut world,
        entity,
        "Fire",
        None,
        Some(Duration::from_secs(2)),
        true,
    );
    update(&mut world);
    assert!(is_showing_alert(&world, entity, "Fire"));
    let rate = u32::from(world.resource::<GameTime>().tick_rate);
    world.resource_mut::<GameTime>().tick.0 += rate * 3;
    update(&mut world);
    assert!(!is_showing_alert(&world, entity, "Fire"));
    // Нажатие поднимает событие на сущности.
    world.subscribe::<Alerts, AlertClicked>(|_, _, event| {
        event.handled = event.event == "ResistFireAlertEvent";
    });
    assert!(activate_alert(&mut world, entity, "Fire"));
    assert!(!activate_alert(&mut world, entity, "LowPressure"));
}
