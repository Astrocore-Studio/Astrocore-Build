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

use std::any::Any;

use astrocore_api::HookContext;
use astrocore_core::NetworkEntityId;

use crate::module::State;

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
            Box::new(console::ConsoleSystem::from_registry(registry)?),
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

    pub(crate) fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, dt: f32) {
        for s in &mut self.0 {
            s.tick(st, ctx, dt);
        }
    }
}
