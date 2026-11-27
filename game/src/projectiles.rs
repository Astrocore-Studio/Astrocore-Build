//! Снаряды: летят с оружия, при касании твёрдого тела наносят урон и исчезают. Образец —
//! `SharedProjectileSystem` и `ProjectileSystem` SS14 @ 2f4a2aad, `TimedDespawnSystem`
//! Robust @ 1afd692.
//!
//! Снаряд за такт пролетает больше клетки (40 м/с), поэтому кроме касания фикстур путь за
//! такт проверяется лучом: так снаряд не проскакивает сквозь тонкие стены. Цель узнаёт о
//! попадании событием `HitByProjectile` (в SS14 цель ловит `StartCollideEvent` сама).
//!
//! Нет пока: застревания снарядов, пробития, отражения, эффекта попадания.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_map::TransformWorldExt;
use astrocore_physics::{
    BodyStatus, BodyType, CollisionGroups, Fixtures, Physics, PhysicsWorldExt, PreventCollide, Ray,
    RayWorldExt, StartCollide,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::{DamageSpecifier, DamageableWorldExt};

/// Фикстура снаряда, которой он попадает (`ProjectileFixture`).
pub const PROJECTILE_FIXTURE: &str = "projectile";
/// Скорость снаряда по умолчанию, м/с (`SharedGunSystem.ProjectileSpeed`).
pub const PROJECTILE_SPEED: f32 = 40.0;

/// Снаряд (`ProjectileComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Projectile {
    #[serde(default)]
    pub damage: DamageSpecifier,
    #[serde(default = "yes")]
    pub delete_on_collide: bool,
    #[serde(default)]
    pub ignore_resistances: bool,
    #[serde(default)]
    pub sound_hit: Option<SoundSpecifier>,
    #[serde(default)]
    pub only_collide_when_shot: bool,
    #[serde(default)]
    pub projectile_spent: bool,
    /// Сколько секунд снаряд не задевает стрелка (`DelayToAcknowledgeShooter`).
    #[serde(default = "one")]
    pub delay_to_acknowledge_shooter: f64,
    #[serde(skip)]
    pub shooter: Option<Entity>,
    #[serde(skip)]
    pub weapon: Option<Entity>,
    #[serde(skip)]
    pub when_to_stop_ignoring_shooter: f64,
}

fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}

/// Исчезает через время (`TimedDespawnComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedDespawn {
    /// Сколько секунд осталось.
    #[serde(default = "five")]
    pub lifetime: f32,
}

fn five() -> f32 {
    5.0
}

/// Снаряд попал (`ProjectileHitEvent`). Поднимается на снаряде.
#[derive(Debug, Clone)]
pub struct ProjectileHit {
    pub damage: DamageSpecifier,
    pub target: Entity,
    pub shooter: Option<Entity>,
}

/// В сущность попал снаряд. Поднимается на цели.
#[derive(Debug, Clone, Copy)]
pub struct HitByProjectile {
    pub projectile: Entity,
    pub shooter: Option<Entity>,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Выпустить снаряд (`ShootProjectile`): скорость по направлению, стрелок, поворот.
pub fn shoot_projectile(
    world: &mut World,
    projectile: Entity,
    direction: astrocore_math::Vec2,
    gun_velocity: astrocore_math::Vec2,
    gun: Option<Entity>,
    user: Option<Entity>,
    speed: f32,
) {
    if world.get::<Physics>(projectile).is_none() {
        world.entity_mut(projectile).insert(Physics::default());
    }
    // В данных снарядов тип тела часто не задан, а неподвижное тело скорость не берёт.
    if world
        .get::<Physics>(projectile)
        .is_some_and(|b| b.body_type == BodyType::Static)
    {
        world.set_body_type(projectile, BodyType::Dynamic);
    }
    world.set_body_status(projectile, BodyStatus::InAir);
    let length = direction.length();
    let direction = if length > 0.0 {
        direction / length
    } else {
        direction
    };
    world.set_linear_velocity(projectile, gun_velocity + direction * speed);
    let delay = world
        .get::<Projectile>(projectile)
        .map_or(1.0, |p| p.delay_to_acknowledge_shooter);
    let time = now(world);
    if let Some(mut component) = world.get_mut::<Projectile>(projectile) {
        component.weapon = gun;
        component.shooter = user.or(gun);
        component.when_to_stop_ignoring_shooter = time + delay;
    }
    let parent_rotation = world
        .parent(projectile)
        .and_then(|p| world.world_transform(p))
        .map_or(0.0, |p| p.rotation.0);
    let angle = astrocore_math::Angle::from_world_vec(direction).0 - parent_rotation;
    let _ = world.set_local_rotation(projectile, astrocore_math::Angle(angle));
}

fn ignores(world: &World, projectile: &Projectile, other: Entity) -> bool {
    now(world) < projectile.when_to_stop_ignoring_shooter
        && (Some(other) == projectile.shooter || Some(other) == projectile.weapon)
}

/// Попадание в цель (`OnStartCollide`).
pub fn hit(world: &mut World, projectile: Entity, target: Entity) {
    let Some(component) = world.get::<Projectile>(projectile).cloned() else {
        return;
    };
    if component.projectile_spent
        || (component.weapon.is_none() && component.only_collide_when_shot)
    {
        return;
    }
    let mut event = ProjectileHit {
        damage: component.damage.clone(),
        target,
        shooter: component.shooter,
    };
    world.raise(projectile, &mut event);
    world.raise(
        target,
        &mut HitByProjectile {
            projectile,
            shooter: component.shooter,
        },
    );
    if world.is_alive(target) {
        world.change_damage(
            target,
            &event.damage,
            component.ignore_resistances,
            true,
            component.shooter,
        );
        if let Some(sound) = &component.sound_hit {
            world.play_entity(sound, target, None);
        }
    }
    if let Some(mut component) = world.get_mut::<Projectile>(projectile) {
        component.projectile_spent = true;
    }
    if component.delete_on_collide {
        world.queue_delete(projectile);
    }
}

fn on_start_collide(world: &mut World, projectile: Entity, event: &mut StartCollide) {
    if event.our_fixture != PROJECTILE_FIXTURE {
        return;
    }
    let other_hard = world
        .get::<Fixtures>(event.other_entity)
        .and_then(|f| f.fixtures.get(&event.other_fixture))
        .is_some_and(|f| f.hard);
    if !other_hard {
        return;
    }
    hit(world, projectile, event.other_entity);
}

/// Путь за такт лучом: первое твёрдое тело на пути получает попадание.
fn sweep(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let flying: Vec<(Entity, astrocore_math::Vec2, CollisionGroups)> = world
        .query::<(Entity, &Projectile, &Physics, &Fixtures)>()
        .iter(world)
        .filter(|(_, p, body, _)| {
            !p.projectile_spent && body.linear_velocity != astrocore_math::Vec2::ZERO
        })
        .filter_map(|(e, _, body, fixtures)| {
            let mask = fixtures.fixtures.get(PROJECTILE_FIXTURE)?.mask;
            Some((e, body.linear_velocity, mask))
        })
        .collect();
    for (projectile, velocity, mask) in flying {
        let (Some(origin), Some(component)) = (
            world.map_coordinates(projectile),
            world.get::<Projectile>(projectile).cloned(),
        ) else {
            continue;
        };
        let ray = Ray {
            origin,
            direction: velocity,
            max_distance: velocity.length() * dt,
            mask,
        };
        let target = world
            .intersect_ray(
                ray,
                &|e| e == projectile || ignores(world, &component, e),
                true,
            )
            .first()
            .map(|h| h.entity);
        if let Some(target) = target {
            hit(world, projectile, target);
        }
    }
}

fn despawn(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let expired: Vec<Entity> = world
        .query::<(Entity, &mut TimedDespawn)>()
        .iter_mut(world)
        .filter_map(|(e, mut d)| {
            d.lifetime -= dt;
            (d.lifetime <= 0.0).then_some(e)
        })
        .collect();
    for entity in expired {
        world.queue_delete(entity);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Projectile>();
    world.register_data_component::<TimedDespawn>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Projectile, PreventCollide>(|world, projectile, event| {
        if let Some(component) = world.get::<Projectile>(projectile).cloned()
            && ignores(world, &component, event.other_entity)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<Projectile, StartCollide>(on_start_collide);
    world.add_game_systems(Update, (sweep, despawn));
}

#[cfg(test)]
pub(crate) mod tests;
