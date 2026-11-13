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
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Door { element, .. } = target else {
            return false;
        };
        // Считыватель не пускает без нужного доступа.
        if !state.door_allowed(ctx, user, *element) {
            state.popup(user, "Доступ запрещён");
            if let Target::Door { at, .. } = target {
                state.sound("Machines/airlock_deny.ogg", *at);
            }
            return true;
        }
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
            Device::Filter { enabled, gas, .. } => {
                if wrench {
                    // Гаечный ключ листает фильтруемый газ: ничего → кислород → … → фрезон → ничего.
                    use atmos::Gas::*;
                    const ORDER: [atmos::Gas; 9] = [
                        Oxygen,
                        Nitrogen,
                        CarbonDioxide,
                        Plasma,
                        Tritium,
                        WaterVapor,
                        Ammonia,
                        NitrousOxide,
                        Frezon,
                    ];
                    *gas = match gas.and_then(|g| ORDER.iter().position(|o| *o == g)) {
                        None if gas.is_none() => Some(ORDER[0]),
                        Some(i) if i + 1 < ORDER.len() => Some(ORDER[i + 1]),
                        _ => None,
                    };
                } else {
                    *enabled = !*enabled;
                }
            }
            Device::Thermo(t) => {
                if wrench {
                    // Целевая температура по шагам: 100, 150, 200, 293,15, 350, 450 К.
                    const PRESETS: [f32; 6] = [100.0, 150.0, 200.0, 293.15, 350.0, 450.0];
                    let next = PRESETS.iter().position(|p| *p > t.target + 0.01);
                    t.target = PRESETS[next.unwrap_or(0)];
                } else {
                    t.enabled = !t.enabled;
                }
            }
            Device::Canister(c) => {
                if wrench {
                    c.port = if c.port.is_some() { None } else { port };
                } else {
                    c.release_open = !c.release_open;
                }
            }
            Device::PassiveVent { .. } | Device::CryoPod { .. } => return false,
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

/// Торговый автомат: пустая рука открывает окно товаров.
pub struct VendingUse;

impl InteractionHandler for VendingUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        state.open_vending(user, *id)
    }
}

/// Шкаф или ящик: пустая рука отпирает запертый (`LockSystem.OnActivated`), иначе открывает или
/// закрывает (`EntityStorage.ToggleOpen`).
pub struct LockerUse;

impl InteractionHandler for LockerUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        let Some(l) = state.lockers.get(id) else {
            return false;
        };
        if l.locked {
            state.toggle_lock(ctx, user, *id, true);
            return true;
        }
        state.toggle_locker(ctx, *id)
    }
}

/// Консоль выдачи ID: пустая рука открывает окно, ID-карта в руке вставляется в первый свободный слот.
pub struct IdConsoleUse;

impl InteractionHandler for IdConsoleUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        state.id_console_levels.contains_key(prototype) && state.open_id_console(user, *id)
    }

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
        state.id_console_levels.contains_key(prototype)
            && state.insert_id(ctx, user, *id, None, used.hand)
    }
}

/// Ёмкости с растворами и шприцы: предмет из руки на ёмкость в мире переливает или набирает;
/// «использовать в руке» переключает режим шприца.
pub struct ChemUse;

impl InteractionHandler for ChemUse {
    fn interact_using(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        let target_item = crate::hands::HeldItem {
            id: *id,
            prototype: prototype.clone(),
        };
        let handled = state.chem_use(&used.item, &target_item);
        if handled {
            state.popup(user, "Содержимое перелито");
        }
        handled
    }

    fn use_in_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        _user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        state.toggle_injector(&used.item)
    }
}

/// Дефибриллятор: «использовать в руке» включает и выключает; приложенный к трупу (предмет-тело)
/// запускает разряд по владельцу тела.
pub struct DefibUse;

impl InteractionHandler for DefibUse {
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
        if !state.defibs.contains_key(&used.item.prototype) {
            return false;
        }
        let Some(owner) = state
            .actors
            .iter()
            .find(|(_, a)| a.ghost && a.corpse == Some(*id))
            .map(|(n, _)| *n)
        else {
            return false;
        };
        state.begin(
            ctx,
            user,
            crate::doafter::Action::Defib {
                hand: used.hand,
                target: owner,
            },
        );
        true
    }

    fn use_in_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        state.toggle_defib(ctx, user, &used.item)
    }
}

/// Криокапсула: пустая рука открывает окно, мензурка в руке встаёт в слот.
pub struct CryoUse;

impl InteractionHandler for CryoUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        state.open_cryo(user, *id)
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
        state.cryo.contains_key(id) && state.cryo_insert(ctx, user, *id, used.hand)
    }
}

/// Химические машины: пустая рука открывает окно, мензурка в руке встаёт в слот.
pub struct LabUse;

impl InteractionHandler for LabUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, .. } = target else {
            return false;
        };
        state.open_lab(user, *id)
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
        (state.mixer_machines.contains_key(id) || state.masters.contains_key(id))
            && state.lab_insert(ctx, user, *id, used.hand)
    }
}

/// Предметы-смесители (ложка, шейкер, бумажная центрифуга, библия): применение на раствор
/// в мире или «использовать в руке» запускает смешивание.
pub struct MixUse;

impl InteractionHandler for MixUse {
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
        let machine = state
            .lab
            .mixers
            .get(&used.item.prototype)
            .is_some_and(|m| m.kind == crate::chemlab::MixerKind::Machine);
        if !machine || !state.lab.mixable.contains(prototype) {
            return false;
        }
        state.begin(
            ctx,
            user,
            crate::doafter::Action::Mix {
                hand: used.hand,
                target: Some(*id),
                shake: false,
            },
        );
        true
    }

    fn use_in_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &Used,
    ) -> bool {
        let handheld = state
            .lab
            .mixers
            .get(&used.item.prototype)
            .is_some_and(|m| m.kind == crate::chemlab::MixerKind::Handheld);
        if !handheld {
            return false;
        }
        state.begin(
            ctx,
            user,
            crate::doafter::Action::Mix {
                hand: used.hand,
                target: None,
                shake: false,
            },
        );
        true
    }
}

/// Раздатчик реагентов: пустая рука открывает окно, мензурка (`FitsInDispenser`) в руке встаёт в слот.
pub struct DispenserUse;

impl InteractionHandler for DispenserUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        state.dispenser_tables.contains_key(prototype) && state.open_dispenser(user, *id)
    }

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
        state.dispenser_tables.contains_key(prototype)
            && state.insert_beaker(ctx, user, *id, used.hand)
    }
}

/// Базовая цепочка: порядок важен — первый обработавший прекращает обход.
pub struct PowerMonitorUse;

impl InteractionHandler for PowerMonitorUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        state.power_monitor.consoles.contains(prototype) && state.open_power_monitor(ctx, user, *id)
    }
}

pub fn default_handlers() -> Vec<Box<dyn InteractionHandler>> {
    vec![
        Box::new(AtmosAlertsUse),
        Box::new(PowerMonitorUse),
        Box::new(GasDeviceUse),
        Box::new(DisposalUnitUse),
        Box::new(SwitchUse),
        Box::new(VendingUse),
        Box::new(LockerUse),
        Box::new(IdConsoleUse),
        Box::new(DispenserUse),
        Box::new(CryoUse),
        Box::new(LabUse),
        Box::new(MixUse),
        Box::new(DefibUse),
        Box::new(ChemUse),
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

pub struct AtmosAlertsUse;
impl InteractionHandler for AtmosAlertsUse {
    fn interact_hand(
        &mut self,
        state: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &Target,
    ) -> bool {
        let Target::Item { id, prototype, .. } = target else {
            return false;
        };
        state.use_atmos_alerts(ctx, user, *id, prototype)
    }
}
