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
        state: &mut State,
        ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Door { element, .. } = target else {
            return false;
        };
        // Огнезапор держит давление или жар: рукой не открыть (`BeforeDoorOpenedEvent`).
        if state.firelock_hold.get(element).is_some_and(|h| h.0 || h.1) {
            return true;
        }
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

/// Использовать закрытую бутылку в руке — открыть её (`OpenableSystem.OnUse`).
pub struct OpenableUse;

impl InteractionHandler for OpenableUse {
    fn use_in_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        let opened = state.open(&used.item);
        if opened {
            state.publish(ctx, user);
        }
        opened
    }
}

/// Использовать еду или питьё в руке — съесть или выпить укус (`IngestionSystem.OnUseEdibleInHand`).
pub struct EdibleUse;

impl InteractionHandler for EdibleUse {
    fn use_in_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        if !state.edibles.contains_key(&used.item.prototype) {
            return false;
        }
        state.begin(
            ctx,
            user,
            Action::Eat {
                hand: used.hand,
                item: used.item.id,
                target: None,
            },
        );
        true
    }
}

/// Использовать в руке источник огня (сварку, спичку) — зажечь или погасить его.
pub struct IgniterUse;

impl InteractionHandler for IgniterUse {
    fn use_in_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        if !state.igniters.contains_key(&used.item.prototype) {
            return false;
        }
        if !state.lit.remove(&used.item.id) {
            state.lit.insert(used.item.id);
        }
        true
    }
}

/// F по переносному баллончику в руке — открыть или закрыть выпуск газа (`ToggleValve`).
pub struct TankUse;

impl InteractionHandler for TankUse {
    fn use_in_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        match state.tank_of(&used.item) {
            Some(t) => {
                t.release_open = !t.release_open;
                true
            }
            None => false,
        }
    }
}

/// Устройства труб: пустая рука включает насос, вентиляцию, скруббер, клапан и выпуск баллона;
/// с гаечным ключом (`Anchoring`) баллон подключается к разъёму и отключается, насос меняет
/// целевое давление, вентиль и скруббер переключают режим.
pub struct GasDeviceUse;

impl GasDeviceUse {
    fn act(state: &mut State, target: &Target, wrench: bool) -> bool {
        use atmos::Device;
        let Target::Item { id, .. } = target else {
            return false;
        };
        let Some(key) = state.pipe_keys.existing(*id) else {
            return false;
        };
        let port = state
            .pipe_sig
            .iter()
            .find(|(o, _, _)| o == id)
            .and_then(|(_, cell, _)| state.port_node_at(*cell));
        let Some(d) = state.atmos.pipes_mut().device_mut(key) else {
            return false;
        };
        match d {
            Device::PressurePump {
                enabled, target, ..
            } => {
                if wrench {
                    const PRESETS: [f32; 4] = [101.325, 303.975, 1013.25, 4500.0];
                    let next = PRESETS.iter().position(|p| *p > *target + 0.01);
                    *target = PRESETS[next.unwrap_or(0)];
                } else {
                    *enabled = !*enabled;
                }
            }
            Device::VentPump(v) => {
                if wrench {
                    v.releasing = !v.releasing;
                } else {
                    v.enabled = !v.enabled;
                }
            }
            Device::Scrubber(s) => {
                if wrench {
                    s.siphoning = !s.siphoning;
                } else {
                    s.enabled = !s.enabled;
                }
            }
            Device::Valve { open, .. } => *open = !*open,
            Device::Canister(c) => {
                if wrench {
                    c.port = if c.port.is_some() { None } else { port };
                } else {
                    c.release_open = !c.release_open;
                }
            }
            Device::PassiveVent { .. } => return false,
        }
        true
    }
}

impl InteractionHandler for GasDeviceUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        Self::act(state, target, false)
    }

    fn interact_using(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        used: &Used,
        target: &Target,
    ) -> bool {
        let wrench = state
            .infos
            .get(&used.item.prototype)
            .is_some_and(|i| i.qualities.contains("Anchoring"));
        wrench && Self::act(state, target, true)
    }
}

/// Ёмкость диспозала: предмет из руки кладётся внутрь (`AfterInteractUsing`), пустая рука
/// включает или выключает смыв (кнопка `Engage` интерфейса SS14).
pub struct DisposalUnitUse;

impl InteractionHandler for DisposalUnitUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        let now = state.clock;
        let Some(slot) = state.disposal_state.get_mut(id) else {
            return false;
        };
        let engaged = slot.unit.engaged;
        slot.unit.set_engage(!engaged, now);
        true
    }

    fn interact_using(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        if !state.disposal_state.contains_key(id) {
            return false;
        }
        // Нельзя сложить в ёмкость саму ёмкость (стоит на полу).
        state.insert_into_unit(ctx, user, *id, used.hand)
    }
}

/// Выключатель (`SignalSwitch`): пустая рука переключает его и связанные ленты.
pub struct SwitchUse;

impl InteractionHandler for SwitchUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        state.switch_protos.contains(prototype) && state.toggle_switch(ctx, *id)
    }
}

/// Базовая цепочка: порядок важен — первый обработавший прекращает обход.
pub fn default_handlers() -> Vec<Box<dyn InteractionHandler>> {
    vec![
        Box::new(GasDeviceUse),
        Box::new(DisposalUnitUse),
        Box::new(SwitchUse),
        Box::new(PryDoor),
        Box::new(StorageClickInsert),
        Box::new(PickupHandler),
        Box::new(DoorActivate),
        Box::new(IgniterUse),
        Box::new(TankUse),
        Box::new(OpenableUse),
        Box::new(EdibleUse),
    ]
}
