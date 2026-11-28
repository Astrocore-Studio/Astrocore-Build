//! Этикетки: текст на предмете (банки, бутылки, таблетки). Этикетка из прототипа задаётся
//! ключом локализации и переводится при появлении; пустой текст снимает этикетку; при
//! осмотре этикетка читается (скобки разметки из текста убираются — экранирования
//! разметки у нас пока нет). Образец — `LabelSystem`, `LabelComponent` SS14 @ 2f4a2aad.
//!
//! Нет пока: приписки этикетки к имени (`NameModifier`), ручного этикетировщика
//! (`HandLabeler`), бумажных этикеток (`PaperLabel`).

use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::{Examined, loc};

fn yes() -> bool {
    true
}

/// Этикетка (`LabelComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    #[serde(default)]
    pub current_label: Option<String>,
    #[serde(default)]
    pub localized_label: Option<String>,
    #[serde(default = "yes")]
    pub examinable: bool,
}

/// Наклеить этикетку; пустой текст — снять (`Label`).
pub fn label(world: &mut World, entity: Entity, text: Option<&str>) {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        remove_label(world, entity);
        return;
    };
    match world.get_mut::<Label>(entity) {
        Some(mut component) => component.current_label = Some(text.to_owned()),
        None => {
            world.entity_mut(entity).insert(Label {
                current_label: Some(text.to_owned()),
                localized_label: None,
                examinable: true,
            });
        }
    }
}

/// Снять этикетку (`RemoveLabel`); `true` — была.
pub fn remove_label(world: &mut World, entity: Entity) -> bool {
    world.entity_mut(entity).take::<Label>().is_some()
}

/// Текст этикетки (`GetLabelText`).
pub fn label_text(world: &World, entity: Entity) -> Option<String> {
    world.get::<Label>(entity)?.current_label.clone()
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Label>();
    world.register_net_serde::<Label>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Label, MapInit>(|world, entity, _| {
        let Some(key) = world
            .get::<Label>(entity)
            .and_then(|l| l.localized_label.clone())
        else {
            return;
        };
        let text = loc(world, &key, &[]);
        if let Some(mut component) = world.get_mut::<Label>(entity) {
            component.current_label = Some(text);
        }
    });
    world.subscribe::<Label, Examined>(|world, entity, event| {
        let Some(component) = world.get::<Label>(entity).cloned() else {
            return;
        };
        let Some(text) = component.current_label.filter(|_| component.examinable) else {
            return;
        };
        // Разметка в тексте игрока не действует: в SS14 он экранируется, у нас скобки
        // разметки убираются.
        let plain = text.replace(['[', ']'], "");
        let line = loc(world, "hand-labeler-has-label", &[("label", plain.into())]);
        event.push_markup(line, 0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{spawn_at, station, world_with};

    #[test]
    fn localized_label_is_set_on_spawn_and_can_be_changed() {
        let mut world = world_with(
            "- type: entity\n  id: Jug\n  components:\n  - type: Label\n    localizedLabel: reagent-name-aluminium\n",
        );
        let grid = station(&mut world);
        let jug = spawn_at(&mut world, "Jug", grid, 1, 1);
        assert_eq!(
            label_text(&world, jug).as_deref(),
            Some("reagent-name-aluminium")
        );
        label(&mut world, jug, Some("вода"));
        assert_eq!(label_text(&world, jug).as_deref(), Some("вода"));
        label(&mut world, jug, Some(""));
        assert_eq!(label_text(&world, jug), None);
        assert!(!remove_label(&mut world, jug));
    }
}
