//! Осмотр: текст о сущности — описание и то, что добавят системы. Образец —
//! `Content.Shared/Examine/ExamineSystemShared.cs` SS14 @ 2f4a2aad и серверный ответ
//! `ExamineInfoResponseMessage`.
//!
//! Клиент просит осмотр цели; сервер проверяет, видит ли игрок цель (не дальше 16 тайлов и
//! без стен между ними), и присылает текст.

use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_input::{BoundKeyState, InputWorldExt};
use astrocore_light::{OccluderGrid, collect_occluders};
use astrocore_loc::{EntityNameWorldExt, Loc, LocArg};
use astrocore_map::{ContainerWorldExt, MapCoordinates, Transform, TransformWorldExt};
use astrocore_net::{
    NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetRegistryWorldExt, NetTarget, Session,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::hands::{Hands, HandsWorldExt};
use crate::interaction::InteractionWorldExt;
use crate::item::Item;

/// Дальность осмотра (`ExamineRange`).
pub const EXAMINE_RANGE: f32 = 16.0;
/// Дальность подробностей (`ExamineDetailsRange`).
pub const EXAMINE_DETAILS_RANGE: f32 = 3.0;

/// Функция ввода «осмотреть» (Shift+клик).
pub const EXAMINE_ENTITY: &str = "ExamineEntity";

/// Сущность умеет осматривать. В данных — компонент `Examiner`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Examiner {
    /// Видит всё (призраки).
    #[serde(default)]
    pub skip_checks: bool,
    #[serde(default = "yes", rename = "checkInRangeUnOccluded")]
    pub check_in_range_unoccluded: bool,
}

fn yes() -> bool {
    true
}

/// Сущность осматривают (`ExaminedEvent`): системы добавляют строки. Поднимается на цели.
#[derive(Debug, Clone)]
pub struct Examined {
    pub examiner: Entity,
    pub examined: Entity,
    /// Осматривающий рядом: можно рассказать подробности.
    pub is_in_details_range: bool,
    parts: Vec<(i32, String)>,
}

impl Examined {
    /// Строка в разметке SS14 (`[color=…]`, `[bold]`). Больше приоритет — выше.
    pub fn push_markup(&mut self, markup: impl Into<String>, priority: i32) {
        self.parts.push((priority, markup.into()));
    }

    /// Весь текст (`GetTotalMessage`): строки по убыванию приоритета.
    pub fn total(&self) -> Vec<String> {
        let mut parts = self.parts.clone();
        parts.sort_by_key(|part| std::cmp::Reverse(part.0));
        parts.into_iter().map(|(_, text)| text).collect()
    }
}

/// Сообщения осмотра.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExamineMessage {
    /// Клиент: осмотреть (`RequestExamineInfoMessage`).
    Request(NetEntity),
    /// Сервер: название и строки текста (`ExamineInfoResponseMessage`).
    Response {
        target: NetEntity,
        name: String,
        lines: Vec<String>,
    },
}

/// Последний ответ осмотра: окно показывает его. Ресурс `World` клиента.
#[derive(Resource, Debug, Default)]
pub struct ExamineResult {
    pub target: Option<Entity>,
    pub name: String,
    pub lines: Vec<String>,
    pub revision: u64,
}

/// Убирает разметку SS14 (`[color=red]…[/color]`) для простого текста.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Между точками нет непрозрачных преград (`InRangeUnOccluded`).
fn in_range_unoccluded(
    world: &World,
    from: MapCoordinates,
    to: MapCoordinates,
    range: f32,
    grid: Option<Entity>,
) -> bool {
    if from.map != to.map || from.z != to.z {
        return false;
    }
    let length = from.position.distance(to.position);
    if range > 0.0 && length > range + 0.01 {
        return false;
    }
    if length < 1e-4 {
        return true;
    }
    let Some(grid) = grid else {
        return true;
    };
    let Some(placement) = world.world_transform(grid) else {
        return true;
    };
    let (a, b) = (
        placement.to_local(from.position),
        placement.to_local(to.position),
    );
    let low = a.min(b).floor();
    let high = a.max(b).ceil();
    let occluders: OccluderGrid = collect_occluders(
        world,
        grid,
        from.z - placement.level,
        (low.x as i32 - 1, low.y as i32 - 1),
        ((high.x - low.x) as i32 + 3, (high.y - low.y) as i32 + 3),
    );
    occluders.visible(a, b)
}

/// Может ли сущность осмотреть цель (`CanExamine`).
pub fn can_examine(world: &World, examiner: Entity, examined: Entity) -> bool {
    if !world.is_alive(examined) {
        return false;
    }
    let Some(own) = world.get::<Examiner>(examiner).copied() else {
        return false;
    };
    if own.skip_checks {
        return true;
    }
    // Из закрытого контейнера ничего не видно, кроме того, что рядом в нём же.
    if let Some(container) = world.containing_container(examiner)
        && world
            .containing_container(examined)
            .is_none_or(|other| other.owner != container.owner)
        && container.owner != examined
    {
        return false;
    }
    if !own.check_in_range_unoccluded {
        return true;
    }
    let (Some(from), Some(to)) = (
        world.map_coordinates(examiner),
        world.map_coordinates(examined),
    ) else {
        return false;
    };
    let grid = world.get::<Transform>(examiner).and_then(Transform::grid);
    in_range_unoccluded(world, from, to, EXAMINE_RANGE, grid)
}

/// Рядом для подробностей (`IsInDetailsRange`).
pub fn is_in_details_range(world: &mut World, examiner: Entity, examined: Entity) -> bool {
    if world
        .get::<Examiner>(examiner)
        .is_some_and(|e| e.skip_checks)
    {
        return true;
    }
    let (Some(from), Some(to)) = (
        world.map_coordinates(examiner),
        world.map_coordinates(examined),
    ) else {
        return false;
    };
    let grid = world.get::<Transform>(examiner).and_then(Transform::grid);
    in_range_unoccluded(world, from, to, EXAMINE_DETAILS_RANGE, grid)
        && world.is_accessible(examiner, examined)
}

/// Текст осмотра (`GetExamineText`).
pub fn examine_text(world: &mut World, examiner: Entity, examined: Entity) -> Vec<String> {
    let mut event = Examined {
        examiner,
        examined,
        is_in_details_range: is_in_details_range(world, examiner, examined),
        parts: Vec::new(),
    };
    if let Some(description) = world.entity_description(examined).filter(|d| !d.is_empty()) {
        event.push_markup(description, i32::MAX);
    }
    world.raise(examined, &mut event);
    event.total()
}

pub(crate) fn loc(world: &World, id: &str, args: &[(&str, LocArg)]) -> String {
    world
        .get_resource::<Loc>()
        .map_or_else(|| id.to_owned(), |loc| loc.format(id, args))
}

fn receive(world: &mut World, event: &mut NetEvent<ExamineMessage>) {
    match (&event.event, event.session) {
        (ExamineMessage::Request(net), Some(session)) => {
            let Some(user) = world.get::<Session>(session).and_then(|s| s.attached) else {
                return;
            };
            let Some(target) = world
                .get_resource::<NetEntities>()
                .and_then(|all| all.entity(*net))
            else {
                return;
            };
            let lines = if can_examine(world, user, target) {
                examine_text(world, user, target)
            } else {
                vec![loc(world, "examine-system-cant-see-entity", &[])]
            };
            let response = ExamineMessage::Response {
                target: *net,
                name: world.entity_name(target),
                lines,
            };
            world.send_net_event(NetTarget::Session(session), &response);
        }
        (
            ExamineMessage::Response {
                target,
                name,
                lines,
            },
            None,
        ) => {
            let target = world
                .get_resource::<NetEntities>()
                .and_then(|all| all.entity(*target));
            let mut result = world.get_resource_or_insert_with(ExamineResult::default);
            result.target = target;
            result.name = name.clone();
            result.lines = lines.clone();
            result.revision += 1;
        }
        _ => {}
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Examiner>();
    world.register_net_serde::<Examiner>();
    world.register_net_event::<ExamineMessage>();
    world.subscribe_net_event::<ExamineMessage>(receive);
    world.init_resource::<ExamineResult>();
    if !side.is_server() {
        // Shift+клик на клиенте просит осмотр сущности под курсором.
        world.bind_input(EXAMINE_ENTITY, |world, args| {
            if args.state == BoundKeyState::Down
                && let Some(target) = args.target
                && let Some(net) = world.get::<NetEntity>(target).copied()
            {
                world.send_net_event_to_server(&ExamineMessage::Request(net));
                return true;
            }
            false
        });
        return;
    }
    // Размер предмета (`SharedItemSystem.OnExamine`).
    world.subscribe::<Item, Examined>(|world, item, event| {
        let Some(size) = world.get::<Item>(item).map(|item| item.size.clone()) else {
            return;
        };
        let size_name = world
            .get_resource::<astrocore_prototypes::Prototypes>()
            .and_then(|p| p.resolved_node("itemSize", &size))
            .and_then(|node| node.get("name"))
            .and_then(|name| name.as_str())
            .map(|key| loc(world, key, &[]))
            .unwrap_or(size);
        let text = loc(
            world,
            "item-component-on-examine-size",
            &[("size", size_name.as_str().into())],
        );
        event.push_markup(text, -2);
    });
    // Что в руках (`SharedHandsSystem.HandleExamined`).
    world.subscribe::<Hands, Examined>(|world, user, event| {
        let held: Vec<String> = world
            .held_items(user)
            .into_iter()
            .map(|item| {
                let name = world.entity_name(item);
                loc(
                    world,
                    "comp-hands-examine-wrapper",
                    &[("item", name.as_str().into())],
                )
            })
            .collect();
        let name = world.entity_name(user);
        let text = if held.is_empty() {
            loc(
                world,
                "comp-hands-examine-empty",
                &[("user", name.as_str().into())],
            )
        } else {
            let items = held.join(", ");
            loc(
                world,
                "comp-hands-examine",
                &[
                    ("user", name.as_str().into()),
                    ("items", items.as_str().into()),
                ],
            )
        };
        event.push_markup(text, 0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{spawn_at, station, world_with};

    const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: Examiner
- type: entity
  id: Wall
  components:
  - type: Transform
    anchored: true
  - type: Occluder
- type: entity
  id: Crowbar
  description: A steel crowbar.
  components:
  - type: Item
"#;

    #[test]
    fn walls_hide_and_descriptions_come_first() {
        let mut world = world_with(PROTOTYPES);
        let grid = station(&mut world);
        let user = spawn_at(&mut world, "Human", grid, 2, 2);
        let near = spawn_at(&mut world, "Crowbar", grid, 3, 2);
        let behind = spawn_at(&mut world, "Crowbar", grid, 2, 6);
        spawn_at(&mut world, "Wall", grid, 2, 4);
        assert!(can_examine(&world, user, near));
        assert!(!can_examine(&world, user, behind));
        // 12 тайлов — видно, 18 по диагонали — дальше 16.
        let far = spawn_at(&mut world, "Crowbar", grid, 14, 2);
        assert!(can_examine(&world, user, far));
        let too_far = spawn_at(&mut world, "Crowbar", grid, 15, 15);
        assert!(!can_examine(&world, user, too_far));
        // Без Examiner осматривать нельзя.
        assert!(!can_examine(&world, near, user));

        let lines = examine_text(&mut world, user, near);
        assert_eq!(lines[0], "A steel crowbar.");
        assert!(
            lines
                .last()
                .unwrap()
                .contains("item-component-on-examine-size")
                || lines.len() == 2
        );
        assert_eq!(
            strip_markup("This is a [bold]small[/bold] [color=red]item[/color]."),
            "This is a small item."
        );
    }
}
