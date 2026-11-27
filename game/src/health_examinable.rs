//! Осмотр здоровья: глагол осмотра «Здоровье» рядом с целью показывает раны по типам урона
//! (строка ближайшего пройденного порога 8–200 из `health-examinable-<префикс>-<тип>-<порог>`)
//! и то, что добавят другие системы (кровотечение, бледность). Образец —
//! `HealthExaminableComponent`, `HealthExaminableSystem` SS14 @ 2f4a2aad.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, Loc};
use astrocore_net::{Actor, NetEntity, NetEventWorldExt, NetTarget};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::Damageable;
use crate::examine::{ExamineMessage, loc};
use crate::fixed_point::FixedPoint2;
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Пороги урона для строк (`Thresholds`).
pub const THRESHOLDS: [i32; 7] = [8, 15, 30, 50, 75, 100, 200];

fn carbon() -> String {
    "carbon".into()
}

/// Можно осмотреть здоровье (`HealthExaminableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthExaminable {
    pub examinable_types: Vec<String>,
    #[serde(default = "carbon")]
    pub loc_prefix: String,
}

/// Здоровье осматривают (`HealthBeingExaminedEvent`): системы добавляют строки. На цели.
#[derive(Debug, Clone, Default)]
pub struct HealthBeingExamined {
    pub lines: Vec<String>,
}

/// Строка ближайшего пройденного порога урона; порог без строки пропускается.
pub fn threshold_message(
    value: FixedPoint2,
    prefix: &str,
    damage_type: &str,
    has: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut chosen = None;
    for threshold in THRESHOLDS {
        let id = format!("health-examinable-{prefix}-{damage_type}-{threshold}");
        if has(&id) && value > FixedPoint2::new(threshold) {
            chosen = Some(id);
        }
    }
    chosen
}

/// Строки осмотра здоровья (`CreateMarkup`).
pub fn health_markup(world: &mut World, entity: Entity) -> Vec<String> {
    let (Some(examinable), Some(damage)) = (
        world.get::<HealthExaminable>(entity).cloned(),
        world.get::<Damageable>(entity).map(|d| d.damage.clone()),
    ) else {
        return Vec::new();
    };
    let target = world.entity_arg(entity);
    let mut lines = Vec::new();
    for damage_type in &examinable.examinable_types {
        let Some(value) = damage.types.get(damage_type).copied() else {
            continue;
        };
        if value == FixedPoint2::ZERO {
            continue;
        }
        let has = |id: &str| world.get_resource::<Loc>().is_some_and(|l| l.has(id));
        if let Some(id) = threshold_message(value, &examinable.loc_prefix, damage_type, has) {
            lines.push(loc(world, &id, &[("target", target.clone().into())]));
        }
    }
    if lines.is_empty() {
        let id = format!("health-examinable-{}-none", examinable.loc_prefix);
        lines.push(loc(world, &id, &[("target", target.into())]));
    }
    let mut event = HealthBeingExamined { lines };
    world.raise(entity, &mut event);
    event.lines
}

/// Показать строки в окне осмотра (`SendExamineTooltip`).
fn send_tooltip(world: &mut World, user: Entity, target: Entity, lines: Vec<String>) {
    let (Some(session), Some(net)) = (
        world.get::<Actor>(user).map(|a| a.session),
        world.get::<NetEntity>(target).copied(),
    ) else {
        return;
    };
    let response = ExamineMessage::Response {
        target: net,
        name: world.entity_name(target),
        lines,
    };
    world.send_net_event(NetTarget::Session(session), &response);
}

fn on_get_verbs(world: &mut World, entity: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Examine || world.get::<Damageable>(entity).is_none() {
        return;
    }
    let user = event.user;
    let details = crate::examine::is_in_details_range(world, user, entity);
    let mut verb = Verb::new(
        VerbKind::Examine,
        loc(world, "health-examinable-verb-text", &[]),
        move |world| {
            let lines = health_markup(world, entity);
            send_tooltip(world, user, entity, lines);
        },
    );
    verb.disabled = !details;
    verb.message = (!details).then(|| loc(world, "health-examinable-verb-disabled", &[]));
    verb.icon = Some("Interface/VerbIcons/rejuvenate.svg.192dpi.png".into());
    event.verbs.push(verb);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<HealthExaminable>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<HealthExaminable, GetVerbs>(on_get_verbs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_passed_threshold_with_a_line_is_chosen() {
        let has = |id: &str| id.ends_with("-8") || id.ends_with("-30");
        let message = |value| threshold_message(FixedPoint2::new(value), "carbon", "Blunt", has);
        assert_eq!(message(5), None);
        // 20 больше 8, а у порога 15 строки нет.
        assert_eq!(
            message(20).as_deref(),
            Some("health-examinable-carbon-Blunt-8")
        );
        assert_eq!(
            message(40).as_deref(),
            Some("health-examinable-carbon-Blunt-30")
        );
        // Ровно порог ещё не пройден.
        assert_eq!(
            message(30).as_deref(),
            Some("health-examinable-carbon-Blunt-8")
        );
    }
}
