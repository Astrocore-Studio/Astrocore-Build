//! Всплывающие сообщения: короткий текст над сущностью, у точки или у курсора игрока.
//! Образец — `Content.Shared/Popups/SharedPopupSystem.cs` и
//! `Content.Server/Popups/PopupSystem.cs` SS14 @ 2f4a2aad.
//!
//! Сервер отправляет сообщение тем, кто видит сущность (зона PVS), или одному игроку;
//! клиент складывает его в [`PopupQueue`], показывает слой окна.

use astrocore_console::ConfigWorldExt;
use astrocore_ecs::{GameTick, GameTime};
use astrocore_map::{MapCoordinates, MapId, TransformWorldExt};
use astrocore_math::Vec2;
use astrocore_net::{
    Actor, NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetTarget, PVS_RANGE, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// Размер и цвет сообщения (`PopupType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PopupType {
    #[default]
    Small,
    SmallCaution,
    Medium,
    MediumCaution,
    Large,
    LargeCaution,
}

impl PopupType {
    /// Предупреждающее сообщение рисуется красным.
    pub fn is_caution(self) -> bool {
        matches!(
            self,
            Self::SmallCaution | Self::MediumCaution | Self::LargeCaution
        )
    }
}

/// Где показать сообщение.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PopupPlace<E> {
    /// Над сущностью.
    Entity(E),
    /// В точке карты.
    Coordinates(MapCoordinates),
    /// У курсора получателя.
    Cursor,
}

/// Место сообщения в сети.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WirePlace {
    Entity(NetEntity),
    Coordinates { map: MapId, position: Vec2, z: i16 },
    Cursor,
}

/// Сообщение сервера клиенту (`PopupEntityEvent`, `PopupCoordinatesEvent`, `PopupCursorEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopupMessage {
    pub message: String,
    pub kind: PopupType,
    pub tick: GameTick,
    pub place: WirePlace,
}

/// Сообщение в очереди клиента.
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub message: String,
    pub kind: PopupType,
    pub place: PopupPlace<Entity>,
}

/// Сообщения, которые клиенту пора показать: слой окна забирает их каждый кадр.
/// Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct PopupQueue(pub Vec<Popup>);

/// Наименьшее и наибольшее время показа, секунды; время растёт с длиной текста
/// (`GetPopupLifetime` клиента SS14).
pub const MINIMUM_POPUP_LIFETIME: f32 = 0.7;
pub const MAXIMUM_POPUP_LIFETIME: f32 = 5.0;
pub const POPUP_LIFETIME_PER_CHARACTER: f32 = 0.04;

pub fn popup_lifetime(message: &str) -> f32 {
    (POPUP_LIFETIME_PER_CHARACTER * message.chars().count() as f32)
        .clamp(MINIMUM_POPUP_LIFETIME, MAXIMUM_POPUP_LIFETIME)
}

/// Сессии, в зону видимости которых попадает точка (`AddPlayersByPvs`).
fn sessions_seeing(world: &mut World, at: MapCoordinates) -> Vec<Entity> {
    let range = world.cvar(&PVS_RANGE);
    let sessions: Vec<(Entity, Entity)> = world
        .query::<(Entity, &Session)>()
        .iter(world)
        .filter_map(|(session, data)| Some((session, data.attached?)))
        .collect();
    sessions
        .into_iter()
        .filter(|(_, player)| {
            world.map_coordinates(*player).is_some_and(|own| {
                own.map == at.map
                    && own.z == at.z
                    && (own.position.x - at.position.x).abs() <= range
                    && (own.position.y - at.position.y).abs() <= range
            })
        })
        .map(|(session, _)| session)
        .collect()
}

fn send(world: &mut World, target: NetTarget, message: &str, kind: PopupType, place: WirePlace) {
    let tick = world.resource::<GameTime>().tick;
    let popup = PopupMessage {
        message: message.to_owned(),
        kind,
        tick,
        place,
    };
    world.send_net_event(target, &popup);
}

fn session_of(world: &World, player: Entity) -> Option<Entity> {
    world.get::<Actor>(player).map(|actor| actor.session)
}

/// Всплывающие сообщения на `World` (сервер).
pub trait PopupWorldExt {
    /// Над сущностью для всех, кто её видит.
    fn popup_entity(&mut self, message: &str, entity: Entity, kind: PopupType);

    /// Над сущностью для одного игрока (по его телу).
    fn popup_entity_for(
        &mut self,
        message: &str,
        entity: Entity,
        recipient: Entity,
        kind: PopupType,
    );

    /// Над сущностью: одно сообщение действующему, другое — остальным, кто видит.
    fn popup_entity_split(
        &mut self,
        recipient_message: &str,
        others_message: &str,
        entity: Entity,
        recipient: Entity,
        kind: PopupType,
    );

    /// У курсора игрока.
    fn popup_cursor(&mut self, message: &str, recipient: Entity, kind: PopupType);

    /// В точке карты для всех, кто её видит.
    fn popup_coordinates(&mut self, message: &str, at: MapCoordinates, kind: PopupType);
}

impl PopupWorldExt for World {
    fn popup_entity(&mut self, message: &str, entity: Entity, kind: PopupType) {
        if message.trim().is_empty() {
            return;
        }
        let (Some(net), Some(at)) = (
            self.get::<NetEntity>(entity).copied(),
            self.map_coordinates(entity),
        ) else {
            return;
        };
        for session in sessions_seeing(self, at) {
            send(
                self,
                NetTarget::Session(session),
                message,
                kind,
                WirePlace::Entity(net),
            );
        }
    }

    fn popup_entity_for(
        &mut self,
        message: &str,
        entity: Entity,
        recipient: Entity,
        kind: PopupType,
    ) {
        if message.trim().is_empty() {
            return;
        }
        let (Some(net), Some(session)) = (
            self.get::<NetEntity>(entity).copied(),
            session_of(self, recipient),
        ) else {
            return;
        };
        send(
            self,
            NetTarget::Session(session),
            message,
            kind,
            WirePlace::Entity(net),
        );
    }

    fn popup_entity_split(
        &mut self,
        recipient_message: &str,
        others_message: &str,
        entity: Entity,
        recipient: Entity,
        kind: PopupType,
    ) {
        let own = session_of(self, recipient);
        let (Some(net), Some(at)) = (
            self.get::<NetEntity>(entity).copied(),
            self.map_coordinates(entity),
        ) else {
            return;
        };
        for session in sessions_seeing(self, at) {
            let text = if Some(session) == own {
                recipient_message
            } else {
                others_message
            };
            if !text.trim().is_empty() {
                send(
                    self,
                    NetTarget::Session(session),
                    text,
                    kind,
                    WirePlace::Entity(net),
                );
            }
        }
    }

    fn popup_cursor(&mut self, message: &str, recipient: Entity, kind: PopupType) {
        if message.trim().is_empty() {
            return;
        }
        if let Some(session) = session_of(self, recipient) {
            send(
                self,
                NetTarget::Session(session),
                message,
                kind,
                WirePlace::Cursor,
            );
        }
    }

    fn popup_coordinates(&mut self, message: &str, at: MapCoordinates, kind: PopupType) {
        if message.trim().is_empty() {
            return;
        }
        for session in sessions_seeing(self, at) {
            send(
                self,
                NetTarget::Session(session),
                message,
                kind,
                WirePlace::Coordinates {
                    map: at.map,
                    position: at.position,
                    z: at.z,
                },
            );
        }
    }
}

fn receive(world: &mut World, event: &mut NetEvent<PopupMessage>) {
    if event.session.is_some() {
        return;
    }
    let place = match event.event.place {
        WirePlace::Entity(net) => match world
            .get_resource::<NetEntities>()
            .and_then(|all| all.entity(net))
        {
            Some(entity) => PopupPlace::Entity(entity),
            None => return,
        },
        WirePlace::Coordinates { map, position, z } => {
            PopupPlace::Coordinates(MapCoordinates { map, position, z })
        }
        WirePlace::Cursor => PopupPlace::Cursor,
    };
    world
        .get_resource_or_insert_with(PopupQueue::default)
        .0
        .push(Popup {
            message: event.event.message.clone(),
            kind: event.event.kind,
            place,
        });
}

pub(crate) fn install(world: &mut World) {
    world.init_resource::<PopupQueue>();
    world.register_net_event::<PopupMessage>();
    world.subscribe_net_event::<PopupMessage>(receive);
}
