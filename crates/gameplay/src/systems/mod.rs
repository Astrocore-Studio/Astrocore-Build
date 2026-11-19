//! Игровые системы Build (ADR-0001): у каждой механики своё состояние, свои команды клиента и
//! свой тик. Общее состояние мира (персонажи, предметы, должности, лобби) — [`State`]; система
//! получает его по ссылке и не хранит чужих данных.
//!
//! Систему можно отключить ([`crate::GameplayModule::without_system`]) или заменить своей с тем же
//! [`GameSystem::id`] ([`crate::GameplayModule::with_system`]) — так форк меняет механику, не
//! трогая остальной код. Пока перенос не закончен, часть механик живёт прямо в `State`
//! (`docs/SYSTEMS.md`).

pub mod chat;
pub mod console;
pub mod mapping;

use std::any::Any;

use astrocore_api::HookContext;
use astrocore_core::NetworkEntityId;

use crate::module::State;

/// Событие для систем: общий код или другая система сообщает о том, что случилось, а
/// заинтересованная система решает, что с этим делать. Разбираются после события модуля, в том же
/// тике.
#[derive(Debug, Clone, PartialEq)]
pub enum GameEvent {
    /// Автомат сломан и выбрасывает часть запаса (`EjectVendorItems` в `Destructible`).
    EjectVendorItems {
        object: astrocore_core::ObjectId,
        percent: f32,
        max: f32,
        at: astrocore_math::glam::Vec3,
    },
}

/// Одна игровая механика.
pub trait GameSystem: Send + Any {
    /// Имя системы: по нему её отключают и заменяют.
    fn id(&self) -> &'static str;

    /// Команды клиента, которые система разбирает сама (до общего разбора модуля).
    fn commands(&self) -> &'static [&'static str] {
        &[]
    }

    /// Команда клиента из [`Self::commands`].
    fn command(
        &mut self,
        _st: &mut State,
        _ctx: &mut HookContext<'_>,
        _actor: NetworkEntityId,
        _kind: &str,
        _data: &[u8],
    ) {
    }

    /// Любая команда клиента, до того как её разберут (не перехватывает её): так система
    /// реагирует на общие команды вроде «сопротивляться».
    fn before_command(
        &mut self,
        _st: &mut State,
        _ctx: &mut HookContext<'_>,
        _actor: NetworkEntityId,
        _kind: &str,
        _data: &[u8],
    ) {
    }

    /// Действие с задержкой этой системы дошло до конца (`Action::System`). Ход, прерывание шагом
    /// и полосу у клиента ведёт `State`; проверить, что действие ещё возможно, — дело системы.
    fn finish_job(
        &mut self,
        _st: &mut State,
        _ctx: &mut HookContext<'_>,
        _actor: NetworkEntityId,
        _job: &crate::doafter::SystemJob,
    ) {
    }

    /// Событие для систем (`GameEvent`).
    fn on_event(&mut self, _st: &mut State, _ctx: &mut HookContext<'_>, _event: &GameEvent) {}

    /// Щелчок пустой рукой по цели — до общих обработчиков взаимодействия; `true`, если система
    /// его обработала.
    fn interact_hand(
        &mut self,
        _st: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _target: &crate::interact::Target,
    ) -> bool {
        false
    }

    /// Предмет в руке применён к цели (там, где модуль проверяет предметы-цели); `true`, если
    /// система его обработала.
    fn interact_using(
        &mut self,
        _st: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _used: &crate::interact::Used,
        _target: &crate::interact::Target,
    ) -> bool {
        false
    }

    /// Игрок отключился.
    fn player_left(&mut self, _st: &mut State, _actor: NetworkEntityId) {}

    /// Каждый тик, после общей симуляции `State`.
    fn tick(&mut self, _st: &mut State, _ctx: &mut HookContext<'_>, _dt: f32) {}

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Системы модуля в порядке тика.
#[derive(Default)]
pub struct Systems(Vec<Box<dyn GameSystem>>);

impl Systems {
    /// Системы Build по умолчанию.
    pub(crate) fn defaults(
        registry: &prototypes::PrototypeRegistry,
    ) -> Result<Self, prototypes::ProtoError> {
        Ok(Self(vec![
            Box::new(chat::ChatSystem::default()),
            Box::new(console::ConsoleSystem::default()),
            Box::new(mapping::MappingSystem::default()),
            Box::new(crate::power_monitor::Monitoring::load(registry)),
            Box::new(crate::door_electronics::DoorElectronics::default()),
            Box::new(crate::cryo::CryoSystem::from_registry(registry)?),
            Box::new(crate::vending::VendingSystem::from_registry(registry)?),
        ]))
    }

    /// Добавить систему; система с тем же именем заменяется на месте.
    pub fn insert(&mut self, system: Box<dyn GameSystem>) {
        match self.0.iter().position(|s| s.id() == system.id()) {
            Some(i) => self.0[i] = system,
            None => self.0.push(system),
        }
    }

    /// Убрать систему по имени; `false`, если такой нет.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|s| s.id() != id);
        self.0.len() != before
    }

    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().map(|s| s.id())
    }

    /// Система по типу (для настройки при сборке модуля).
    pub fn get_mut<T: GameSystem>(&mut self) -> Option<&mut T> {
        self.0
            .iter_mut()
            .find_map(|s| s.as_any_mut().downcast_mut::<T>())
    }

    /// Отдать команду системе, которая её разбирает; `true`, если такая нашлась.
    pub(crate) fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) -> bool {
        let Some(s) = self.0.iter_mut().find(|s| s.commands().contains(&kind)) else {
            return false;
        };
        s.command(st, ctx, actor, kind, data);
        true
    }

    pub(crate) fn before_command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        for s in &mut self.0 {
            s.before_command(st, ctx, actor, kind, data);
        }
    }

    /// Подвести итог действиям систем, завершившимся за событие, и раздать события систем.
    /// Действие отключённой системы пропадает, событие без заинтересованных — тоже.
    pub(crate) fn finish_jobs(&mut self, st: &mut State, ctx: &mut HookContext<'_>) {
        while !st.system_jobs.is_empty() || !st.events.is_empty() {
            for (actor, job) in std::mem::take(&mut st.system_jobs) {
                if let Some(s) = self.0.iter_mut().find(|s| s.id() == job.system) {
                    s.finish_job(st, ctx, actor, &job);
                }
            }
            for event in std::mem::take(&mut st.events) {
                for s in &mut self.0 {
                    s.on_event(st, ctx, &event);
                }
            }
        }
    }

    pub(crate) fn interact_hand(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &crate::interact::Target,
    ) -> bool {
        self.0
            .iter_mut()
            .any(|s| s.interact_hand(st, ctx, user, target))
    }

    pub(crate) fn interact_using(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &crate::interact::Used,
        target: &crate::interact::Target,
    ) -> bool {
        self.0
            .iter_mut()
            .any(|s| s.interact_using(st, ctx, user, used, target))
    }

    pub(crate) fn player_left(&mut self, st: &mut State, actor: NetworkEntityId) {
        for s in &mut self.0 {
            s.player_left(st, actor);
        }
    }

    pub(crate) fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, dt: f32) {
        for s in &mut self.0 {
            s.tick(st, ctx, dt);
        }
    }
}
