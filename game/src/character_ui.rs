//! Экран создания персонажа на клиенте (F7): слоты персонажей с выбором, созданием и
//! удалением; имя, возраст, пол, местоимения, вид, голос, место появления; вкладки
//! внешности (кожа ползунком тона или цветом, глаза), меток по органам и слоям с поиском,
//! пределами и цветами слоёв, должностей по отделам с приоритетами и антагонистов.
//! Сохранение проверяет профиль на месте и шлёт его серверу; несохранённые правки при
//! закрытии предлагается сохранить. Образец — `CharacterSetupGui`, `HumanoidProfileEditor`,
//! `MarkingPicker`, `OrganMarkingPicker`, `CharacterSetupGuiSavePanel` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: куклы предпросмотра, черт, снаряжения, описания внешности, требований
//! должностей по наигранному времени, замков случайного выбора, импорта и экспорта.

use std::collections::BTreeSet;

use astrocore_client::{Game, KeyboardFocus, UiAction, UiStyle};
use astrocore_loc::{Loc, LocArg};
use astrocore_math::Color as GameColor;
use astrocore_prototypes::Prototypes;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::body::Sex;
use crate::character_editor::{
    self as edit, DepartmentEntry, PreviousColors, departments, hex, layer_choices, organs,
    parse_hex, selected, skin_strategy, skin_unary,
};
use crate::dev_console::DevConsole;
use crate::examine::loc;
use crate::preferences::{
    ClientPreferences, client_create_character, client_delete_character, client_select_character,
    client_update_character,
};
use crate::profiles::{
    FALLBACK_OVERFLOW_JOB, Gender, HumanoidCharacterProfile, JobPriority,
    PreferenceUnavailableMode, SpawnPriorityPreference, random_name, random_profile,
    round_start_species, species_info,
};

const PANEL: Color = Color::srgba(0.07, 0.08, 0.10, 0.96);
const SECTION: Color = Color::srgba(0.10, 0.11, 0.14, 0.95);
const SELECTED: Color = Color::srgb(0.30, 0.45, 0.75);
const ROW: Color = Color::srgba(0.15, 0.16, 0.20, 0.95);
const DANGER: Color = Color::srgb(0.60, 0.20, 0.20);
const BORDER: Color = Color::srgb(0.25, 0.27, 0.32);
const WHITE: Color = Color::WHITE;
/// Высота колонок должностей.
const JOBS_HEIGHT: f32 = 500.0;
/// Меток на странице списка.
const MARKINGS_PER_PAGE: usize = 24;

/// Вкладка редактора.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Appearance,
    Markings,
    Jobs,
    Antags,
}

/// Что красит палитра.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ColorTarget {
    #[default]
    Skin,
    Eyes,
    Marking {
        organ: String,
        layer: String,
        marking: String,
        index: usize,
    },
}

/// Поле ввода с клавиатуры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Age,
    Search,
    Hex,
}

/// Состояние редактора. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct CharacterEditor {
    pub open: bool,
    /// Слот правимого персонажа.
    pub slot: Option<i32>,
    pub profile: Option<HumanoidCharacterProfile>,
    /// Есть несохранённые правки (`IsDirty`).
    pub dirty: bool,
    /// Спросить о сохранении перед закрытием.
    pub confirm_close: bool,
    /// Слот, удаление которого ждёт подтверждения.
    pub confirm_delete: Option<i32>,
    pub tab: Tab,
    pub organ: Option<String>,
    pub layer: Option<String>,
    pub page: usize,
    pub search: String,
    pub color: ColorTarget,
    pub typing: Option<Field>,
    /// Текст поля цвета.
    pub hex: String,
    pub previous: PreviousColors,
    pub version: u64,
}

impl CharacterEditor {
    fn changed(&mut self) {
        self.version += 1;
    }
}

fn editor(world: &mut GameWorld) -> Mut<'_, CharacterEditor> {
    world.get_resource_or_insert_with(CharacterEditor::default)
}

fn stored(world: &GameWorld, slot: i32) -> Option<HumanoidCharacterProfile> {
    world
        .get_resource::<ClientPreferences>()?
        .preferences
        .as_ref()?
        .characters
        .get(&slot)
        .cloned()
}

/// Открыть персонажа слота (`SetProfile`): правки сбрасываются; у другого слота — и
/// выбор вкладок меток и цвета.
pub fn load_slot(world: &mut GameWorld, slot: i32) {
    let profile = stored(world, slot);
    let mut state = editor(world);
    let same = state.slot == Some(slot) && profile.is_some();
    state.slot = profile.is_some().then_some(slot);
    state.profile = profile;
    state.dirty = false;
    state.confirm_delete = None;
    state.hex.clear();
    if !same {
        state.previous.clear();
        state.organ = None;
        state.layer = None;
        state.page = 0;
        state.color = ColorTarget::Skin;
        state.typing = None;
    }
    state.changed();
}

/// Открыть выбранного персонажа или первого из есть.
fn load_selected(world: &mut GameWorld) {
    let Some(prefs) = world
        .get_resource::<ClientPreferences>()
        .and_then(|p| p.preferences.clone())
    else {
        return;
    };
    let slot = if prefs
        .characters
        .contains_key(&prefs.selected_character_index)
    {
        Some(prefs.selected_character_index)
    } else {
        prefs.characters.keys().next().copied()
    };
    if let Some(slot) = slot {
        load_slot(world, slot);
    }
}

/// Сохранить правки (`Save`): профиль проверяется и уходит серверу.
pub fn save(world: &mut GameWorld) {
    let (Some(slot), Some(profile)) = ({
        let state = editor(world);
        (state.slot, state.profile.clone())
    }) else {
        return;
    };
    client_update_character(world, &profile, slot);
    let saved = stored(world, slot);
    let mut state = editor(world);
    if saved.is_some() {
        state.profile = saved;
    }
    state.dirty = false;
    state.changed();
}

/// Правка профиля с прототипами под рукой.
fn change(
    world: &mut GameWorld,
    f: impl FnOnce(&Prototypes, &mut HumanoidCharacterProfile, &mut CharacterEditor),
) {
    if !world.contains_resource::<Prototypes>() {
        return;
    }
    world.resource_scope(|world, prototypes: Mut<Prototypes>| {
        let mut state = editor(world);
        let state = &mut *state;
        let Some(mut profile) = state.profile.take() else {
            return;
        };
        f(&prototypes, &mut profile, state);
        state.profile = Some(profile);
        state.dirty = true;
        state.changed();
    });
}

/// Случайная внешность и имя (`RandomizeProfile`); должности и роли остаются.
pub fn randomize(world: &mut GameWorld) {
    if editor(world).profile.is_none() {
        return;
    }
    let random = random_profile(world, &BTreeSet::new());
    change(world, |_, profile, state| {
        *profile = HumanoidCharacterProfile {
            job_priorities: std::mem::take(&mut profile.job_priorities),
            preference_unavailable: profile.preference_unavailable,
            spawn_priority: profile.spawn_priority,
            antag_preferences: std::mem::take(&mut profile.antag_preferences),
            trait_preferences: std::mem::take(&mut profile.trait_preferences),
            loadouts: std::mem::take(&mut profile.loadouts),
            ..random
        };
        state.previous.clear();
        state.color = ColorTarget::Skin;
        state.hex.clear();
    });
}

/// Новый случайный персонаж в свободном слоте (`NewCharacterButton`).
pub fn new_character(world: &mut GameWorld) {
    let profile = random_profile(world, &BTreeSet::new());
    if let Some(slot) = client_create_character(world, &profile) {
        load_slot(world, slot);
    }
}

/// Выбрать персонажа для игры и открыть его.
pub fn select(world: &mut GameWorld, slot: i32) {
    client_select_character(world, slot);
    load_slot(world, slot);
}

/// Удалить персонажа со второго нажатия (`ConfirmDeleteButton`).
pub fn delete(world: &mut GameWorld, slot: i32) {
    if editor(world).confirm_delete != Some(slot) {
        let mut state = editor(world);
        state.confirm_delete = Some(slot);
        state.changed();
        return;
    }
    let editing = editor(world).slot == Some(slot);
    client_delete_character(world, slot);
    if editing {
        load_selected(world);
    }
    let mut state = editor(world);
    state.confirm_delete = None;
    state.changed();
}

/// Закрыть; при несохранённых правках сначала спросить (`CharacterSetupGuiSavePanel`).
pub fn close(world: &mut GameWorld) {
    let mut state = editor(world);
    if state.dirty && !state.confirm_close {
        state.confirm_close = true;
    } else {
        state.open = false;
        state.confirm_close = false;
        state.typing = None;
    }
    state.changed();
}

/// Ответ на вопрос о сохранении: сохранить, не сохранять или вернуться.
pub fn answer_close(world: &mut GameWorld, save_changes: Option<bool>) {
    match save_changes {
        Some(true) => save(world),
        Some(false) => {
            if let Some(slot) = editor(world).slot {
                load_slot(world, slot);
            }
        }
        None => {}
    }
    let mut state = editor(world);
    state.confirm_close = false;
    if save_changes.is_some() {
        state.open = false;
        state.typing = None;
    }
    state.changed();
}

/// Цвет палитры — в цель: кожу, глаза или слой метки.
pub fn apply_color(world: &mut GameWorld, color: GameColor) {
    change(world, |prototypes, profile, state| {
        match state.color.clone() {
            ColorTarget::Skin => edit::set_skin(prototypes, profile, None, color),
            ColorTarget::Eyes => edit::set_eye_color(profile, color),
            ColorTarget::Marking {
                organ,
                layer,
                marking,
                index,
            } => {
                edit::set_marking_color(prototypes, profile, &organ, &layer, &marking, index, color)
            }
        }
        state.hex = hex(target_color(profile, &state.color).unwrap_or(color));
    });
}

/// Нынешний цвет цели.
pub fn target_color(profile: &HumanoidCharacterProfile, target: &ColorTarget) -> Option<GameColor> {
    match target {
        ColorTarget::Skin => Some(profile.appearance.skin_color),
        ColorTarget::Eyes => Some(profile.appearance.eye_color),
        ColorTarget::Marking {
            organ,
            layer,
            marking,
            index,
        } => selected(profile, organ, layer)
            .iter()
            .find(|m| &m.marking_id == marking)?
            .marking_colors
            .get(*index)
            .copied(),
    }
}

fn set_target(world: &mut GameWorld, target: ColorTarget) {
    let mut state = editor(world);
    let color = state
        .profile
        .as_ref()
        .and_then(|p| target_color(p, &target));
    state.hex = color.map(hex).unwrap_or_default();
    state.color = target;
    state.changed();
}

/// Ввод в поле: имя и поиск — сразу, возраст — цифрами, цвет — по Enter.
pub fn type_text(world: &mut GameWorld, field: Field, text: &str, backspace: bool) {
    let edit_text = |value: &mut String| {
        if backspace {
            value.pop();
        } else {
            value.push_str(text);
        }
    };
    match field {
        Field::Name => change(world, |_, profile, _| edit_text(&mut profile.name)),
        Field::Age => {
            if !backspace && !text.chars().all(|c| c.is_ascii_digit()) {
                return;
            }
            change(world, |_, profile, _| {
                let mut value = profile.age.to_string();
                edit_text(&mut value);
                value.truncate(3);
                profile.age = value.parse().unwrap_or(0);
            });
        }
        Field::Search => {
            let mut state = editor(world);
            edit_text(&mut state.search);
            state.page = 0;
            state.changed();
        }
        Field::Hex => {
            let mut state = editor(world);
            edit_text(&mut state.hex);
            state.changed();
        }
    }
}

fn finish_typing(world: &mut GameWorld) {
    let (field, text) = {
        let state = editor(world);
        (state.typing, state.hex.clone())
    };
    if field == Some(Field::Hex)
        && let Some(color) = parse_hex(&text)
    {
        apply_color(world, color);
    }
    let mut state = editor(world);
    state.typing = None;
    state.changed();
}

/// Открыть или закрыть редактор (F7).
pub fn toggle(world: &mut GameWorld) {
    if editor(world).open {
        close(world);
        return;
    }
    let mut state = editor(world);
    state.open = true;
    state.changed();
    if editor(world).profile.is_none() {
        load_selected(world);
    }
}

fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    console: Res<DevConsole>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let was_typing = editor(world).typing.is_some();
    for event in events.read() {
        if event.state != ButtonState::Pressed || console.open {
            continue;
        }
        if let Some(field) = editor(world).typing {
            match &event.logical_key {
                Key::Escape | Key::Enter => finish_typing(world),
                Key::Backspace => type_text(world, field, "", true),
                Key::Space => type_text(world, field, " ", false),
                Key::Character(text) => {
                    let text: String = text
                        .chars()
                        .filter(|c| !c.is_control() && *c != '`')
                        .collect();
                    type_text(world, field, &text, false);
                }
                _ => {}
            }
            continue;
        }
        if focus.0 {
            continue;
        }
        match event.key_code {
            KeyCode::F7 => toggle(world),
            KeyCode::Escape if editor(world).open => close(world),
            _ => {}
        }
    }
    let typing = editor(world).typing.is_some();
    if typing != was_typing {
        focus.0 = typing;
    }
}

// Окно.

#[derive(Component)]
struct EditorPanel;

fn text_or(world: &GameWorld, key: &str, fallback: &str) -> String {
    match world.get_resource::<Loc>() {
        Some(l) if l.has(key) => l.format(key, &[]),
        _ => fallback.to_owned(),
    }
}

fn to_ui(color: GameColor) -> Color {
    Color::srgb(color.r, color.g, color.b)
}

struct Ui<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    style: &'a UiStyle,
}

impl Ui<'_, '_, '_> {
    fn node(&mut self, parent: Entity, node: Node) -> Entity {
        self.commands.spawn((ChildOf(parent), node)).id()
    }

    fn row(&mut self, parent: Entity) -> Entity {
        self.node(
            parent,
            Node {
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                column_gap: Val::Px(4.0),
                row_gap: Val::Px(2.0),
                ..default()
            },
        )
    }

    fn column(&mut self, parent: Entity, width: Option<f32>) -> Entity {
        self.node(
            parent,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                width: width.map_or(Val::Auto, Val::Px),
                ..default()
            },
        )
    }

    fn label(&mut self, parent: Entity, text: impl Into<String>, size: f32) {
        self.commands
            .spawn((ChildOf(parent), self.style.text(text, size)));
    }

    fn chip_colored(
        &mut self,
        parent: Entity,
        text: impl Into<String>,
        background: Color,
        action: UiAction,
    ) -> Entity {
        let button = self
            .commands
            .spawn((
                ChildOf(parent),
                Interaction::default(),
                Node {
                    padding: UiRect::axes(Val::Px(5.0), Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(background),
                action,
            ))
            .id();
        self.commands
            .spawn((ChildOf(button), self.style.text(text, 12.0)));
        button
    }

    fn chip(
        &mut self,
        parent: Entity,
        text: impl Into<String>,
        selected: bool,
        action: UiAction,
    ) -> Entity {
        self.chip_colored(parent, text, if selected { SELECTED } else { ROW }, action)
    }

    fn swatch(&mut self, parent: Entity, color: GameColor, selected: bool, action: UiAction) {
        self.commands.spawn((
            ChildOf(parent),
            Interaction::default(),
            Node {
                width: Val::Px(16.0),
                height: Val::Px(16.0),
                border: UiRect::all(Val::Px(if selected { 2.0 } else { 1.0 })),
                ..default()
            },
            BackgroundColor(to_ui(color)),
            BorderColor::all(if selected { WHITE } else { BORDER }),
            action,
        ));
    }

    fn field(
        &mut self,
        parent: Entity,
        label: &str,
        value: &str,
        field: Field,
        typing: Option<Field>,
    ) {
        let cursor = if typing == Some(field) { "_" } else { "" };
        self.chip(
            parent,
            format!("{label} {value}{cursor}"),
            typing == Some(field),
            UiAction::new(move |world| {
                let mut state = editor(world);
                state.typing = if state.typing == Some(field) {
                    None
                } else {
                    Some(field)
                };
                state.changed();
            }),
        );
    }
}

fn window(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<EditorPanel>>,
    mut shown: Local<Option<u64>>,
    mut seen: Local<Option<ClientPreferences>>,
) {
    let world = &mut game.0;
    let Some(open) = world.get_resource::<CharacterEditor>().map(|s| s.open) else {
        return;
    };
    // Настройки с сервера: открыть выбранного, если ещё нечего править или правок нет.
    if open && seen.as_ref() != world.get_resource::<ClientPreferences>() {
        *seen = world.get_resource::<ClientPreferences>().cloned();
        let (slot, dirty) = {
            let state = editor(world);
            (state.slot, state.dirty)
        };
        match slot {
            // Правки не теряются: перерисовать только список слотов.
            Some(_) if dirty => editor(world).changed(),
            Some(slot) if stored(world, slot).is_some() => load_slot(world, slot),
            _ => load_selected(world),
        }
    }
    let state = editor(world).clone();
    if *shown == Some(state.version) {
        return;
    }
    *shown = Some(state.version);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !state.open {
        return;
    }
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                left: Val::Px(8.0),
                width: Val::Px(1264.0),
                max_height: Val::Px(704.0),
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            Interaction::default(),
            GlobalZIndex(60),
            EditorPanel,
        ))
        .id();
    let mut ui = Ui {
        commands: &mut commands,
        style: &style,
    };
    slots(world, &mut ui, panel, &state);
    let main = ui.column(panel, Some(1020.0));
    let Some(profile) = state.profile.clone() else {
        ui.label(main, "Настройки персонажей ещё не пришли с сервера.", 13.0);
        return;
    };
    if !world.contains_resource::<Prototypes>() {
        return;
    }
    world.resource_scope(|world, prototypes: Mut<Prototypes>| {
        basic_info(world, &prototypes, &mut ui, main, &state, &profile);
        let tabs = ui.row(main);
        for (tab, key, fallback) in [
            (
                Tab::Appearance,
                "humanoid-profile-editor-appearance-tab",
                "Внешность",
            ),
            (
                Tab::Markings,
                "humanoid-profile-editor-markings-tab",
                "Метки",
            ),
            (Tab::Jobs, "humanoid-profile-editor-jobs-tab", "Должности"),
            (
                Tab::Antags,
                "humanoid-profile-editor-antags-tab",
                "Антагонисты",
            ),
        ] {
            ui.chip(
                tabs,
                text_or(world, key, fallback),
                state.tab == tab,
                UiAction::new(move |world| {
                    let mut state = editor(world);
                    state.tab = tab;
                    state.changed();
                }),
            );
        }
        let content = ui.node(
            main,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
        );
        ui.commands.entity(content).insert(BackgroundColor(SECTION));
        match state.tab {
            Tab::Appearance => appearance(world, &prototypes, &mut ui, content, &state, &profile),
            Tab::Markings => markings(world, &prototypes, &mut ui, content, &state, &profile),
            Tab::Jobs => jobs(world, &prototypes, &mut ui, content, &profile),
            Tab::Antags => antags(world, &prototypes, &mut ui, content, &profile),
        }
        if matches!(state.tab, Tab::Appearance | Tab::Markings) {
            palette(world, &prototypes, &mut ui, content, &state, &profile);
        }
    });
}

/// Слоты персонажей (`CharacterSetupGui`) и кнопки сохранения.
fn slots(world: &GameWorld, ui: &mut Ui, panel: Entity, state: &CharacterEditor) {
    let column = ui.column(panel, Some(220.0));
    ui.label(
        column,
        format!(
            "{} (F7)",
            text_or(
                world,
                "character-setup-gui-character-setup-label",
                "Настройка персонажа"
            )
        ),
        15.0,
    );
    let prefs = world
        .get_resource::<ClientPreferences>()
        .cloned()
        .unwrap_or_default();
    let Some(preferences) = prefs.preferences else {
        return;
    };
    for (slot, character) in &preferences.characters {
        let slot = *slot;
        let row = ui.row(column);
        let mark = if slot == preferences.selected_character_index {
            "● "
        } else {
            ""
        };
        ui.chip(
            row,
            format!("{mark}{}", character.name),
            state.slot == Some(slot),
            UiAction::new(move |world| select(world, slot)),
        );
        let confirm = state.confirm_delete == Some(slot);
        let key = if confirm {
            "character-setup-gui-character-picker-button-confirm-delete-button"
        } else {
            "character-setup-gui-character-picker-button-delete-button"
        };
        // Последнего персонажа не удалить: сервер откажет.
        if preferences.characters.len() > 1 {
            ui.chip_colored(
                row,
                text_or(
                    world,
                    key,
                    if confirm {
                        "Точно?"
                    } else {
                        "Удалить"
                    },
                ),
                if confirm { DANGER } else { ROW },
                UiAction::new(move |world| delete(world, slot)),
            );
        }
    }
    if (preferences.characters.len() as i32) < prefs.max_character_slots {
        ui.chip(
            column,
            text_or(
                world,
                "character-setup-gui-create-new-character-button",
                "Новый персонаж…",
            ),
            false,
            UiAction::new(new_character),
        );
    }
    ui.label(column, "", 6.0);
    let dirty = if state.dirty { " *" } else { "" };
    let save_text = text_or(world, "humanoid-profile-editor-save-button", "Сохранить");
    ui.chip(
        column,
        format!("{save_text}{dirty}"),
        state.dirty,
        UiAction::new(save),
    );
    ui.chip(
        column,
        text_or(world, "humanoid-profile-editor-reset-button", "Сбросить"),
        false,
        UiAction::new(|world| {
            if let Some(slot) = editor(world).slot {
                load_slot(world, slot);
            }
        }),
    );
    ui.chip(
        column,
        text_or(world, "humanoid-profile-editor-randomize", "Случайно"),
        false,
        UiAction::new(randomize),
    );
    ui.chip(
        column,
        text_or(
            world,
            "character-setup-gui-character-setup-close-button",
            "Закрыть",
        ),
        false,
        UiAction::new(close),
    );
    if state.confirm_close {
        ui.label(
            column,
            text_or(
                world,
                "character-setup-gui-save-panel-title",
                "Есть несохранённые правки",
            ),
            13.0,
        );
        let row = ui.row(column);
        for (key, fallback, answer) in [
            (
                "character-setup-gui-save-panel-save",
                "Сохранить",
                Some(true),
            ),
            (
                "character-setup-gui-save-panel-nosave",
                "Не сохранять",
                Some(false),
            ),
            ("character-setup-gui-save-panel-cancel", "Отмена", None),
        ] {
            ui.chip(
                row,
                text_or(world, key, fallback),
                false,
                UiAction::new(move |world| answer_close(world, answer)),
            );
        }
    }
}

fn sex_key(sex: Sex) -> &'static str {
    match sex {
        Sex::Male => "humanoid-profile-editor-sex-male-text",
        Sex::Female => "humanoid-profile-editor-sex-female-text",
        Sex::Unsexed => "humanoid-profile-editor-sex-unsexed-text",
    }
}

fn gender_key(gender: Gender) -> &'static str {
    match gender {
        Gender::Male => "humanoid-profile-editor-pronouns-male-text",
        Gender::Female => "humanoid-profile-editor-pronouns-female-text",
        Gender::Epicene => "humanoid-profile-editor-pronouns-epicene-text",
        Gender::Neuter => "humanoid-profile-editor-pronouns-neuter-text",
    }
}

fn species_name(world: &GameWorld, prototypes: &Prototypes, id: &str) -> String {
    let key = prototypes
        .resolved_node("species", id)
        .and_then(|n| n.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or(id)
        .to_owned();
    text_or(world, &key, id)
}

/// Имя, возраст, пол, местоимения, вид, голос, место появления.
fn basic_info(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    main: Entity,
    state: &CharacterEditor,
    profile: &HumanoidCharacterProfile,
) {
    let row = ui.row(main);
    let name = text_or(world, "humanoid-profile-editor-name-label", "Имя:");
    ui.field(row, &name, &profile.name, Field::Name, state.typing);
    ui.chip(
        row,
        text_or(
            world,
            "humanoid-profile-editor-randomize-name-button",
            "Имя",
        ),
        false,
        UiAction::new(|world| {
            let Some((species, gender)) = editor(world)
                .profile
                .as_ref()
                .map(|p| (p.species.clone(), p.gender))
            else {
                return;
            };
            let name = random_name(world, &species, Some(gender));
            change(world, |_, profile, _| profile.name = name);
        }),
    );
    let age = text_or(world, "humanoid-profile-editor-age-label", "Возраст:");
    ui.field(
        row,
        &age,
        &profile.age.to_string(),
        Field::Age,
        state.typing,
    );

    let info = species_info(prototypes, &profile.species);
    let row = ui.row(main);
    ui.label(
        row,
        text_or(world, "humanoid-profile-editor-sex-label", "Пол:"),
        12.0,
    );
    let sexes = info
        .as_ref()
        .map_or_else(|| vec![Sex::Unsexed], |i| i.sexes.clone());
    for sex in sexes {
        ui.chip(
            row,
            text_or(world, sex_key(sex), crate::profiles::sex_name(sex)),
            profile.sex == sex,
            UiAction::new(move |world| {
                change(world, |p, profile, _| edit::set_sex(p, profile, sex))
            }),
        );
    }
    ui.label(
        row,
        text_or(
            world,
            "humanoid-profile-editor-pronouns-label",
            "Местоимения:",
        ),
        12.0,
    );
    for gender in [
        Gender::Male,
        Gender::Female,
        Gender::Epicene,
        Gender::Neuter,
    ] {
        ui.chip(
            row,
            text_or(world, gender_key(gender), gender.name()),
            profile.gender == gender,
            UiAction::new(move |world| change(world, |_, profile, _| profile.gender = gender)),
        );
    }

    let row = ui.row(main);
    ui.label(
        row,
        text_or(world, "humanoid-profile-editor-species-label", "Вид:"),
        12.0,
    );
    let mut species: Vec<(String, String)> = round_start_species(prototypes)
        .into_iter()
        .map(|id| (species_name(world, prototypes, &id), id))
        .collect();
    species.sort_by_key(|(name, _)| name.to_lowercase());
    for (name, id) in species {
        let chosen = profile.species == id;
        ui.chip(
            row,
            name,
            chosen,
            UiAction::new(move |world| {
                change(world, |p, profile, state| {
                    edit::set_species(p, profile, &id);
                    state.organ = None;
                    state.layer = None;
                    state.page = 0;
                    state.color = ColorTarget::Skin;
                })
            }),
        );
    }

    let row = ui.row(main);
    ui.label(
        row,
        text_or(world, "humanoid-profile-editor-voice-label", "Голос:"),
        12.0,
    );
    for voice in info.iter().flat_map(|i| i.voices.iter().cloned()) {
        let key = edit::voice_name(prototypes, &voice);
        ui.chip(
            row,
            text_or(world, &key, &voice),
            profile.voice == voice,
            UiAction::new(move |world| {
                let voice = voice.clone();
                change(world, |_, profile, _| profile.voice = voice);
            }),
        );
    }
    ui.label(
        row,
        text_or(
            world,
            "humanoid-profile-editor-spawn-priority-label",
            "Появление:",
        ),
        12.0,
    );
    for (priority, key) in [
        (
            SpawnPriorityPreference::None,
            "humanoid-profile-editor-preference-spawn-priority-none",
        ),
        (
            SpawnPriorityPreference::Arrivals,
            "humanoid-profile-editor-preference-spawn-priority-arrivals",
        ),
        (
            SpawnPriorityPreference::Cryosleep,
            "humanoid-profile-editor-preference-spawn-priority-cryosleep",
        ),
    ] {
        ui.chip(
            row,
            text_or(world, key, key),
            profile.spawn_priority == priority,
            UiAction::new(move |world| {
                change(world, |_, profile, _| profile.spawn_priority = priority)
            }),
        );
    }
}

/// Кожа и глаза (`UpdateSkinColor`, `UpdateEyePickers`).
fn appearance(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    content: Entity,
    state: &CharacterEditor,
    profile: &HumanoidCharacterProfile,
) {
    let row = ui.row(content);
    ui.label(
        row,
        text_or(world, "humanoid-profile-editor-skin-color-label", "Кожа:"),
        12.0,
    );
    ui.swatch(
        row,
        profile.appearance.skin_color,
        state.color == ColorTarget::Skin,
        UiAction::new(|world| set_target(world, ColorTarget::Skin)),
    );
    // Числовая окраска — ползунок тона ступенями по 5.
    if let Some(current) = skin_unary(prototypes, profile) {
        let strategy = skin_strategy(prototypes, &profile.species);
        for step in 0..=20 {
            let value = step as f32 * 5.0;
            let color = strategy
                .as_ref()
                .map_or(GameColor::WHITE, |s| s.from_unary(value));
            ui.swatch(
                row,
                color,
                (current - value).abs() < 2.5,
                UiAction::new(move |world| {
                    change(world, |p, profile, _| {
                        edit::set_skin(p, profile, Some(value), GameColor::WHITE)
                    })
                }),
            );
        }
    }
    let row = ui.row(content);
    ui.label(
        row,
        text_or(world, "humanoid-profile-editor-eyes-label", "Глаза:"),
        12.0,
    );
    ui.swatch(
        row,
        profile.appearance.eye_color,
        state.color == ColorTarget::Eyes,
        UiAction::new(|world| set_target(world, ColorTarget::Eyes)),
    );
    let unavailable = ui.row(content);
    let overflow = prototypes
        .resolved_node("job", FALLBACK_OVERFLOW_JOB)
        .and_then(|n| n.get("name"))
        .and_then(|n| n.as_str())
        .map(|key| loc(world, key, &[]))
        .unwrap_or_else(|| FALLBACK_OVERFLOW_JOB.to_owned());
    for (mode, key) in [
        (
            PreferenceUnavailableMode::SpawnAsOverflow,
            "humanoid-profile-editor-preference-unavailable-spawn-as-overflow-button",
        ),
        (
            PreferenceUnavailableMode::StayInLobby,
            "humanoid-profile-editor-preference-unavailable-stay-in-lobby-button",
        ),
    ] {
        let text = loc(
            world,
            key,
            &[("overflowJob", LocArg::from(overflow.as_str()))],
        );
        ui.chip(
            unavailable,
            text,
            profile.preference_unavailable == mode,
            UiAction::new(move |world| {
                change(world, |_, profile, _| profile.preference_unavailable = mode)
            }),
        );
    }
}

fn marking_name(world: &GameWorld, id: &str) -> String {
    text_or(world, &format!("marking-{id}"), id)
}

fn layer_title(world: &GameWorld, layer: &str, group: &str) -> String {
    let special = format!("markings-layer-{layer}-{group}");
    match world.get_resource::<Loc>() {
        Some(l) if l.has(&special) => l.format(&special, &[]),
        _ => text_or(world, &format!("markings-layer-{layer}"), layer),
    }
}

/// Метки по органам и слоям (`MarkingPicker`, `OrganMarkingPicker`).
fn markings(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    content: Entity,
    state: &CharacterEditor,
    profile: &HumanoidCharacterProfile,
) {
    let organs = organs(prototypes, &profile.species);
    if organs.is_empty() {
        ui.label(content, "У вида нет органов с метками.", 12.0);
        return;
    }
    let organ = state
        .organ
        .clone()
        .filter(|o| organs.contains_key(o))
        .or_else(|| organs.keys().next().cloned())
        .unwrap_or_default();
    let row = ui.row(content);
    for id in organs.keys() {
        let id = id.clone();
        ui.chip(
            row,
            text_or(world, &format!("markings-organ-{id}"), &id),
            id == organ,
            UiAction::new(move |world| {
                let mut state = editor(world);
                state.organ = Some(id.clone());
                state.layer = None;
                state.page = 0;
                state.changed();
            }),
        );
    }
    let choices = layer_choices(prototypes, profile, &organ);
    let Some(choice) = choices
        .iter()
        .find(|c| Some(&c.layer) == state.layer.as_ref())
        .or_else(|| choices.first())
    else {
        return;
    };
    let row = ui.row(content);
    for layer in &choices {
        let id = layer.layer.clone();
        let count = selected(profile, &organ, &id).len();
        let title = layer_title(world, &id, &layer.group);
        let title = if count > 0 {
            format!("{title} ({count})")
        } else {
            title
        };
        ui.chip(
            row,
            title,
            id == choice.layer,
            UiAction::new(move |world| {
                let mut state = editor(world);
                state.layer = Some(id.clone());
                state.page = 0;
                state.changed();
            }),
        );
    }
    let current = selected(profile, &organ, &choice.layer);
    let limit = choice.limit.unwrap_or(-1);
    let selectable = if limit < 0 {
        -1
    } else {
        (limit - current.len() as i32).max(0)
    };
    ui.label(
        content,
        loc(
            world,
            "markings-limits",
            &[
                (
                    "required",
                    LocArg::from(if choice.required { "true" } else { "false" }),
                ),
                ("count", LocArg::from(limit)),
                ("selectable", LocArg::from(selectable)),
            ],
        ),
        12.0,
    );
    let row = ui.row(content);
    let search = text_or(world, "markings-search", "Поиск");
    ui.field(row, &search, &state.search, Field::Search, state.typing);
    let needle = state.search.to_lowercase();
    let found: Vec<(String, String)> = choice
        .available
        .iter()
        .map(|m| (m.id.clone(), marking_name(world, &m.id)))
        .filter(|(id, name)| {
            needle.is_empty()
                || name.to_lowercase().contains(&needle)
                || id.to_lowercase().contains(&needle)
        })
        .collect();
    let pages = found.len().div_ceil(MARKINGS_PER_PAGE).max(1);
    let page = state.page.min(pages - 1);
    if pages > 1 {
        ui.chip(
            row,
            "<",
            false,
            UiAction::new(|world| {
                let mut state = editor(world);
                state.page = state.page.saturating_sub(1);
                state.changed();
            }),
        );
        ui.label(row, format!("{}/{pages}", page + 1), 12.0);
        ui.chip(
            row,
            ">",
            false,
            UiAction::new(move |world| {
                let mut state = editor(world);
                state.page = (state.page + 1).min(pages - 1);
                state.changed();
            }),
        );
    }
    let list = ui.row(content);
    for (id, name) in found
        .into_iter()
        .skip(page * MARKINGS_PER_PAGE)
        .take(MARKINGS_PER_PAGE)
    {
        let on = current.iter().any(|m| m.marking_id == id);
        let (organ, layer) = (organ.clone(), choice.layer.clone());
        ui.chip(
            list,
            name,
            on,
            UiAction::new(move |world| {
                change(world, |p, profile, state| {
                    if on {
                        edit::deselect_marking(
                            p,
                            profile,
                            &organ,
                            &layer,
                            &id,
                            &mut state.previous,
                        );
                    } else {
                        edit::select_marking(p, profile, &organ, &layer, &id, &state.previous);
                    }
                })
            }),
        );
    }
    // Выбранные метки слоя и цвета их слоёв.
    for marking in current {
        let row = ui.row(content);
        ui.label(row, marking_name(world, &marking.marking_id), 12.0);
        if !edit::color_customizable(
            prototypes,
            profile,
            &organ,
            &choice.layer,
            &marking.marking_id,
        ) {
            ui.label(row, "(цвет задан)", 11.0);
            continue;
        }
        for (index, color) in marking.marking_colors.iter().enumerate() {
            let target = ColorTarget::Marking {
                organ: organ.clone(),
                layer: choice.layer.clone(),
                marking: marking.marking_id.clone(),
                index,
            };
            let chosen = state.color == target;
            ui.swatch(
                row,
                *color,
                chosen,
                UiAction::new(move |world| set_target(world, target.clone())),
            );
        }
    }
}

/// Цвета палитры: оттенки по кругу в четыре яркости и серые.
pub fn palette_colors() -> Vec<GameColor> {
    let mut colors = Vec::new();
    for (saturation, value) in [(0.85, 0.95), (0.6, 0.75), (0.55, 0.5), (0.45, 0.3)] {
        for step in 0..15 {
            let hue = step as f32 / 15.0;
            colors.push(crate::markings::from_hsv([hue, saturation, value], 1.0));
        }
    }
    for step in 0..15 {
        let value = step as f32 / 14.0;
        colors.push(crate::markings::from_hsv([0.0, 0.0, value], 1.0));
    }
    colors
}

/// Палитра для выбранной цели и поле `#RRGGBB`.
fn palette(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    content: Entity,
    state: &CharacterEditor,
    profile: &HumanoidCharacterProfile,
) {
    // Кожа числовой окраски палитрой не красится.
    if state.color == ColorTarget::Skin && skin_unary(prototypes, profile).is_some() {
        return;
    }
    let target = match &state.color {
        ColorTarget::Skin => text_or(world, "humanoid-profile-editor-skin-color-label", "Кожа"),
        ColorTarget::Eyes => text_or(world, "humanoid-profile-editor-eyes-label", "Глаза"),
        ColorTarget::Marking { marking, index, .. } => {
            format!("{} #{}", marking_name(world, marking), index + 1)
        }
    };
    let current = target_color(profile, &state.color);
    let row = ui.row(content);
    ui.label(row, target, 12.0);
    if let Some(color) = current {
        ui.swatch(row, color, true, UiAction::new(|_| {}));
    }
    let hex_text = if state.typing == Some(Field::Hex) || !state.hex.is_empty() {
        state.hex.clone()
    } else {
        current.map(hex).unwrap_or_default()
    };
    ui.field(
        row,
        "#",
        hex_text.trim_start_matches('#'),
        Field::Hex,
        state.typing,
    );
    let colors = palette_colors();
    for line in colors.chunks(15) {
        let row = ui.row(content);
        for &color in line {
            ui.swatch(
                row,
                color,
                false,
                UiAction::new(move |world| apply_color(world, color)),
            );
        }
    }
}

fn priority_key(priority: JobPriority) -> &'static str {
    match priority {
        JobPriority::High => "humanoid-profile-editor-job-priority-high-button",
        JobPriority::Medium => "humanoid-profile-editor-job-priority-medium-button",
        JobPriority::Low => "humanoid-profile-editor-job-priority-low-button",
        JobPriority::Never => "humanoid-profile-editor-job-priority-never-button",
    }
}

/// Должности по отделам с приоритетами (`RefreshJobs`).
fn jobs(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    content: Entity,
    profile: &HumanoidCharacterProfile,
) {
    // Отделы текут по колонкам: так все должности помещаются в окно без прокрутки.
    let grid = ui.row(content);
    ui.commands.entity(grid).insert(Node {
        flex_direction: FlexDirection::Column,
        flex_wrap: FlexWrap::Wrap,
        align_content: AlignContent::FlexStart,
        height: Val::Px(JOBS_HEIGHT),
        column_gap: Val::Px(12.0),
        row_gap: Val::Px(6.0),
        ..default()
    });
    for DepartmentEntry {
        name, color, jobs, ..
    } in departments(prototypes)
    {
        if jobs.is_empty() {
            continue;
        }
        let block = ui.column(grid, Some(320.0));
        let title = loc(
            world,
            "humanoid-profile-editor-department-jobs-label",
            &[("departmentName", LocArg::from(loc(world, &name, &[])))],
        );
        let header = ui.row(block);
        if let Some(color) = color {
            ui.swatch(header, color, false, UiAction::new(|_| {}));
        }
        ui.label(header, title, 13.0);
        for job in jobs {
            let row = ui.row(block);
            let label = ui.node(
                row,
                Node {
                    width: Val::Px(130.0),
                    ..default()
                },
            );
            ui.label(label, text_or(world, &job.name, &job.id), 12.0);
            let current = edit::job_priority(profile, &job.id);
            for priority in [
                JobPriority::High,
                JobPriority::Medium,
                JobPriority::Low,
                JobPriority::Never,
            ] {
                let id = job.id.clone();
                ui.chip(
                    row,
                    text_or(world, priority_key(priority), priority_key(priority)),
                    current == priority,
                    UiAction::new(move |world| {
                        change(world, |_, profile, _| {
                            profile.set_job_priority(&id, priority)
                        })
                    }),
                );
            }
        }
    }
}

/// Антагонисты: да или нет (`RefreshAntags`).
fn antags(
    world: &GameWorld,
    prototypes: &Prototypes,
    ui: &mut Ui,
    content: Entity,
    profile: &HumanoidCharacterProfile,
) {
    let mut list = edit::antags(prototypes);
    list.sort_by_key(|a| text_or(world, &a.name, &a.id).to_lowercase());
    for antag in list {
        let row = ui.row(content);
        let label = ui.node(
            row,
            Node {
                width: Val::Px(260.0),
                ..default()
            },
        );
        ui.label(label, text_or(world, &antag.name, &antag.id), 12.0);
        let wanted = profile.antag_preferences.contains(&antag.id);
        for (value, key) in [
            (true, "humanoid-profile-editor-antag-preference-yes-button"),
            (false, "humanoid-profile-editor-antag-preference-no-button"),
        ] {
            let id = antag.id.clone();
            ui.chip(
                row,
                text_or(world, key, if value { "Да" } else { "Нет" }),
                wanted == value,
                UiAction::new(move |world| {
                    change(world, |_, profile, _| edit::set_antag(profile, &id, value))
                }),
            );
        }
    }
}

/// Экран создания персонажа.
pub struct CharacterUiPlugin;

impl Plugin for CharacterUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
        app.add_systems(Update, window);
    }
}

#[cfg(test)]
mod tests;
