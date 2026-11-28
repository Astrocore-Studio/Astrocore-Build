//! Предложение вернуться в тело: когда тело призрака оживляют, игроку приходит окно «Вас
//! оживляют! Вернуться в тело?»; «Да» возвращает его из призрака в тело, «Нет» закрывает
//! окно. Образец — `ReturnToBodyEui` (сервер), `ReturnToBodyEui`, `ReturnToBodyMenu`
//! (клиент), `ReturnToBodyMessage` SS14 @ 2f4a2aad.

use std::collections::HashMap;

use astrocore_ecs::EventWorldExt;
use astrocore_net::{NetEventWorldExt, NetTarget, PlayerDisconnected};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Открыть окно. Сервер → клиент.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnToBodyOpen;

/// Ответ игрока (`ReturnToBodyMessage`). Клиент → сервер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnToBodyResponse {
    pub accepted: bool,
}

/// Открытые предложения: сессия → разум. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct ReturnToBodyOffers(pub HashMap<Entity, Entity>);

/// Окно открыто у клиента. Ресурс клиента.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientReturnToBody {
    pub open: bool,
}

/// Предложить игроку вернуться в тело (`OpenEui(new ReturnToBodyEui)`).
pub fn offer(world: &mut World, session: Entity, mind: Entity) {
    world
        .get_resource_or_insert_with(ReturnToBodyOffers::default)
        .0
        .insert(session, mind);
    world.send_net_event(NetTarget::Session(session), &ReturnToBodyOpen);
}

/// Ответ игрока (`HandleMessage`): «Да» — вернуться из призрака (`UnVisit`).
fn receive(world: &mut World, session: Entity, accepted: bool) {
    let Some(mind) = world
        .get_resource_mut::<ReturnToBodyOffers>()
        .and_then(|mut offers| offers.0.remove(&session))
    else {
        return;
    };
    if accepted {
        crate::mind::unvisit(world, mind);
    }
}

/// Ответ клиента: закрыть окно и отправить выбор.
pub fn client_answer(world: &mut World, accepted: bool) {
    world.insert_resource(ClientReturnToBody { open: false });
    world.send_net_event_to_server(&ReturnToBodyResponse { accepted });
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<ReturnToBodyOpen>();
    world.register_net_event::<ReturnToBodyResponse>();
    if !side.is_server() {
        world.init_resource::<ClientReturnToBody>();
        world.subscribe_net_event::<ReturnToBodyOpen>(|world, _| {
            world.insert_resource(ClientReturnToBody { open: true });
        });
        return;
    }
    world.init_resource::<ReturnToBodyOffers>();
    world.subscribe_net_event::<ReturnToBodyResponse>(|world, event| {
        if let Some(session) = event.session {
            receive(world, session, event.event.accepted);
        }
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        if let Some(mut offers) = world.get_resource_mut::<ReturnToBodyOffers>() {
            offers.0.remove(&event.session);
        }
    });
}

#[cfg(test)]
mod tests {
    use astrocore_net::NetEvent;

    use super::*;
    use crate::mind::tests::{PROTOTYPES, scene};
    use crate::mobs::{MobState, MobStateWorldExt};

    const CORPSE: &str = r#"
- type: entity
  id: Corpse
  components:
  - type: MindContainer
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      200: Dead
"#;

    fn answer(world: &mut World, session: Entity, accepted: bool) {
        world.raise_broadcast(&mut NetEvent {
            session: Some(session),
            event: ReturnToBodyResponse { accepted },
        });
    }

    #[test]
    fn accepting_returns_the_ghost_to_its_body() {
        let mut scene = scene(&format!("{PROTOTYPES}{CORPSE}"));
        let (session, _, body, mind) = scene.player(1);
        scene.world.change_mob_state(body, MobState::Dead, None);
        assert!(crate::ghost::on_ghost_attempt(
            &mut scene.world,
            mind,
            true,
            false,
            false
        ));
        let ghost = scene.attached(session).unwrap();
        assert_ne!(ghost, body);
        // Отказ закрывает предложение, тело не меняется.
        offer(&mut scene.world, session, mind);
        answer(&mut scene.world, session, false);
        assert_eq!(scene.attached(session), Some(ghost));
        // Ответ без предложения ничего не делает.
        answer(&mut scene.world, session, true);
        assert_eq!(scene.attached(session), Some(ghost));
        offer(&mut scene.world, session, mind);
        answer(&mut scene.world, session, true);
        assert_eq!(scene.attached(session), Some(body));
    }
}
