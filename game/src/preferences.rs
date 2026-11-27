//! Настройки игроков на сервере: при входе профили загружаются из базы (новому игроку — один
//! случайный персонаж), проверяются и уходят клиенту; клиент выбирает слот, сохраняет и
//! удаляет персонажей — сервер проверяет и пишет в базу. Гостю без сохранения (или без базы)
//! достаётся случайный персонаж в памяти. Образец — `ServerPreferencesManager`,
//! `MsgPreferencesAndSettings`, `MsgSelectCharacter`, `MsgUpdateCharacter`,
//! `MsgDeleteCharacter`, запросы настроек `ServerDbBase` (`InitPrefsAsync`,
//! `GetPlayerPreferencesAsync`, `SaveCharacterSlotAsync`, `SaveSelectedCharacterIndexAsync`,
//! `DeleteSlotAndSetSelectedIndex`) SS14 @ 2f4a2aad.
//!
//! Нет пока: избранного строительства (`ConstructionFavorites`), цвета OOC администратора в
//! окне (6.6), отметки активности (`AfkManager`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use astrocore_console::{Config, ConfigWorldExt};
use astrocore_ecs::EventWorldExt;
use astrocore_math::Color;
use astrocore_net::{
    NetEvent, NetEventWorldExt, NetTarget, NetUserId, PlayerConnected, PlayerDisconnected, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::Side;
use crate::database::{Db, DbResult, GAME_PERSIST_GUESTS, db_error, spawn_db};
use crate::profiles::{
    GAME_MAX_CHARACTER_SLOTS, Gender, HumanoidCharacterAppearance, HumanoidCharacterProfile,
    IC_NEW_ACCOUNT_SPECIES_BLACKLIST, JobPriority, OrganMarkings, PlayerPreferences,
    PreferenceUnavailableMode, RoleLoadout, SpawnPriorityPreference, parse_sex, random_profile,
    sex_name, validated,
};

/// Сообщения настроек (`MsgPreferencesAndSettings`, `MsgSelectCharacter`,
/// `MsgUpdateCharacter`, `MsgDeleteCharacter`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreferencesMessage {
    /// Сервер → клиент: настройки и предел слотов.
    PreferencesAndSettings {
        preferences: PlayerPreferences,
        max_character_slots: i32,
    },
    /// Клиент → сервер: выбрать слот.
    SelectCharacter(i32),
    /// Клиент → сервер: сохранить персонажа в слот.
    UpdateCharacter {
        slot: i32,
        profile: Box<HumanoidCharacterProfile>,
    },
    /// Клиент → сервер: удалить персонажа.
    DeleteCharacter(i32),
}

/// Настройки загружены и проверены (`FinishLoad`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct PreferencesLoaded {
    pub session: Entity,
    pub user_id: NetUserId,
}

/// Загруженные настройки игроков (`_cachedPlayerPrefs`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct PreferencesCache {
    prefs: HashMap<NetUserId, PlayerPreferences>,
    /// Настройки этих игроков хранятся в базе.
    stored: BTreeSet<NetUserId>,
}

impl PreferencesCache {
    /// Настройки игрока, если загружены (`TryGetCachedPreferences`).
    pub fn get(&self, user: NetUserId) -> Option<&PlayerPreferences> {
        self.prefs.get(&user)
    }

    /// Выбранный персонаж игрока.
    pub fn selected_profile(&self, user: NetUserId) -> Option<&HumanoidCharacterProfile> {
        self.prefs.get(&user)?.selected_character()
    }
}

/// Цвет OOC администратора по умолчанию (`Color.Red`).
fn default_admin_ooc_color() -> Color {
    Color::rgba(1.0, 0.0, 0.0, 1.0)
}

// База.

/// Метки внешности для базы: JSON (`OrganMarkings`).
fn markings_json(markings: &OrganMarkings) -> Option<String> {
    (!markings.is_empty()).then(|| serde_json::to_string(markings).unwrap_or_default())
}

impl Db {
    async fn insert_profile(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        preference: i64,
        slot: i32,
        profile: &HumanoidCharacterProfile,
    ) -> DbResult<()> {
        let row = sqlx::query(
            "INSERT INTO profile (preference_id, slot, char_name, flavor_text, age, sex, voice, \
             gender, species, organ_markings, eye_color, skin_color, spawn_priority, pref_unavailable) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING id",
        )
        .bind(preference)
        .bind(i64::from(slot))
        .bind(profile.name.clone())
        .bind(profile.flavor_text.clone())
        .bind(i64::from(profile.age))
        .bind(sex_name(profile.sex))
        .bind(profile.voice.clone())
        .bind(profile.gender.name())
        .bind(profile.species.clone())
        .bind(markings_json(&profile.appearance.markings))
        .bind(profile.appearance.eye_color.to_hex())
        .bind(profile.appearance.skin_color.to_hex())
        .bind(profile.spawn_priority as i64)
        .bind(profile.preference_unavailable as i64)
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?;
        let id: i64 = row.try_get("id").map_err(db_error)?;
        for (job, priority) in &profile.job_priorities {
            sqlx::query("INSERT INTO job (profile_id, job_name, priority) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(job.clone())
                .bind(*priority as i64)
                .execute(&mut **tx)
                .await
                .map_err(db_error)?;
        }
        for antag in &profile.antag_preferences {
            sqlx::query("INSERT INTO antag (profile_id, antag_name) VALUES ($1, $2)")
                .bind(id)
                .bind(antag.clone())
                .execute(&mut **tx)
                .await
                .map_err(db_error)?;
        }
        for item in &profile.trait_preferences {
            sqlx::query("INSERT INTO trait (profile_id, trait_name) VALUES ($1, $2)")
                .bind(id)
                .bind(item.clone())
                .execute(&mut **tx)
                .await
                .map_err(db_error)?;
        }
        for (role, loadout) in &profile.loadouts {
            let row = sqlx::query(
                "INSERT INTO profile_role_loadout (profile_id, role_name, entity_name) \
                 VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(id)
            .bind(role.clone())
            .bind(loadout.entity_name.clone())
            .fetch_one(&mut **tx)
            .await
            .map_err(db_error)?;
            let role_id: i64 = row.try_get("id").map_err(db_error)?;
            for (group, items) in &loadout.selected {
                let row = sqlx::query(
                    "INSERT INTO profile_loadout_group (profile_role_loadout_id, group_name) \
                     VALUES ($1, $2) RETURNING id",
                )
                .bind(role_id)
                .bind(group.clone())
                .fetch_one(&mut **tx)
                .await
                .map_err(db_error)?;
                let group_id: i64 = row.try_get("id").map_err(db_error)?;
                for item in items {
                    sqlx::query(
                        "INSERT INTO profile_loadout (profile_loadout_group_id, loadout_name) \
                         VALUES ($1, $2)",
                    )
                    .bind(group_id)
                    .bind(item.clone())
                    .execute(&mut **tx)
                    .await
                    .map_err(db_error)?;
                }
            }
        }
        Ok(())
    }

    /// Настройки нового игрока с одним персонажем (`InitPrefsAsync`).
    pub async fn init_prefs(
        &self,
        user: NetUserId,
        profile: &HumanoidCharacterProfile,
    ) -> DbResult<PlayerPreferences> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = sqlx::query(
            "INSERT INTO preference (user_id, selected_character_slot, admin_ooc_color) \
             VALUES ($1, 0, $2) RETURNING id",
        )
        .bind(user.to_string())
        .bind(default_admin_ooc_color().to_hex())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let preference: i64 = row.try_get("id").map_err(db_error)?;
        self.insert_profile(&mut tx, preference, 0, profile).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(PlayerPreferences {
            characters: BTreeMap::from([(0, profile.clone())]),
            selected_character_index: 0,
            admin_ooc_color: default_admin_ooc_color(),
        })
    }

    async fn profile_row(
        &self,
        row: &sqlx::any::AnyRow,
    ) -> DbResult<(i32, HumanoidCharacterProfile)> {
        let id: i64 = row.try_get("id").map_err(db_error)?;
        let slot: i64 = row.try_get("slot").map_err(db_error)?;
        let names = |sql: &'static str, column: &'static str| async move {
            sqlx::query(sql)
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?
                .iter()
                .map(|r| r.try_get::<String, _>(column).map_err(db_error))
                .collect::<DbResult<BTreeSet<String>>>()
        };
        let job_priorities =
            sqlx::query("SELECT job_name, priority FROM job WHERE profile_id = $1")
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?
                .iter()
                .map(|r| {
                    Ok((
                        r.try_get::<String, _>("job_name").map_err(db_error)?,
                        JobPriority::from_i64(r.try_get("priority").map_err(db_error)?),
                    ))
                })
                .collect::<DbResult<BTreeMap<_, _>>>()?;
        let antag_preferences = names(
            "SELECT antag_name FROM antag WHERE profile_id = $1",
            "antag_name",
        )
        .await?;
        let trait_preferences = names(
            "SELECT trait_name FROM trait WHERE profile_id = $1",
            "trait_name",
        )
        .await?;
        let mut loadouts = BTreeMap::new();
        let roles = sqlx::query(
            "SELECT id, role_name, entity_name FROM profile_role_loadout WHERE profile_id = $1 ORDER BY id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        for role in &roles {
            let role_id: i64 = role.try_get("id").map_err(db_error)?;
            let name: String = role.try_get("role_name").map_err(db_error)?;
            let mut loadout = RoleLoadout {
                role: name.clone(),
                entity_name: role.try_get("entity_name").map_err(db_error)?,
                selected: BTreeMap::new(),
            };
            let groups = sqlx::query(
                "SELECT id, group_name FROM profile_loadout_group WHERE profile_role_loadout_id = $1 ORDER BY id",
            )
            .bind(role_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
            for group in &groups {
                let group_id: i64 = group.try_get("id").map_err(db_error)?;
                let items = sqlx::query(
                    "SELECT loadout_name FROM profile_loadout WHERE profile_loadout_group_id = $1 ORDER BY id",
                )
                .bind(group_id)
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?
                .iter()
                .map(|r| r.try_get::<String, _>("loadout_name").map_err(db_error))
                .collect::<DbResult<Vec<_>>>()?;
                loadout
                    .selected
                    .insert(group.try_get("group_name").map_err(db_error)?, items);
            }
            loadouts.insert(name, loadout);
        }
        let sex = parse_sex(&row.try_get::<String, _>("sex").map_err(db_error)?)
            .unwrap_or(crate::body::Sex::Male);
        let gender = Gender::parse(&row.try_get::<String, _>("gender").map_err(db_error)?)
            .unwrap_or(crate::profiles::gender_for_sex(sex));
        let markings = row
            .try_get::<Option<String>, _>("organ_markings")
            .map_err(db_error)?
            .and_then(|json| serde_json::from_str::<OrganMarkings>(&json).ok())
            .unwrap_or_default();
        let color = |column: &str| -> DbResult<Color> {
            Ok(
                Color::parse(&row.try_get::<String, _>(column).map_err(db_error)?)
                    .unwrap_or(Color::BLACK),
            )
        };
        let profile = HumanoidCharacterProfile {
            name: row.try_get("char_name").map_err(db_error)?,
            flavor_text: row.try_get("flavor_text").map_err(db_error)?,
            species: row.try_get("species").map_err(db_error)?,
            age: row.try_get::<i64, _>("age").map_err(db_error)? as i32,
            sex,
            voice: row
                .try_get::<Option<String>, _>("voice")
                .map_err(db_error)?
                .unwrap_or_default(),
            gender,
            appearance: HumanoidCharacterAppearance {
                eye_color: color("eye_color")?,
                skin_color: color("skin_color")?,
                markings,
            },
            spawn_priority: SpawnPriorityPreference::from_i64(
                row.try_get("spawn_priority").map_err(db_error)?,
            ),
            job_priorities,
            preference_unavailable: PreferenceUnavailableMode::from_i64(
                row.try_get("pref_unavailable").map_err(db_error)?,
            ),
            antag_preferences,
            trait_preferences,
            loadouts,
        };
        Ok((slot as i32, profile))
    }

    /// Настройки игрока (`GetPlayerPreferencesAsync`).
    pub async fn get_player_preferences(
        &self,
        user: NetUserId,
    ) -> DbResult<Option<PlayerPreferences>> {
        let Some(pref) = sqlx::query(
            "SELECT id, selected_character_slot, admin_ooc_color FROM preference WHERE user_id = $1",
        )
        .bind(user.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        else {
            return Ok(None);
        };
        let preference: i64 = pref.try_get("id").map_err(db_error)?;
        let rows = sqlx::query(
            "SELECT id, slot, char_name, flavor_text, age, sex, voice, gender, species, \
             organ_markings, eye_color, skin_color, spawn_priority, pref_unavailable \
             FROM profile WHERE preference_id = $1 ORDER BY slot",
        )
        .bind(preference)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut characters = BTreeMap::new();
        for row in &rows {
            let (slot, profile) = self.profile_row(row).await?;
            characters.insert(slot, profile);
        }
        Ok(Some(PlayerPreferences {
            characters,
            selected_character_index: pref
                .try_get::<i64, _>("selected_character_slot")
                .map_err(db_error)? as i32,
            admin_ooc_color: Color::parse(
                &pref
                    .try_get::<String, _>("admin_ooc_color")
                    .map_err(db_error)?,
            )
            .unwrap_or_else(default_admin_ooc_color),
        }))
    }

    async fn preference_id(&self, user: NetUserId) -> DbResult<Option<i64>> {
        let row = sqlx::query("SELECT id FROM preference WHERE user_id = $1")
            .bind(user.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(|r| r.try_get("id").map_err(db_error)).transpose()
    }

    /// Сохранить персонажа в слот или удалить его (`SaveCharacterSlotAsync`).
    pub async fn save_character_slot(
        &self,
        user: NetUserId,
        profile: Option<&HumanoidCharacterProfile>,
        slot: i32,
    ) -> DbResult<()> {
        let Some(preference) = self.preference_id(user).await? else {
            return Ok(());
        };
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query("DELETE FROM profile WHERE preference_id = $1 AND slot = $2")
            .bind(preference)
            .bind(i64::from(slot))
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        if let Some(profile) = profile {
            self.insert_profile(&mut tx, preference, slot, profile)
                .await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    /// Выбранный слот (`SaveSelectedCharacterIndexAsync`).
    pub async fn save_selected_character_index(&self, user: NetUserId, slot: i32) -> DbResult<()> {
        sqlx::query("UPDATE preference SET selected_character_slot = $1 WHERE user_id = $2")
            .bind(i64::from(slot))
            .bind(user.to_string())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Удалить слот и выбрать другой (`DeleteSlotAndSetSelectedIndex`).
    pub async fn delete_slot_and_set_selected_index(
        &self,
        user: NetUserId,
        slot: i32,
        new_slot: i32,
    ) -> DbResult<()> {
        self.save_character_slot(user, None, slot).await?;
        self.save_selected_character_index(user, new_slot).await
    }
}

// Менеджер.

fn max_slots(world: &World) -> i32 {
    world.resource::<Config>().get(&GAME_MAX_CHARACTER_SLOTS)
}

fn session_user(world: &World, session: Entity) -> Option<NetUserId> {
    world.get::<Session>(session).map(|s| s.user_id)
}

fn session_of(world: &mut World, user: NetUserId) -> Option<Entity> {
    world
        .query::<(Entity, &Session)>()
        .iter(world)
        .find(|(_, s)| s.user_id == user)
        .map(|(e, _)| e)
}

/// Хранятся ли настройки игрока (`ShouldStorePrefs`): есть база и гости сохраняются.
fn should_store(world: &World) -> bool {
    world.get_resource::<crate::database::Database>().is_some() && world.cvar(&GAME_PERSIST_GUESTS)
}

/// Проверить, запомнить и отправить клиенту (`FinishLoad`).
fn finish_load(world: &mut World, user: NetUserId, prefs: PlayerPreferences, stored: bool) {
    let Some(session) = session_of(world, user) else {
        return;
    };
    // Проверка — на случай изменившейся игры (`SanitizePreferences`).
    let characters = prefs
        .characters
        .iter()
        .map(|(slot, profile)| (*slot, validated(world, profile)))
        .collect();
    let prefs = PlayerPreferences {
        characters,
        ..prefs
    };
    {
        let mut cache = world.resource_mut::<PreferencesCache>();
        cache.prefs.insert(user, prefs.clone());
        if stored {
            cache.stored.insert(user);
        }
    }
    let message = PreferencesMessage::PreferencesAndSettings {
        preferences: prefs,
        max_character_slots: max_slots(world),
    };
    world.send_net_event(NetTarget::Session(session), &message);
    world.raise_broadcast(&mut PreferencesLoaded {
        session,
        user_id: user,
    });
}

/// Загрузить настройки вошедшего (`LoadData`, `GetOrCreatePreferencesAsync`).
fn load(world: &mut World, session: Entity) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    let ignored: BTreeSet<String> = world
        .resource::<Config>()
        .get_text(&IC_NEW_ACCOUNT_SPECIES_BLACKLIST)
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    // Случайный персонаж готовится сразу: он нужен гостю и новому игроку.
    let mut fresh = random_profile(world, &ignored);
    fresh.set_jobs_from_config(world.resource::<Config>());
    if !should_store(world) {
        let prefs = PlayerPreferences {
            characters: BTreeMap::from([(0, fresh)]),
            selected_character_index: 0,
            admin_ooc_color: Color::rgba(0.0, 0.0, 0.0, 0.0),
        };
        finish_load(world, user, prefs, false);
        return;
    }
    let fallback = fresh.clone();
    spawn_db(
        world,
        move |db| async move {
            match db.get_player_preferences(user).await? {
                Some(prefs) if !prefs.characters.is_empty() => Ok(prefs),
                Some(_) => {
                    db.save_character_slot(user, Some(&fresh), 0).await?;
                    db.save_selected_character_index(user, 0).await?;
                    Ok(PlayerPreferences {
                        characters: BTreeMap::from([(0, fresh)]),
                        selected_character_index: 0,
                        admin_ooc_color: default_admin_ooc_color(),
                    })
                }
                None => db.init_prefs(user, &fresh).await,
            }
        },
        move |world, result: DbResult<PlayerPreferences>| match result {
            Ok(prefs) => finish_load(world, user, prefs, true),
            Err(error) => {
                // Без настроек игрок не появится: даётся случайный персонаж без сохранения.
                eprintln!("настройки игрока {user}: {error}");
                let prefs = PlayerPreferences {
                    characters: BTreeMap::from([(0, fallback)]),
                    selected_character_index: 0,
                    admin_ooc_color: default_admin_ooc_color(),
                };
                finish_load(world, user, prefs, false);
            }
        },
    );
}

/// Сохранить в базу, если настройки игрока там хранятся.
fn store<F, Fut>(world: &World, user: NetUserId, task: F)
where
    F: FnOnce(Db) -> Fut,
    Fut: std::future::Future<Output = DbResult<()>> + Send + 'static,
{
    if !world.resource::<PreferencesCache>().stored.contains(&user) {
        return;
    }
    spawn_db(world, task, move |_, result: DbResult<()>| {
        if let Err(error) = result {
            eprintln!("настройки игрока {user}: {error}");
        }
    });
}

/// Выбрать слот (`HandleSelectCharacterMessage`).
pub fn select_character(world: &mut World, user: NetUserId, slot: i32) {
    if slot < 0 || slot >= max_slots(world) {
        return;
    }
    {
        let mut cache = world.resource_mut::<PreferencesCache>();
        let Some(prefs) = cache.prefs.get_mut(&user) else {
            return;
        };
        if !prefs.characters.contains_key(&slot) {
            return;
        }
        prefs.selected_character_index = slot;
    }
    store(world, user, move |db| async move {
        db.save_selected_character_index(user, slot).await
    });
}

/// Сохранить персонажа в слот (`SetProfile`): он же становится выбранным.
pub fn set_profile(
    world: &mut World,
    user: NetUserId,
    slot: i32,
    profile: &HumanoidCharacterProfile,
) {
    if slot < 0 || slot >= max_slots(world) {
        return;
    }
    if !world
        .resource::<PreferencesCache>()
        .prefs
        .contains_key(&user)
    {
        return;
    }
    let profile = validated(world, profile);
    {
        let mut cache = world.resource_mut::<PreferencesCache>();
        let Some(prefs) = cache.prefs.get_mut(&user) else {
            return;
        };
        prefs.characters.insert(slot, profile.clone());
        prefs.selected_character_index = slot;
    }
    store(world, user, move |db| async move {
        db.save_character_slot(user, Some(&profile), slot).await
    });
}

/// Удалить персонажа (`HandleDeleteCharacterMessage`): выбранный уступает место другому;
/// последнего не удалить.
pub fn delete_character(world: &mut World, user: NetUserId, slot: i32) {
    if slot < 0 {
        return;
    }
    let next = {
        let mut cache = world.resource_mut::<PreferencesCache>();
        let Some(prefs) = cache.prefs.get_mut(&user) else {
            return;
        };
        if !prefs.characters.contains_key(&slot) {
            return;
        }
        let mut next = None;
        if prefs.selected_character_index == slot {
            let Some(other) = prefs.characters.keys().copied().find(|s| *s != slot) else {
                return;
            };
            next = Some(other);
        }
        prefs.characters.remove(&slot);
        if let Some(other) = next {
            prefs.selected_character_index = other;
        }
        next
    };
    store(world, user, move |db| async move {
        match next {
            Some(other) => {
                db.delete_slot_and_set_selected_index(user, slot, other)
                    .await
            }
            None => db.save_character_slot(user, None, slot).await,
        }
    });
}

fn receive(world: &mut World, event: &mut NetEvent<PreferencesMessage>) {
    let Some(user) = event.session.and_then(|s| session_user(world, s)) else {
        return;
    };
    match event.event.clone() {
        PreferencesMessage::SelectCharacter(slot) => select_character(world, user, slot),
        PreferencesMessage::UpdateCharacter { slot, profile } => {
            set_profile(world, user, slot, &profile);
        }
        PreferencesMessage::DeleteCharacter(slot) => delete_character(world, user, slot),
        PreferencesMessage::PreferencesAndSettings { .. } => {}
    }
}

/// Настройки, пришедшие с сервера (клиент).
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct ClientPreferences {
    pub preferences: Option<PlayerPreferences>,
    pub max_character_slots: i32,
}

fn client_prefs(world: &mut World) -> Option<&mut PlayerPreferences> {
    world
        .get_resource_mut::<ClientPreferences>()?
        .into_inner()
        .preferences
        .as_mut()
}

/// Выбрать слот на клиенте (`ClientPreferencesManager.SelectCharacter`).
pub fn client_select_character(world: &mut World, slot: i32) {
    let Some(prefs) = client_prefs(world) else {
        return;
    };
    prefs.selected_character_index = slot;
    world.send_net_event_to_server(&PreferencesMessage::SelectCharacter(slot));
}

/// Сохранить персонажа в слот (`UpdateCharacter`): проверка на месте, как на сервере;
/// выбранный слот не меняется.
pub fn client_update_character(world: &mut World, profile: &HumanoidCharacterProfile, slot: i32) {
    let profile = validated(world, profile);
    let Some(prefs) = client_prefs(world) else {
        return;
    };
    prefs.characters.insert(slot, profile.clone());
    world.send_net_event_to_server(&PreferencesMessage::UpdateCharacter {
        slot,
        profile: Box::new(profile),
    });
}

/// Создать персонажа в первом свободном слоте (`CreateCharacter`); `None` — слотов нет.
pub fn client_create_character(
    world: &mut World,
    profile: &HumanoidCharacterProfile,
) -> Option<i32> {
    let max = world
        .get_resource::<ClientPreferences>()?
        .max_character_slots;
    let prefs = client_prefs(world)?;
    let slot = (0..max).find(|slot| !prefs.characters.contains_key(slot))?;
    client_update_character(world, profile, slot);
    Some(slot)
}

/// Удалить персонажа (`DeleteCharacter`); выбор поправит сервер.
pub fn client_delete_character(world: &mut World, slot: i32) {
    let Some(prefs) = client_prefs(world) else {
        return;
    };
    prefs.characters.remove(&slot);
    world.send_net_event_to_server(&PreferencesMessage::DeleteCharacter(slot));
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<PreferencesMessage>();
    if !side.is_server() {
        world.init_resource::<ClientPreferences>();
        world.subscribe_broadcast::<NetEvent<PreferencesMessage>>(|world, event| {
            if let PreferencesMessage::PreferencesAndSettings {
                preferences,
                max_character_slots,
            } = &event.event
            {
                *world.resource_mut::<ClientPreferences>() = ClientPreferences {
                    preferences: Some(preferences.clone()),
                    max_character_slots: *max_character_slots,
                };
            }
        });
        return;
    }
    world.init_resource::<PreferencesCache>();
    world.subscribe_broadcast::<PlayerConnected>(|world, event| load(world, event.session));
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        if let Some(user) = session_user(world, event.session) {
            let mut cache = world.resource_mut::<PreferencesCache>();
            cache.prefs.remove(&user);
            cache.stored.remove(&user);
        }
    });
    world.subscribe_broadcast::<NetEvent<PreferencesMessage>>(receive);
}

#[cfg(test)]
mod tests;
