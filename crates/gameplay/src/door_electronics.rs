use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;
use serde::{Deserialize, Serialize};

use crate::{State, crafting::CATALOG};

pub const COMPONENT: &str = "build.door_electronics";
pub const CMD_UPDATE: &str = "door_electronics.update";
pub const CMD_CLOSE: &str = "door_electronics.close";
const MAX_BYTES: usize = 8192;

#[derive(Serialize, Deserialize)]
pub struct View {
    pub board: ObjectId,
    pub levels: Vec<String>,
    pub selected: Vec<String>,
}

pub fn decode(bytes: &[u8]) -> Option<View> {
    (bytes.len() <= MAX_BYTES)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

/// Настройка доступа платы двери (`AccessOverrider` SS14): окно у того, кто держит настройщик.
/// Доступ платы — свойство предмета (`State::board_access`), окна — у системы.
#[derive(Default)]
pub struct DoorElectronics {
    windows: std::collections::BTreeMap<NetworkEntityId, (ObjectId, Vec<u8>)>,
}

impl crate::systems::GameSystem for DoorElectronics {
    fn id(&self) -> &'static str {
        "door_electronics"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[CMD_UPDATE, CMD_CLOSE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        if kind == CMD_UPDATE {
            self.update_board_config(st, ctx, actor, data);
        } else {
            self.close_board_config(ctx, actor);
        }
    }

    fn interact_using(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        _used: &crate::interact::Used,
        target: &crate::interact::Target,
    ) -> bool {
        let crate::interact::Target::Item { id, .. } = target else {
            return false;
        };
        self.open_board_config(st, ctx, user, *id)
    }

    fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, _dt: f32) {
        self.refresh_board_windows(st, ctx);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl DoorElectronics {
    fn board_view(
        &self,
        st: &State,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        board: ObjectId,
    ) -> Option<View> {
        let who = st.actors.get(&actor)?;
        if who.ghost || !who.health.state().can_act() {
            return None;
        }
        let used = st.used_item(actor)?;
        let tool = st.ss14_names.get(&used.item.prototype)?;
        if !CATALOG["boardConfigurators"]
            .as_array()?
            .iter()
            .any(|name| name == tool)
        {
            return None;
        }
        let feet = ctx.position_of(actor)?;
        let prototype = if let Some(item) = who.hands.held().find(|item| item.id == board) {
            &item.prototype
        } else {
            let object = ctx.world.authored_object(board)?;
            let AuthoredKind::Placement { prototype } = &object.kind else {
                return None;
            };
            let at = (object.min() + object.max()) * 0.5;
            let chest = feet + Vec3::Y * crate::module::CHEST_M;
            if chest.distance(at) > st.range_m || !ctx.is_unobstructed(chest, at) {
                return None;
            }
            prototype
        };
        let name = st.ss14_names.get(prototype)?;
        let defaults = CATALOG["doorBoards"][name].as_array()?;
        let selected = match st.board_access.get(&board) {
            Some(access) => access
                .split(';')
                .filter_map(|list| list.split(',').next())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            None => defaults
                .iter()
                .filter_map(|list| list.as_array()?.first()?.as_str().map(str::to_owned))
                .collect(),
        };
        Some(View {
            board,
            selected,
            levels: serde_json::from_value(CATALOG["accessLevels"].clone()).unwrap(),
        })
    }

    pub(crate) fn open_board_config(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        board: ObjectId,
    ) -> bool {
        if self.board_view(st, ctx, actor, board).is_none() {
            return false;
        }
        self.windows.insert(actor, (board, Vec::new()));
        self.refresh_board_windows(st, ctx);
        true
    }

    pub(crate) fn update_board_config(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        bytes: &[u8],
    ) {
        if bytes.len() > MAX_BYTES {
            return;
        }
        let Ok(selected) = serde_json::from_slice::<Vec<String>>(bytes) else {
            return;
        };
        let Some(&(board, _)) = self.windows.get(&actor) else {
            return;
        };
        let Some(view) = self.board_view(st, ctx, actor, board) else {
            return;
        };
        if selected.iter().any(|id| !view.levels.contains(id)) {
            return;
        }
        // AccessReaderSystem.TrySetAccesses accepts each selected level as an alternative.
        let selected: std::collections::BTreeSet<_> = selected.into_iter().collect();
        st.board_access
            .insert(board, selected.into_iter().collect::<Vec<_>>().join(";"));
        self.refresh_board_windows(st, ctx);
    }

    pub(crate) fn close_board_config(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        self.windows.remove(&actor);
        ctx.set_component(actor, COMPONENT, Vec::new());
    }

    pub(crate) fn refresh_board_windows(&mut self, st: &mut State, ctx: &mut HookContext<'_>) {
        let open: Vec<_> = self
            .windows
            .iter()
            .map(|(actor, (board, _))| (*actor, *board))
            .collect();
        for (actor, board) in open {
            let Some(view) = self.board_view(st, ctx, actor, board) else {
                self.close_board_config(ctx, actor);
                continue;
            };
            let bytes = serde_json::to_vec(&view).unwrap();
            let (_, last) = self.windows.get_mut(&actor).unwrap();
            if *last != bytes {
                *last = bytes.clone();
                ctx.set_component(actor, COMPONENT, bytes);
            }
        }
    }
}
