use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;

use crate::hands::HeldItem;
use crate::module::State;

/// Цель взаимодействия в мире.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Предмет на земле.
    Item {
        id: ObjectId,
        prototype: PrototypeId,
        at: Vec3,
    },
    /// Дверь (структурный элемент-проём).
    Door { element: ObjectId, at: Vec3 },
}

impl Target {
    /// Точка цели: от неё считаются дальность и видимость.
    pub fn at(&self) -> Vec3 {
        match self {
            Target::Item { at, .. } | Target::Door { at, .. } => *at,
        }
    }

    pub fn id(&self) -> ObjectId {
        match self {
            Target::Item { id, .. } => *id,
            Target::Door { element, .. } => *element,
        }
    }
}

/// Предмет, которым действует игрок: рука и сам предмет.
#[derive(Debug, Clone, PartialEq)]
pub struct Used {
    pub hand: usize,
    pub item: HeldItem,
}

/// Обработчик взаимодействий. Цепочка вызывается по порядку; первый вернувший `true` «съедает»
/// событие. Порядок и смысл — как в SS14: `InteractHand` (пустая рука), `InteractUsing` (обработчики
/// цели), затем `AfterInteract` (обработчики самого предмета), `UseInHand`. Форк добавляет
/// собственные обработчики (`GameplayModule::with_handler`), не меняя базовые.
pub trait InteractionHandler: Send {
    fn interact_hand(
        &mut self,
        _state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _target: &Target,
    ) -> bool {
        false
    }

    fn interact_using(
        &mut self,
        _state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _used: &Used,
        _target: &Target,
    ) -> bool {
        false
    }

    fn after_interact(
        &mut self,
        _state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _used: &Used,
        _target: &Target,
    ) -> bool {
        false
    }

    fn use_in_hand(
        &mut self,
        _state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        _used: &Used,
    ) -> bool {
        false
    }
}
