use astrocore_api::HookContext;
use astrocore_core::NetworkEntityId;
use astrocore_runtime::modules::DoorAction;

use crate::doafter::Action;
use crate::interact::{InteractionHandler, Target, Used};
use crate::module::State;

/// Пустая рука по предмету на земле — взять его в активную руку (`OnHandInteract` у предметов).
pub struct PickupHandler;

impl InteractionHandler for PickupHandler {
    fn interact_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        match target {
            Target::Item { id, .. } => state.pickup(ctx, user, *id, false),
            _ => false,
        }
    }
}

/// Предмет в руке по сумке на земле — положить его внутрь (`ClickInsert` у хранилищ).
pub struct StorageClickInsert;

impl InteractionHandler for StorageClickInsert {
    fn interact_using(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        let is_bag = state
            .infos
            .get(prototype)
            .is_some_and(|i| i.storage.is_some());
        is_bag && state.store(ctx, user, used.hand, *id)
    }
}

/// Пустая рука по двери — открыть или закрыть её (`ActivateInWorld`); замок уважается.
pub struct DoorActivate;

impl InteractionHandler for DoorActivate {
    fn interact_hand(
        &mut self,
        _state: &mut State,
        ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Door { element, .. } = target else {
            return false;
        };
        if ctx.door_locked(*element) == Some(true) {
            return true; // дверь заперта: событие обработано отказом
        }
        ctx.request_door(*element, DoorAction::Toggle, false);
        true
    }
}

/// Монтировка (качество `Prying`) в руке по закрытой двери — распахнуть её силой, в том числе
/// запертую. Открытую дверь монтировкой не трогаем.
pub struct PryDoor;

impl InteractionHandler for PryDoor {
    fn interact_using(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
        target: &Target,
    ) -> bool {
        let Target::Door { element, .. } = target else {
            return false;
        };
        let prying = state
            .infos
            .get(&used.item.prototype)
            .is_some_and(|i| i.qualities.contains("Prying"));
        if !prying || ctx.door_open(*element) != Some(false) {
            return false;
        }
        state.begin(
            ctx,
            user,
            Action::Pry {
                element: *element,
                hand: used.hand,
            },
        );
        true
    }
}

/// Базовая цепочка: порядок важен — первый обработавший прекращает обход.
pub fn default_handlers() -> Vec<Box<dyn InteractionHandler>> {
    vec![
        Box::new(PryDoor),
        Box::new(StorageClickInsert),
        Box::new(PickupHandler),
        Box::new(DoorActivate),
    ]
}
