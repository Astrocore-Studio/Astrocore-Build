//! Клиент игры: мир клиента с контентом, сглаживанием спрайтов и скрытыми маркерами.

use astrocore_client::{Sprite, register_client_components, register_draw_depth};
use astrocore_ecs::{ComponentInit, EventWorldExt, GameWorldExt, RegistryWorldExt};
use astrocore_movement::MovementClientPlugin;
use astrocore_net::{
    ClientNet, ClientTransport, ClientWorldExt, ConnId, Incoming, NetClientPlugin, ServerNet,
};
use astrocore_physics::PhysicsPlugin;
use astrocore_resources::Resources;
use bevy_ecs::component::Component;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::content;
use crate::smooth;

/// Служебная метка карты: точка появления, указатель для мапперов. В игре не видна
/// (`MarkerComponent` и `MarkerSystem` SS14).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {}

/// Создаёт мир клиента и отправляет серверу приветствие с именем игрока.
pub fn start(name: &str) -> Result<(World, Resources), String> {
    content::register_collision_groups();
    for (depth, value) in content::DRAW_DEPTHS {
        register_draw_depth(depth, value);
    }
    let (resources, ss14) = content::mount_resources()?;
    let mut world = World::new();
    world.add_plugin(NetClientPlugin);
    world.add_plugin(PhysicsPlugin::CLIENT);
    world.add_plugin(MovementClientPlugin);
    register_client_components(&mut world);
    world.register_data_component::<Marker>();
    world.subscribe::<Marker, ComponentInit>(|world, entity, _| {
        if let Some(mut sprite) = world.get_mut::<Sprite>(entity) {
            sprite.visible = false;
        }
    });
    smooth::install(&mut world);
    let report = content::load_content(&mut world, &resources, &ss14);
    for error in report.errors.iter().take(20) {
        eprintln!("прототипы: {error}");
    }
    world.net_connect(name);
    Ok((world, resources))
}

/// Сервер в процессе клиента: игра в одиночку без сети. Пакеты передаются напрямую, сервер
/// делает такт вместе с клиентом.
pub struct LocalHost {
    pub server: World,
}

const LOCAL: ConnId = ConnId(1);

impl ClientTransport for LocalHost {
    fn receive(&mut self, client: &mut World) {
        let packets = std::mem::take(&mut self.server.resource_mut::<ServerNet>().outgoing);
        client
            .resource_mut::<ClientNet>()
            .incoming
            .extend(packets.into_iter().map(|packet| packet.bytes));
    }

    fn send(&mut self, client: &mut World) {
        let packets = std::mem::take(&mut client.resource_mut::<ClientNet>().outgoing);
        self.server.resource_mut::<ServerNet>().incoming.extend(
            packets
                .into_iter()
                .map(|(_, bytes)| Incoming::Packet(LOCAL, bytes)),
        );
        self.server.run_tick();
    }
}
