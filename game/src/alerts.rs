//! Значки тревог у сущности игрока: прототипы `alert` (значки по уровням, название, описание,
//! категория — в категории виден один значок), порядок на панели (`alertOrder`), показ и
//! снятие с уровнем и отсчётом, самоснятие по истечении отсчёта, нажатие на значок — событие
//! на сущности игрока (`clickEvent`); состояние уходит только самому игроку. Образец —
//! `AlertsSystem`, `AlertsComponent`, `AlertPrototype`, `AlertOrderPrototype`, `AlertKey`,
//! `AlertState`, `ClickAlertEvent`, `AlertAutoRemoveComponent` SS14 @ 2f4a2aad.
//!
//! Значки, которые ставит сам клиент (`clientHandled`: «шаг» при переключаемой ходьбе), —
//! вместе с переключаемой ходьбой (у нас ходьба удерживается).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Duration;

use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_net::{
    NetComponent, NetError, NetEvent, NetEventWorldExt, NetRegistryWorldExt, PlayerAttached,
    Session, WriteContext,
};
use astrocore_prototypes::Prototypes;
use bevy_ecs::change_detection::DetectChangesMut;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Значок тревоги: картинка или состояние RSI (`SpriteSpecifier`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertIcon {
    Texture(String),
    Rsi { sprite: String, state: String },
}

/// Прототип тревоги (`AlertPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct AlertPrototype {
    pub id: String,
    pub icons: Vec<AlertIcon>,
    pub name: String,
    pub description: String,
    pub category: Option<String>,
    min_severity: i16,
    pub max_severity: i16,
    pub client_handled: bool,
    /// Событие нажатия (тег `!type:` у `clickEvent`).
    pub click_event: Option<String>,
}

impl AlertPrototype {
    /// Из данных прототипа.
    pub fn from_node(id: &str, node: &DataNode) -> Self {
        let text = |key: &str| node.get(key).and_then(DataNode::as_str).map(str::to_owned);
        let number = |key: &str| text(key).and_then(|t| t.trim().parse::<i16>().ok());
        let icons = node
            .get("icons")
            .and_then(DataNode::as_seq)
            .unwrap_or_default()
            .iter()
            .filter_map(|icon| match icon.as_str() {
                Some(path) => Some(AlertIcon::Texture(path.to_owned())),
                None => Some(AlertIcon::Rsi {
                    sprite: icon.get("sprite")?.as_str()?.to_owned(),
                    state: icon.get("state")?.as_str()?.to_owned(),
                }),
            })
            .collect();
        Self {
            id: id.to_owned(),
            icons,
            name: text("name").unwrap_or_default(),
            description: text("description").unwrap_or_default(),
            category: text("category"),
            min_severity: number("minSeverity").unwrap_or(1),
            max_severity: number("maxSeverity").unwrap_or(-1),
            client_handled: text("clientHandled").is_some_and(|t| t == "true"),
            click_event: node
                .get("clickEvent")
                .and_then(DataNode::type_tag)
                .map(str::to_owned),
        }
    }

    /// Есть ли уровни (`SupportsSeverity`).
    pub fn supports_severity(&self) -> bool {
        self.max_severity != -1
    }

    /// Наименьший уровень (`MinSeverity`).
    pub fn min_severity(&self) -> i16 {
        if self.supports_severity() {
            self.min_severity
        } else {
            -1
        }
    }

    /// Ключ на панели (`AlertKey`): категория, если есть, иначе сам вид.
    pub fn key(&self) -> String {
        alert_key(&self.id, self.category.as_deref())
    }

    /// Значок уровня (`GetIcon`); уровень вне пределов — ближайший.
    pub fn icon(&self, severity: Option<i16>) -> Option<&AlertIcon> {
        if !self.supports_severity() {
            return self.icons.first();
        }
        let severity = severity
            .unwrap_or(self.min_severity)
            .clamp(self.min_severity, self.max_severity);
        self.icons
            .get((severity - self.min_severity) as usize)
            .or_else(|| self.icons.last())
    }
}

/// Ключ на панели: в категории — один значок.
pub fn alert_key(alert_type: &str, category: Option<&str>) -> String {
    match category {
        Some(category) => format!("category:{category}"),
        None => format!("type:{alert_type}"),
    }
}

/// Прототип тревоги по виду.
pub fn alert_prototype(prototypes: &Prototypes, id: &str) -> Option<AlertPrototype> {
    prototypes
        .resolved_node("alert", id)
        .map(|node| AlertPrototype::from_node(id, node))
}

/// Порядок значков (`AlertOrderPrototype`): виды и категории по списку, остальные — после,
/// по имени.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertOrder {
    types: BTreeMap<String, usize>,
    categories: BTreeMap<String, usize>,
}

impl AlertOrder {
    pub fn from_prototypes(prototypes: &Prototypes) -> Self {
        let mut order = Self::default();
        let Some(id) = prototypes.ids("alertOrder").next().map(str::to_owned) else {
            return order;
        };
        let entries = prototypes
            .resolved_node("alertOrder", &id)
            .and_then(|node| node.get("order"))
            .and_then(DataNode::as_seq)
            .unwrap_or_default();
        for (index, entry) in entries.iter().enumerate() {
            if let Some(alert) = entry.get("alertType").and_then(DataNode::as_str) {
                order.types.insert(alert.to_owned(), index);
            } else if let Some(category) = entry.get("category").and_then(DataNode::as_str) {
                order.categories.insert(category.to_owned(), index);
            }
        }
        order
    }

    fn index(&self, alert: &AlertPrototype) -> Option<usize> {
        self.types.get(&alert.id).copied().or_else(|| {
            alert
                .category
                .as_ref()
                .and_then(|c| self.categories.get(c).copied())
        })
    }

    /// Сравнение для сортировки панели (`Compare`).
    pub fn compare(&self, a: &AlertPrototype, b: &AlertPrototype) -> Ordering {
        match (self.index(a), self.index(b)) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        }
    }
}

/// Показанный значок (`AlertState`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertState {
    pub alert_type: String,
    pub severity: Option<i16>,
    /// Отсчёт: начало и конец по часам игры.
    pub cooldown: Option<(Duration, Duration)>,
    pub auto_remove: bool,
    pub show_cooldown: bool,
}

/// Значки тревог сущности (`AlertsComponent`). Только владельцу.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Alerts {
    pub alerts: BTreeMap<String, AlertState>,
}

impl NetComponent for Alerts {
    const NAME: &'static str = "Alerts";

    fn write(&self, _: &World, entity: Entity, ctx: &WriteContext, out: &mut Vec<u8>) {
        // Чужие значки не уходят (`SendOnlyToOwner`).
        let owner = ctx.attached == Some(entity);
        let empty = BTreeMap::new();
        let alerts = if owner { &self.alerts } else { &empty };
        if let Ok(bytes) = postcard::to_extend(alerts, std::mem::take(out)) {
            *out = bytes;
        }
    }

    fn apply(world: &mut World, entity: Entity, bytes: &[u8]) -> Result<(), NetError> {
        let alerts: BTreeMap<String, AlertState> = postcard::from_bytes(bytes)?;
        world.entity_mut(entity).insert(Alerts { alerts });
        Ok(())
    }
}

impl<'de> Deserialize<'de> for Alerts {
    /// В прототипе у компонента нет полей.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(Self::default())
    }
}

impl Serialize for Alerts {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        serializer.serialize_map(Some(0))?.end()
    }
}

/// Нажатие на значок (`ClickAlertEvent`). Клиент → сервер.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClickAlert {
    pub alert_type: String,
}

/// Значок нажат (событие `clickEvent` прототипа). На сущности игрока; `handled` — нажатие
/// обработано.
#[derive(Debug, Clone)]
pub struct AlertClicked {
    pub user: Entity,
    pub alert_type: String,
    /// Имя события (`ResistFireAlertEvent`, `StopPullingAlertEvent`…).
    pub event: String,
    pub handled: bool,
}

/// Разобранные прототипы тревог: значки ставятся часто, разбор — один раз.
#[derive(bevy_ecs::resource::Resource, Default)]
pub struct AlertCache(std::sync::RwLock<std::collections::HashMap<String, Option<AlertPrototype>>>);

fn prototype(world: &World, alert_type: &str) -> Option<AlertPrototype> {
    let cache = world.get_resource::<AlertCache>();
    if let Some(found) = cache
        .and_then(|c| c.0.read().ok())
        .and_then(|map| map.get(alert_type).cloned())
    {
        return found;
    }
    let parsed = alert_prototype(world.get_resource::<Prototypes>()?, alert_type);
    if let Some(mut map) = cache.and_then(|c| c.0.write().ok()) {
        map.insert(alert_type.to_owned(), parsed.clone());
    }
    parsed
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Значки сущности (`GetActiveAlerts`).
pub fn active_alerts(world: &World, entity: Entity) -> Option<&BTreeMap<String, AlertState>> {
    world.get::<Alerts>(entity).map(|a| &a.alerts)
}

/// Показан ли значок вида (`IsShowingAlert`).
pub fn is_showing_alert(world: &World, entity: Entity, alert_type: &str) -> bool {
    let Some(alert) = prototype(world, alert_type) else {
        return false;
    };
    world
        .get::<Alerts>(entity)
        .is_some_and(|a| a.alerts.contains_key(&alert.key()))
}

/// Показан ли значок категории (`IsShowingAlertCategory`).
pub fn is_showing_alert_category(world: &World, entity: Entity, category: &str) -> bool {
    world
        .get::<Alerts>(entity)
        .is_some_and(|a| a.alerts.contains_key(&alert_key("", Some(category))))
}

/// Показать значок (`ShowAlert`): в категории заменяет прежний.
pub fn show_alert(world: &mut World, entity: Entity, alert_type: &str, severity: Option<i16>) {
    show_alert_state(
        world,
        entity,
        AlertState {
            alert_type: alert_type.to_owned(),
            severity,
            cooldown: None,
            auto_remove: false,
            show_cooldown: true,
        },
    );
}

/// Показать значок с полным состоянием (`ShowAlert(AlertState)`).
pub fn show_alert_state(world: &mut World, entity: Entity, state: AlertState) {
    if world.get::<Alerts>(entity).is_none() {
        return;
    }
    let Some(alert) = prototype(world, &state.alert_type) else {
        eprintln!("нет прототипа тревоги {}", state.alert_type);
        return;
    };
    let key = alert.key();
    if world
        .get::<Alerts>(entity)
        .is_some_and(|a| a.alerts.get(&key) == Some(&state))
    {
        return;
    }
    if let Some(mut alerts) = world.get_mut::<Alerts>(entity) {
        alerts.alerts.insert(key, state);
    }
}

/// Показать значок с отсчётом, сохраняя начало, если отсчёт не растёт (`UpdateAlert`).
pub fn update_alert(
    world: &mut World,
    entity: Entity,
    alert_type: &str,
    severity: Option<i16>,
    cooldown: Option<Duration>,
    auto_remove: bool,
) {
    let Some(end) = cooldown else {
        show_alert(world, entity, alert_type, severity);
        return;
    };
    let Some(alert) = prototype(world, alert_type) else {
        return;
    };
    let previous = world
        .get::<Alerts>(entity)
        .and_then(|a| a.alerts.get(&alert.key()))
        .and_then(|s| s.cooldown);
    let time = now(world);
    let down = match previous {
        Some((start, old_end)) if old_end >= end => (start, end),
        _ => (time, end),
    };
    show_alert_state(
        world,
        entity,
        AlertState {
            alert_type: alert_type.to_owned(),
            severity,
            cooldown: Some(down),
            auto_remove,
            show_cooldown: true,
        },
    );
}

/// Снять значок вида (`ClearAlert`).
pub fn clear_alert(world: &mut World, entity: Entity, alert_type: &str) {
    let Some(alert) = prototype(world, alert_type) else {
        return;
    };
    let key = alert.key();
    if world
        .get::<Alerts>(entity)
        .is_some_and(|a| a.alerts.contains_key(&key))
        && let Some(mut alerts) = world.get_mut::<Alerts>(entity)
    {
        alerts.alerts.remove(&key);
    }
}

/// Снять значок категории (`ClearAlertCategory`).
pub fn clear_alert_category(world: &mut World, entity: Entity, category: &str) {
    let key = alert_key("", Some(category));
    if world
        .get::<Alerts>(entity)
        .is_some_and(|a| a.alerts.contains_key(&key))
        && let Some(mut alerts) = world.get_mut::<Alerts>(entity)
    {
        alerts.alerts.remove(&key);
    }
}

/// Нажать значок (`ActivateAlert`): событие на сущности; `true` — обработано.
pub fn activate_alert(world: &mut World, user: Entity, alert_type: &str) -> bool {
    let Some(event) = prototype(world, alert_type).and_then(|a| a.click_event) else {
        return false;
    };
    let mut clicked = AlertClicked {
        user,
        alert_type: alert_type.to_owned(),
        event,
        handled: false,
    };
    world.raise(user, &mut clicked);
    clicked.handled
}

fn receive_click(world: &mut World, event: &mut NetEvent<ClickAlert>) {
    let Some(player) = event
        .session
        .and_then(|s| world.get::<Session>(s))
        .and_then(|s| s.attached)
    else {
        return;
    };
    if world.get::<Alerts>(player).is_none() {
        return;
    }
    let alert_type = event.event.alert_type.clone();
    if !is_showing_alert(world, player, &alert_type) {
        return;
    }
    activate_alert(world, player, &alert_type);
}

/// Такт: значки с истёкшим отсчётом и `auto_remove` снимаются.
fn update(world: &mut World) {
    let time = now(world);
    let expired: Vec<(Entity, String)> = world
        .query::<(Entity, &Alerts)>()
        .iter(world)
        .flat_map(|(entity, alerts)| {
            alerts
                .alerts
                .iter()
                .filter(|(_, state)| {
                    state.auto_remove && state.cooldown.is_some_and(|(_, end)| end < time)
                })
                .map(move |(key, _)| (entity, key.clone()))
        })
        .collect();
    for (entity, key) in expired {
        if let Some(mut alerts) = world.get_mut::<Alerts>(entity) {
            alerts.alerts.remove(&key);
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Alerts>();
    world.register_net_component::<Alerts>();
    world.register_net_event::<ClickAlert>();
    world.init_resource::<AlertCache>();
    if !side.is_server() {
        return;
    }
    world.subscribe_net_event::<ClickAlert>(receive_click);
    // Новому владельцу значки уходят заново (`OnPlayerAttached`).
    world.subscribe::<Alerts, PlayerAttached>(|world, entity, _| {
        if let Some(mut alerts) = world.get_mut::<Alerts>(entity) {
            alerts.set_changed();
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
