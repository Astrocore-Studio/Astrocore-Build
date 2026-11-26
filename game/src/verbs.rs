//! Глаголы: действия с сущностью из контекстного меню («Взять», «Открыть», Alt+клик).
//! Образец — `Content.Shared/Verbs` SS14 @ 2f4a2aad (`Verb`, `GetVerbsEvent<T>`,
//! `SharedVerbSystem.GetLocalVerbs`, `ExecuteVerb`).
//!
//! Системы добавляют глаголы, отвечая на [`GetVerbs`]. Клиент просит список глаголов цели,
//! сервер отвечает без действий; выбранный глагол клиент присылает обратно, и сервер
//! находит его заново среди своих — так нельзя выполнить то, чего нет.

use std::cmp::Ordering;
use std::sync::Arc;

use astrocore_ecs::{EventWorldExt, LifecycleWorldExt};
use astrocore_net::{NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetTarget, Session};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::interaction::{INTERACTION_RANGE, InteractionWorldExt};

/// Вид глагола (`InteractionVerb`, `AlternativeVerb` и соседи).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerbKind {
    /// Обычное действие с целью; поднимается на цели.
    Interaction,
    /// Действие предметом в руке; поднимается на предмете.
    Utility,
    /// Своё действие действующего; поднимается на нём.
    Innate,
    /// Alt+клик.
    Alternative,
    /// «Использовать» (E).
    Activation,
    /// Подробности в окне осмотра.
    Examine,
    /// Снаряжение на цели.
    Equipment,
    /// Прочее.
    Other,
}

impl VerbKind {
    pub const ALL: [VerbKind; 8] = [
        VerbKind::Interaction,
        VerbKind::Utility,
        VerbKind::Innate,
        VerbKind::Alternative,
        VerbKind::Activation,
        VerbKind::Examine,
        VerbKind::Equipment,
        VerbKind::Other,
    ];

    /// Место вида в меню (`TypePriority`): больше — выше.
    pub fn type_priority(self) -> i32 {
        match self {
            VerbKind::Equipment => 5,
            VerbKind::Interaction => 4,
            VerbKind::Utility | VerbKind::Innate => 3,
            VerbKind::Alternative => 2,
            VerbKind::Activation => 1,
            VerbKind::Examine | VerbKind::Other => 0,
        }
    }

    /// Касание при выполнении по умолчанию (`DefaultDoContactInteraction`).
    fn default_contact(self) -> bool {
        matches!(
            self,
            VerbKind::Interaction
                | VerbKind::Utility
                | VerbKind::Alternative
                | VerbKind::Activation
        )
    }
}

/// Действие глагола: выполняется на сервере.
pub type VerbAct = Arc<dyn Fn(&mut World) + Send + Sync>;

/// Глагол (`Verb`).
#[derive(Clone)]
pub struct Verb {
    pub kind: VerbKind,
    pub text: String,
    /// Категория: глаголы одной категории собираются в подменю.
    pub category: Option<String>,
    pub priority: i32,
    pub disabled: bool,
    /// Пояснение у неактивного глагола.
    pub message: Option<String>,
    /// Картинка: путь от `Textures`.
    pub icon: Option<String>,
    pub close_menu: Option<bool>,
    pub confirmation_popup: bool,
    pub do_contact_interaction: Option<bool>,
    pub act: Option<VerbAct>,
}

impl std::fmt::Debug for Verb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verb")
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("category", &self.category)
            .finish()
    }
}

impl Verb {
    pub fn new(
        kind: VerbKind,
        text: impl Into<String>,
        act: impl Fn(&mut World) + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            text: text.into(),
            category: None,
            priority: 0,
            disabled: false,
            message: None,
            icon: None,
            close_menu: None,
            confirmation_popup: false,
            do_contact_interaction: None,
            act: Some(Arc::new(act)),
        }
    }

    fn key(&self) -> VerbKey {
        VerbKey {
            kind: self.kind,
            text: self.text.clone(),
            category: self.category.clone(),
        }
    }

    /// Порядок в меню (`Verb.CompareTo`): вид, приоритет, категория, текст.
    pub fn order(&self, other: &Verb) -> Ordering {
        other
            .kind
            .type_priority()
            .cmp(&self.kind.type_priority())
            .then(other.priority.cmp(&self.priority))
            .then(self.category.cmp(&other.category))
            .then(self.text.cmp(&other.text))
            .then(self.icon.cmp(&other.icon))
    }
}

/// Сбор глаголов одного вида (`GetVerbsEvent<T>`). Поднимается на цели (на предмете для
/// `Utility`, на действующем для `Innate`) и широковещательно.
#[derive(Debug, Clone)]
pub struct GetVerbs {
    pub kind: VerbKind,
    pub user: Entity,
    pub target: Entity,
    /// Предмет в активной руке.
    pub using: Option<Entity>,
    pub can_interact: bool,
    pub can_complex_interact: bool,
    /// Цель в пределах досягаемости и доступна.
    pub can_access: bool,
    pub verbs: Vec<Verb>,
}

/// Глаголы на `World`.
pub trait VerbWorldExt {
    /// Глаголы цели для действующего (`GetLocalVerbs`), по порядку меню.
    fn local_verbs(&mut self, target: Entity, user: Entity, kinds: &[VerbKind]) -> Vec<Verb>;

    /// Выполнить глагол (`ExecuteVerb`).
    fn execute_verb(&mut self, verb: &Verb, user: Entity, target: Entity);
}

impl VerbWorldExt for World {
    fn local_verbs(&mut self, target: Entity, user: Entity, kinds: &[VerbKind]) -> Vec<Verb> {
        let can_access = self.is_accessible(user, target)
            && (user == target || self.in_range_unobstructed(user, target, INTERACTION_RANGE));
        let can_interact = self.can_interact(user, Some(target));
        let can_complex_interact = self.can_complex_interact(user);
        let using = self.used_entity(user);
        let mut verbs: Vec<Verb> = Vec::new();
        for &kind in kinds {
            let on = match kind {
                VerbKind::Utility => match using {
                    Some(using) if using != target => using,
                    _ => continue,
                },
                VerbKind::Innate => user,
                _ => target,
            };
            let mut event = GetVerbs {
                kind,
                user,
                target,
                using,
                can_interact,
                can_complex_interact,
                can_access,
                verbs: Vec::new(),
            };
            if kind == VerbKind::Equipment {
                self.raise(on, &mut event);
            } else {
                self.raise_and_broadcast(on, &mut event);
            }
            // Глагол другого вида, добавленный по ошибке, не попадает в список.
            verbs.extend(event.verbs.into_iter().filter(|verb| verb.kind == kind));
        }
        verbs.sort_by(Verb::order);
        verbs.dedup_by(|a, b| a.key() == b.key());
        verbs
    }

    fn execute_verb(&mut self, verb: &Verb, user: Entity, target: Entity) {
        if verb.disabled {
            return;
        }
        if let Some(act) = &verb.act {
            act(self);
        }
        if !self.is_alive(user) || !self.is_alive(target) {
            return;
        }
        let contact = verb.do_contact_interaction.unwrap_or_else(|| {
            verb.kind.default_contact()
                && self.in_range_unobstructed(user, target, INTERACTION_RANGE)
        });
        if contact && user != target {
            self.raise(
                user,
                &mut crate::interaction::ContactInteraction { other: target },
            );
            self.raise(
                target,
                &mut crate::interaction::ContactInteraction { other: user },
            );
        }
    }
}

/// Альтернативное действие: первый альтернативный глагол (`AltInteract`).
pub fn alt_interact(world: &mut World, user: Entity, target: Entity) -> bool {
    let verbs = world.local_verbs(target, user, &[VerbKind::Alternative]);
    let Some(verb) = verbs.into_iter().find(|verb| !verb.disabled) else {
        return false;
    };
    world.execute_verb(&verb, user, target);
    true
}

/// Чем глагол узнаётся при выполнении.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerbKey {
    pub kind: VerbKind,
    pub text: String,
    pub category: Option<String>,
}

/// Глагол без действия: для меню клиента.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerbData {
    pub key: VerbKey,
    pub priority: i32,
    pub disabled: bool,
    pub message: Option<String>,
    pub icon: Option<String>,
    pub close_menu: bool,
    pub confirmation_popup: bool,
}

/// Сообщения глаголов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerbMessage {
    /// Клиент: глаголы цели (`RequestServerVerbsEvent`).
    Request(NetEntity),
    /// Сервер: глаголы цели (`VerbsResponseEvent`).
    Response {
        target: NetEntity,
        verbs: Vec<VerbData>,
    },
    /// Клиент: выполнить глагол (`ExecuteVerbEvent`).
    Execute { target: NetEntity, verb: VerbKey },
}

/// Последний ответ сервера с глаголами: по нему клиент строит меню. Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct VerbMenuData {
    pub target: Option<Entity>,
    pub verbs: Vec<VerbData>,
    /// Растёт с каждым ответом.
    pub revision: u64,
}

fn data(verb: &Verb) -> VerbData {
    VerbData {
        key: verb.key(),
        priority: verb.priority,
        disabled: verb.disabled,
        message: verb.message.clone(),
        icon: verb.icon.clone(),
        close_menu: verb.close_menu.unwrap_or(verb.kind != VerbKind::Examine),
        confirmation_popup: verb.confirmation_popup,
    }
}

fn receive(world: &mut World, event: &mut NetEvent<VerbMessage>) {
    match (&event.event, event.session) {
        // Сервер.
        (VerbMessage::Request(net), Some(session)) => {
            let Some(user) = world.get::<Session>(session).and_then(|s| s.attached) else {
                return;
            };
            let Some(target) = world
                .get_resource::<NetEntities>()
                .and_then(|all| all.entity(*net))
            else {
                return;
            };
            if !world.is_alive(user) || !world.is_alive(target) {
                return;
            }
            let verbs = world.local_verbs(target, user, &VerbKind::ALL);
            let response = VerbMessage::Response {
                target: *net,
                verbs: verbs.iter().map(data).collect(),
            };
            world.send_net_event(NetTarget::Session(session), &response);
        }
        (VerbMessage::Execute { target, verb }, Some(session)) => {
            let Some(user) = world.get::<Session>(session).and_then(|s| s.attached) else {
                return;
            };
            let Some(target) = world
                .get_resource::<NetEntities>()
                .and_then(|all| all.entity(*target))
            else {
                return;
            };
            if !world.is_alive(user) || !world.is_alive(target) {
                return;
            }
            let found = world
                .local_verbs(target, user, &[verb.kind])
                .into_iter()
                .find(|candidate| candidate.key() == *verb);
            if let Some(found) = found {
                world.execute_verb(&found, user, target);
            }
        }
        // Клиент.
        (VerbMessage::Response { target, verbs }, None) => {
            let target = world
                .get_resource::<NetEntities>()
                .and_then(|all| all.entity(*target));
            let mut menu = world.get_resource_or_insert_with(VerbMenuData::default);
            menu.target = target;
            menu.verbs = verbs.clone();
            menu.revision += 1;
        }
        _ => {}
    }
}

pub(crate) fn install(world: &mut World) {
    world.init_resource::<VerbMenuData>();
    world.register_net_event::<VerbMessage>();
    world.subscribe_net_event::<VerbMessage>(receive);
}

#[cfg(test)]
mod tests {
    use bevy_ecs::resource::Resource;

    use super::*;
    use crate::item::Item;
    use crate::testing::{spawn_at, station, world_with};

    const PROTOTYPES: &str = "
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Crowbar
  components:
  - type: Item
  - type: Physics
    bodyType: Dynamic
";

    #[derive(Resource, Default)]
    struct Log(Vec<String>);

    #[test]
    fn verbs_are_gathered_sorted_and_executed_by_key() {
        let mut world = world_with(PROTOTYPES);
        world.init_resource::<Log>();
        let grid = station(&mut world);
        let user = spawn_at(&mut world, "Human", grid, 2, 2);
        let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
        world.subscribe::<Item, GetVerbs>(|_, _, event| {
            if event.kind == VerbKind::Alternative {
                event
                    .verbs
                    .push(Verb::new(VerbKind::Alternative, "Крутить", |world| {
                        world.resource_mut::<Log>().0.push("alt".into());
                    }));
            }
            if event.kind == VerbKind::Other {
                let mut verb = Verb::new(VerbKind::Other, "Б", |_| {});
                verb.priority = 1;
                event.verbs.push(verb);
                event.verbs.push(Verb::new(VerbKind::Other, "А", |_| {}));
            }
        });
        let verbs = world.local_verbs(crowbar, user, &VerbKind::ALL);
        let texts: Vec<&str> = verbs.iter().map(|verb| verb.text.as_str()).collect();
        // Подбор (глагол взаимодействия) выше альтернативного, затем прочие по приоритету.
        // Без загруженного языка подпись — ключ сообщения.
        assert_eq!(texts[0], "pick-up-verb-get-data-text");
        assert_eq!(&texts[1..], ["Крутить", "Б", "А"]);

        assert!(alt_interact(&mut world, user, crowbar));
        assert_eq!(world.resource::<Log>().0, ["alt"]);
        let pick_up = verbs[0].clone();
        world.execute_verb(&pick_up, user, crowbar);
        assert_eq!(
            crate::hands::HandsWorldExt::active_item(&world, user),
            Some(crowbar)
        );
    }
}
