//! Окно медсканера на клиенте: имя и вид пациента, режим сканирования (зелёный — активно,
//! красный — нет), состояние, температура, уровень крови, общий урон, предупреждение о
//! кровотечении и урон по группам (по убыванию) с типами. Урон и состояние моба берутся с
//! сущности пациента. Образец — `HealthAnalyzerControl`, `HealthAnalyzerWindow`,
//! `HealthAnalyzerBoundUserInterface` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока картинки пациента (вида сущности в окне).

use astrocore_client::{GameResources, Textures, UiStyle, WindowKinds, spawn_label};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_net::NetEntities;
use astrocore_prototypes::Prototypes;
use astrocore_ui::OpenUi;
use bevy::prelude::*;
use bevy_ecs::world::World as GameWorld;

use crate::body::HumanoidProfile;
use crate::damage::{Damageable, catalog_of};
use crate::examine::{loc, strip_markup};
use crate::fixed_point::FixedPoint2;
use crate::health_analyzer::HealthAnalyzerUiState;
use crate::mobs::{MobState, MobStateComponent};

const KIND: &str = "HealthAnalyzerBoundUserInterface";
/// Значки групп урона: состояние — идентификатор группы строчными буквами.
const GROUP_ICONS: &str = "Textures/Objects/Devices/health_analyzer.rsi";
const ICON: f32 = 30.0;

/// Значок группы урона: система ниже ставит картинку, когда текстура загружена.
#[derive(Component)]
struct GroupIcon(String);
const GREEN: Color = Color::srgb(0.2, 0.8, 0.2);
const RED: Color = Color::srgb(0.9, 0.2, 0.2);

/// Ноль по Цельсию, К (`Atmospherics.T0C`).
const T0C: f32 = 273.15;

/// Температура строкой «°C (K)».
pub fn temperature_text(kelvin: f32) -> String {
    format!("{:.1} °C ({kelvin:.1} K)", kelvin - T0C)
}

/// Уровень крови строкой в процентах.
pub fn blood_text(level: f32) -> String {
    format!("{:.1} %", level * 100.0)
}

fn status_key(state: MobState) -> &'static str {
    match state {
        MobState::Alive => "health-analyzer-window-entity-alive-text",
        MobState::Critical => "health-analyzer-window-entity-critical-text",
        MobState::Dead => "health-analyzer-window-entity-dead-text",
        MobState::Invalid => "health-analyzer-window-entity-unknown-text",
    }
}

/// Название прототипа по полю `name` (виды, группы и типы урона).
fn proto_name(world: &GameWorld, kind: &str, id: &str) -> String {
    let key = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node(kind, id))
        .and_then(|node| node.get("name"))
        .and_then(|name| astrocore_data::from_node::<String>(name).ok());
    match key {
        Some(key) => loc(world, &key, &[]),
        None => id.to_owned(),
    }
}

fn colored(commands: &mut Commands, parent: Entity, style: &UiStyle, text: &str, color: Color) {
    commands
        .spawn((ChildOf(parent), style.text(text, 14.0)))
        .insert(TextColor(color));
}

fn field(commands: &mut Commands, parent: Entity, style: &UiStyle, name: &str, value: &str) {
    let row = commands
        .spawn((
            ChildOf(parent),
            Node {
                column_gap: Val::Px(8.0),
                ..default()
            },
        ))
        .id();
    spawn_label(commands, row, style, name);
    spawn_label(commands, row, style, value);
}

fn window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let state = ui.state::<HealthAnalyzerUiState>().unwrap_or_default();
    let target = state
        .target_entity
        .and_then(|net| world.get_resource::<NetEntities>()?.entity(net))
        .filter(|entity| world.get_entity(*entity).is_ok());
    let Some(target) = target else {
        let text = if state.target_entity.is_some() {
            loc(world, "health-analyzer-window-scan-mode-inactive", &[])
        } else {
            loc(world, "health-analyzer-window-no-patient-data-text", &[])
        };
        spawn_label(commands, content, style, &text);
        return;
    };
    let unknown = loc(world, "health-analyzer-window-entity-unknown-text", &[]);
    let header = commands
        .spawn((
            ChildOf(content),
            Node {
                column_gap: Val::Px(16.0),
                min_width: Val::Px(320.0),
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
        ))
        .id();
    let names = commands
        .spawn((
            ChildOf(header),
            Node {
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .id();
    spawn_label(commands, names, style, &world.entity_name(target));
    let species = match world.get::<HumanoidProfile>(target) {
        Some(profile) => proto_name(world, "species", &profile.species),
        None => loc(
            world,
            "health-analyzer-window-entity-unknown-species-text",
            &[],
        ),
    };
    spawn_label(commands, names, style, &species);
    let (mode, color) = if state.scan_mode {
        ("health-analyzer-window-scan-mode-active", GREEN)
    } else {
        ("health-analyzer-window-scan-mode-inactive", RED)
    };
    colored(commands, header, style, &loc(world, mode, &[]), color);

    let status = world
        .get::<MobStateComponent>(target)
        .map_or(unknown.clone(), |m| {
            loc(world, status_key(m.current_state), &[])
        });
    field(
        commands,
        content,
        style,
        &loc(world, "health-analyzer-window-entity-status-text", &[]),
        &status,
    );
    let temperature = state.temperature.map_or_else(
        || {
            loc(
                world,
                "health-analyzer-window-entity-unknown-temperature-text",
                &[],
            )
        },
        temperature_text,
    );
    field(
        commands,
        content,
        style,
        &loc(world, "health-analyzer-window-entity-temperature-text", &[]),
        &temperature,
    );
    let blood = state.blood_level.map_or_else(
        || {
            loc(
                world,
                "health-analyzer-window-entity-unknown-value-text",
                &[],
            )
        },
        blood_text,
    );
    field(
        commands,
        content,
        style,
        &loc(world, "health-analyzer-window-entity-blood-level-text", &[]),
        &blood,
    );
    let Some(damageable) = world.get::<Damageable>(target) else {
        return;
    };
    field(
        commands,
        content,
        style,
        &loc(
            world,
            "health-analyzer-window-entity-damage-total-text",
            &[],
        ),
        &damageable.damage.total().to_string(),
    );
    if state.unrevivable {
        let text = loc(world, "health-analyzer-window-entity-unrevivable-text", &[]);
        colored(
            commands,
            content,
            style,
            &strip_markup(&text),
            Color::srgb(1.0, 1.0, 0.0),
        );
    }
    if state.bleeding {
        let text = loc(world, "health-analyzer-window-entity-bleeding-text", &[]);
        colored(commands, content, style, &strip_markup(&text), RED);
    }
    // Группы по убыванию урона, под каждой — её типы.
    let mut groups: Vec<(String, FixedPoint2)> = damageable
        .damage_per_group
        .iter()
        .filter(|(_, amount)| **amount != FixedPoint2::ZERO)
        .map(|(id, amount)| (id.clone(), *amount))
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.1));
    let catalog = catalog_of(world);
    for (group, amount) in groups {
        let title = loc(
            world,
            "health-analyzer-window-damage-group-text",
            &[
                (
                    "damageGroup",
                    LocArg::from(proto_name(world, "damageGroup", &group)),
                ),
                ("amount", LocArg::from(amount.to_string())),
            ],
        );
        let title_row = commands
            .spawn((
                ChildOf(content),
                Node {
                    column_gap: Val::Px(4.0),
                    align_items: AlignItems::Center,
                    margin: UiRect::top(Val::Px(6.0)),
                    ..default()
                },
            ))
            .id();
        commands.spawn((
            ChildOf(title_row),
            Node {
                width: Val::Px(ICON),
                height: Val::Px(ICON),
                ..default()
            },
            GroupIcon(group.to_lowercase()),
        ));
        spawn_label(commands, title_row, style, &title);
        let types = catalog
            .as_ref()
            .and_then(|c| c.group(&group).map(<[String]>::to_vec))
            .unwrap_or_default();
        for damage_type in types {
            let Some(value) = damageable.damage.types.get(&damage_type).copied() else {
                continue;
            };
            if value <= FixedPoint2::ZERO {
                continue;
            }
            let text = loc(
                world,
                "health-analyzer-window-damage-type-text",
                &[
                    (
                        "damageType",
                        LocArg::from(proto_name(world, "damageType", &damage_type)),
                    ),
                    ("amount", LocArg::from(value.to_string())),
                ],
            );
            spawn_label(commands, content, style, &format!(" · {text}"));
        }
    }
}

/// Картинки значков групп; нет такого состояния — `unknown` (`GetTexture`).
fn load_group_icons(
    mut commands: Commands,
    icons: Query<(Entity, &GroupIcon), Without<ImageNode>>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
) {
    let Some(resources) = resources else {
        return;
    };
    for (entity, icon) in &icons {
        let texture = textures
            .state(
                &resources.0,
                &mut images,
                &mut layouts,
                GROUP_ICONS,
                &icon.0,
            )
            .or_else(|| {
                textures.state(
                    &resources.0,
                    &mut images,
                    &mut layouts,
                    GROUP_ICONS,
                    "unknown",
                )
            });
        let Some(texture) = texture else {
            commands.entity(entity).remove::<GroupIcon>();
            continue;
        };
        let index = texture.state.sheet_index(0, 0);
        commands.entity(entity).insert(ImageNode::from_atlas_image(
            texture.image.clone(),
            TextureAtlas {
                layout: texture.layout.clone(),
                index,
            },
        ));
    }
}

/// Окно медсканера.
pub struct HealthAnalyzerUiPlugin;

impl Plugin for HealthAnalyzerUiPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<WindowKinds>()
            .register(KIND, window);
        // После перерисовки окон в `Update`: значки видны в том же кадре.
        app.add_systems(PostUpdate, load_group_icons);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_formatted_like_ss14() {
        assert_eq!(temperature_text(310.15), "37.0 °C (310.1 K)");
        assert_eq!(blood_text(0.875), "87.5 %");
    }
}
