//! Быстрые окна ввода: сервер просит игрока ввести значения (целое, дробное, короткий текст до
//! 100 символов, длинный — до 2000) и получает их в обработчике; «Отмена» и выход игрока
//! вызывают обработчик отмены, неверный ответ отключает игрока. Образец — `QuickDialogSystem`
//! (сервер и клиент), `QuickDialogOpenEvent`, `QuickDialogResponseEvent`, `QuickDialogEntry`
//! SS14 @ 2f4a2aad.

use std::collections::{BTreeMap, HashMap};

use astrocore_ecs::EventWorldExt;
use astrocore_net::{NetEvent, NetEventWorldExt, NetTarget, PlayerDisconnected};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Вид поля (`QuickDialogEntryType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuickDialogEntryType {
    Integer,
    Float,
    ShortText,
    LongText,
}

/// Поле окна (`QuickDialogEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickDialogEntry {
    pub field_id: String,
    pub kind: QuickDialogEntryType,
    pub prompt: String,
    pub placeholder: Option<String>,
}

/// Кнопки (`QuickDialogButtonFlag`).
pub const OK_BUTTON: u8 = 1;
pub const CANCEL_BUTTON: u8 = 2;

/// Открыть окно (`QuickDialogOpenEvent`). Сервер → клиент.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickDialogOpen {
    pub title: String,
    pub dialog_id: u32,
    pub prompts: Vec<QuickDialogEntry>,
    pub buttons: u8,
}

/// Ответ игрока (`QuickDialogResponseEvent`). Клиент → сервер.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickDialogResponse {
    pub dialog_id: u32,
    pub responses: BTreeMap<String, String>,
    pub button: u8,
}

type OkAction = Box<dyn FnOnce(&mut World, &BTreeMap<String, String>) -> bool + Send + Sync>;
type CancelAction = Box<dyn FnOnce(&mut World) + Send + Sync>;

struct OpenDialog {
    session: Entity,
    prompts: Vec<QuickDialogEntry>,
    ok: OkAction,
    cancel: CancelAction,
}

/// Открытые окна. Ресурс сервера.
#[derive(Resource, Default)]
pub struct QuickDialogs {
    next_id: u32,
    open: HashMap<u32, OpenDialog>,
}

/// Проверить значение поля (`TryParseQuickDialog`).
pub fn valid_value(kind: QuickDialogEntryType, input: &str) -> bool {
    match kind {
        QuickDialogEntryType::Integer => input.trim().parse::<i64>().is_ok(),
        QuickDialogEntryType::Float => input.trim().parse::<f64>().is_ok(),
        QuickDialogEntryType::ShortText => input.chars().count() <= 100,
        QuickDialogEntryType::LongText => input.chars().count() <= 2000,
    }
}

/// Открыть окно игроку (`OpenDialogInternal`). `ok` получает ответы по полям; `false` из
/// `ok` значит, что ответ не разобрался.
pub fn open_dialog(
    world: &mut World,
    session: Entity,
    title: &str,
    prompts: Vec<QuickDialogEntry>,
    buttons: u8,
    ok: impl FnOnce(&mut World, &BTreeMap<String, String>) -> bool + Send + Sync + 'static,
    cancel: impl FnOnce(&mut World) + Send + Sync + 'static,
) -> u32 {
    let mut dialogs = world.get_resource_or_insert_with(QuickDialogs::default);
    dialogs.next_id += 1;
    let dialog_id = dialogs.next_id;
    dialogs.open.insert(
        dialog_id,
        OpenDialog {
            session,
            prompts: prompts.clone(),
            ok: Box::new(ok),
            cancel: Box::new(cancel),
        },
    );
    world.send_net_event(
        NetTarget::Session(session),
        &QuickDialogOpen {
            title: title.to_owned(),
            dialog_id,
            prompts,
            buttons,
        },
    );
    dialog_id
}

/// Окно с одним коротким текстом (`OpenDialog<string>`).
pub fn open_text_dialog(
    world: &mut World,
    session: Entity,
    title: &str,
    prompt: &str,
    ok: impl FnOnce(&mut World, String) + Send + Sync + 'static,
) -> u32 {
    open_dialog(
        world,
        session,
        title,
        vec![QuickDialogEntry {
            field_id: "1".into(),
            kind: QuickDialogEntryType::ShortText,
            prompt: prompt.to_owned(),
            placeholder: None,
        }],
        OK_BUTTON | CANCEL_BUTTON,
        move |world, responses| {
            let Some(text) = responses.get("1").cloned() else {
                return false;
            };
            ok(world, text);
            true
        },
        |_| {},
    )
}

/// Ответ игрока (`Handler`): чужое или неверное — отключение.
fn receive(world: &mut World, event: &mut NetEvent<QuickDialogResponse>) {
    let Some(session) = event.session else {
        return;
    };
    let response = event.event.clone();
    let owned = world
        .get_resource::<QuickDialogs>()
        .and_then(|d| d.open.get(&response.dialog_id))
        .is_some_and(|d| d.session == session);
    if !owned {
        crate::admin_commands::kick(
            world,
            session,
            &format!(
                "Replied with invalid quick dialog data with id {}.",
                response.dialog_id
            ),
        );
        return;
    }
    let Some(dialog) = world
        .resource_mut::<QuickDialogs>()
        .open
        .remove(&response.dialog_id)
    else {
        return;
    };
    match response.button {
        OK_BUTTON => {
            let valid = dialog.prompts.iter().all(|prompt| {
                response
                    .responses
                    .get(&prompt.field_id)
                    .is_some_and(|value| valid_value(prompt.kind, value))
            });
            if !valid || !(dialog.ok)(world, &response.responses) {
                crate::admin_commands::kick(
                    world,
                    session,
                    "Provided invalid response to a quick dialog.",
                );
            }
        }
        _ => (dialog.cancel)(world),
    }
}

/// Игрок ушёл — его окна отменяются (`PlayerManagerOnPlayerStatusChanged`).
fn on_disconnect(world: &mut World, session: Entity) {
    let Some(mut dialogs) = world.get_resource_mut::<QuickDialogs>() else {
        return;
    };
    let ids: Vec<u32> = dialogs
        .open
        .iter()
        .filter(|(_, d)| d.session == session)
        .map(|(id, _)| *id)
        .collect();
    let cancels: Vec<CancelAction> = ids
        .into_iter()
        .filter_map(|id| dialogs.open.remove(&id).map(|d| d.cancel))
        .collect();
    for cancel in cancels {
        cancel(world);
    }
}

/// Открытые у клиента окна: ответы по полям и поле с набором. Ресурс клиента.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientQuickDialogs {
    pub dialogs: Vec<ClientQuickDialog>,
    pub version: u64,
}

/// Окно у клиента.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientQuickDialog {
    pub open: QuickDialogOpen,
    pub values: Vec<String>,
    pub focused: usize,
}

/// Ответ клиента на окно: убрать его и отправить серверу (`OnConfirmed`, `OnCancelled`).
pub fn client_answer(world: &mut World, dialog_id: u32, button: u8) {
    let Some(mut dialogs) = world.get_resource_mut::<ClientQuickDialogs>() else {
        return;
    };
    let Some(index) = dialogs
        .dialogs
        .iter()
        .position(|d| d.open.dialog_id == dialog_id)
    else {
        return;
    };
    let dialog = dialogs.dialogs.remove(index);
    dialogs.version += 1;
    let responses = if button == OK_BUTTON {
        dialog
            .open
            .prompts
            .iter()
            .zip(dialog.values)
            .map(|(prompt, value)| (prompt.field_id.clone(), value))
            .collect()
    } else {
        BTreeMap::new()
    };
    world.send_net_event_to_server(&QuickDialogResponse {
        dialog_id,
        responses,
        button,
    });
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<QuickDialogOpen>();
    world.register_net_event::<QuickDialogResponse>();
    if !side.is_server() {
        world.init_resource::<ClientQuickDialogs>();
        world.subscribe_net_event::<QuickDialogOpen>(|world, event| {
            let open = event.event.clone();
            let values = vec![String::new(); open.prompts.len()];
            let mut dialogs = world.get_resource_or_insert_with(ClientQuickDialogs::default);
            dialogs.dialogs.push(ClientQuickDialog {
                open,
                values,
                focused: 0,
            });
            dialogs.version += 1;
        });
        return;
    }
    world.init_resource::<QuickDialogs>();
    world.subscribe_net_event::<QuickDialogResponse>(receive);
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        on_disconnect(world, event.session);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_checked_by_field_kind() {
        assert!(valid_value(QuickDialogEntryType::Integer, " 42 "));
        assert!(!valid_value(QuickDialogEntryType::Integer, "4.2"));
        assert!(valid_value(QuickDialogEntryType::Float, "4.2"));
        assert!(valid_value(
            QuickDialogEntryType::ShortText,
            &"а".repeat(100)
        ));
        assert!(!valid_value(
            QuickDialogEntryType::ShortText,
            &"а".repeat(101)
        ));
        assert!(valid_value(
            QuickDialogEntryType::LongText,
            &"а".repeat(2000)
        ));
    }
}
