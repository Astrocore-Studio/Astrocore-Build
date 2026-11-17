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

impl State {
    fn board_view(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        board: ObjectId,
    ) -> Option<View> {
        let who = self.actors.get(&actor)?;
        if who.ghost || !who.health.state().can_act() {
            return None;
        }
        let used = self.used_item(actor)?;
        let tool = self.ss14_names.get(&used.item.prototype)?;
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
            if chest.distance(at) > self.range_m || !ctx.is_unobstructed(chest, at) {
                return None;
            }
            prototype
        };
        let name = self.ss14_names.get(prototype)?;
        let defaults = CATALOG["doorBoards"][name].as_array()?;
        let selected = match self.board_access.get(&board) {
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
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        board: ObjectId,
    ) -> bool {
        if self.board_view(ctx, actor, board).is_none() {
            return false;
        }
        self.board_windows.insert(actor, (board, Vec::new()));
        self.refresh_board_windows(ctx);
        true
    }

    pub(crate) fn update_board_config(
        &mut self,
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
        let Some(&(board, _)) = self.board_windows.get(&actor) else {
            return;
        };
        let Some(view) = self.board_view(ctx, actor, board) else {
            return;
        };
        if selected.iter().any(|id| !view.levels.contains(id)) {
            return;
        }
        // AccessReaderSystem.TrySetAccesses accepts each selected level as an alternative.
        let selected: std::collections::BTreeSet<_> = selected.into_iter().collect();
        self.board_access
            .insert(board, selected.into_iter().collect::<Vec<_>>().join(";"));
        self.refresh_board_windows(ctx);
    }

    pub(crate) fn close_board_config(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        self.board_windows.remove(&actor);
        ctx.set_component(actor, COMPONENT, Vec::new());
    }

    pub(crate) fn refresh_board_windows(&mut self, ctx: &mut HookContext<'_>) {
        let open: Vec<_> = self
            .board_windows
            .iter()
            .map(|(actor, (board, _))| (*actor, *board))
            .collect();
        for (actor, board) in open {
            let Some(view) = self.board_view(ctx, actor, board) else {
                self.close_board_config(ctx, actor);
                continue;
            };
            let bytes = serde_json::to_vec(&view).unwrap();
            let (_, last) = self.board_windows.get_mut(&actor).unwrap();
            if *last != bytes {
                *last = bytes.clone();
                ctx.set_component(actor, COMPONENT, bytes);
            }
        }
    }
}
