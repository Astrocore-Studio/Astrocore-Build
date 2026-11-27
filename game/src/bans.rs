//! Баны: на сервер и на роли, по игрокам и адресам (подсети), с раундом, сроком, тяжестью и
//! причиной; снятие бана, исключения для игроков (`ServerBanExemptFlags`), попадания в бан при
//! входе. Сверка бана с игроком — как `BanMatcher` SS14; действующие баны берутся из базы
//! целиком и сверяются в памяти, как в `ServerDbSqlite` (ADR-0011).
//! Образец — `Model.Ban.cs`, `BanDef`, `UnbanDef`, `BanMatcher`, `ServerDbSqlite`
//! (`GetBanQueryAsync`, `AddBanAsync`, `AddUnbanAsync`), `ServerDbBase` (`GetBanExemption`,
//! `UpdateBanExemption`, `AddServerBanHitsAsync`) SS14 @ 2f4a2aad.
//!
//! Нет пока: банов по железу (`HWId` — у клиента его нет), команд `ban`, `unban`, `banlist` и
//! окна банов (6.6), правки бана, банов на роли при выборе должности (6.3).

use std::net::IpAddr;

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_loc::LocArg;
use astrocore_net::NetUserId;
use bevy_ecs::world::World;
use sqlx::Row;
use sqlx::any::AnyRow;

use crate::database::{Db, DbResult, db_error, now_ms, parse_user_id};

/// Ссылка на обжалование банов (`infolinks.appeal`).
pub const INFO_LINKS_APPEAL: CVarDef<&str> = CVarDef::new(
    "infolinks.appeal",
    "",
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Ссылка, где обжалуют баны.",
);

/// Вид бана (`BanType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BanType {
    Server = 0,
    Role = 1,
}

/// Тяжесть нарушения (`NoteSeverity`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NoteSeverity {
    #[default]
    None = 0,
    Minor = 1,
    Medium = 2,
    High = 3,
}

impl NoteSeverity {
    fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Minor,
            2 => Self::Medium,
            3 => Self::High,
            _ => Self::None,
        }
    }
}

/// Исключения из банов (`ServerBanExemptFlags`).
pub mod exempt {
    pub const NONE: i64 = 0;
    /// Бан, наложенный на диапазон дата-центров.
    pub const DATACENTER: i64 = 1 << 0;
    /// Бан по адресу.
    pub const IP: i64 = 1 << 1;
    /// Бан на диапазон из чёрного списка: касается только новых игроков.
    pub const BLACKLISTED_RANGE: i64 = 1 << 2;
    pub const ALL: i64 = i32::MAX as i64;
}

/// Подсеть адресов бана (`NpgsqlInet`): адрес и длина префикса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    pub address: IpAddr,
    pub prefix: u8,
}

impl IpRange {
    /// Один адрес.
    pub fn single(address: IpAddr) -> Self {
        let prefix = if address.is_ipv4() { 32 } else { 128 };
        Self { address, prefix }
    }

    /// Разбор `адрес` или `адрес/префикс`.
    pub fn parse(text: &str) -> Option<Self> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let address: IpAddr = address.trim().parse().ok()?;
        let max = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.trim().parse::<u8>().ok().filter(|p| *p <= max)?,
            None => max,
        };
        Some(Self { address, prefix })
    }

    /// Входит ли адрес в подсеть (`IsInSubnet`). Адрес IPv4 внутри IPv6 сверяется как IPv4.
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = match address {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
            v4 => v4,
        };
        match (self.address, address) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for IpRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

/// Снятие бана (`UnbanDef`).
#[derive(Debug, Clone, PartialEq)]
pub struct UnbanDef {
    pub ban_id: i64,
    pub unbanning_admin: Option<NetUserId>,
    pub unban_time: i64,
}

/// Бан (`BanDef`).
#[derive(Debug, Clone, PartialEq)]
pub struct BanDef {
    pub id: Option<i64>,
    pub ban_type: BanType,
    pub user_ids: Vec<NetUserId>,
    pub addresses: Vec<IpRange>,
    /// Роли бана на роли: тип и идентификатор (`BanRoleDef`).
    pub roles: Vec<(String, String)>,
    pub round_ids: Vec<i64>,
    /// Время бана, мс UTC.
    pub ban_time: i64,
    /// Конец бана, мс UTC; `None` — навсегда.
    pub expiration_time: Option<i64>,
    pub reason: String,
    pub severity: NoteSeverity,
    pub banning_admin: Option<NetUserId>,
    /// Сыграно к моменту бана, мс.
    pub playtime_at_note: i64,
    pub exempt_flags: i64,
    pub auto_delete: bool,
    pub hidden: bool,
    pub unban: Option<UnbanDef>,
}

impl BanDef {
    /// Бан на сервер с этого момента.
    pub fn server(reason: &str, expiration_time: Option<i64>) -> Self {
        Self {
            id: None,
            ban_type: BanType::Server,
            user_ids: Vec::new(),
            addresses: Vec::new(),
            roles: Vec::new(),
            round_ids: Vec::new(),
            ban_time: now_ms(),
            expiration_time,
            reason: reason.into(),
            severity: NoteSeverity::None,
            banning_admin: None,
            playtime_at_note: 0,
            exempt_flags: exempt::NONE,
            auto_delete: false,
            hidden: false,
            unban: None,
        }
    }

    /// Действует ли бан сейчас: не снят и не истёк.
    pub fn is_active(&self, now: i64) -> bool {
        self.unban.is_none() && self.expiration_time.is_none_or(|end| end > now)
    }
}

/// Сведения об игроке для сверки (`BanMatcher.PlayerInfo`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerInfo {
    pub user_id: Option<NetUserId>,
    pub address: Option<IpAddr>,
    pub exempt_flags: i64,
    pub is_new_player: bool,
}

/// Попадает ли игрок под бан (`BanMatcher.BanMatches`).
pub fn ban_matches(ban: &BanDef, player: &PlayerInfo) -> bool {
    let mut exempt_flags = player.exempt_flags;
    // Любое исключение снимает и баны диапазонов из чёрного списка.
    if exempt_flags != exempt::NONE {
        exempt_flags |= exempt::BLACKLISTED_RANGE;
    }
    if ban.exempt_flags & exempt_flags != 0 {
        return false;
    }
    if player.exempt_flags & exempt::IP == 0
        && let Some(address) = player.address
        && ban.addresses.iter().any(|range| range.contains(address))
        && (ban.exempt_flags & exempt::BLACKLISTED_RANGE == 0 || player.is_new_player)
    {
        return true;
    }
    player.user_id.is_some_and(|id| ban.user_ids.contains(&id))
}

/// Условие отбора банов: вид, действующие (если не просят все), без исключений игрока.
/// `$1` — вид, `$2` — 1, если нужны и снятые с истёкшими, `$3` — сейчас, `$4` — исключения.
macro_rules! ban_filter {
    () => {
        "b.type = $1 AND ($2 = 1 OR (NOT EXISTS (SELECT 1 FROM unban u WHERE u.ban_id = b.id) \
         AND (b.expiration_time IS NULL OR b.expiration_time > $3))) AND (b.exempt_flags & $4) = 0"
    };
}

fn ban_row(row: &AnyRow) -> DbResult<BanDef> {
    let user = |column: &str| -> DbResult<Option<NetUserId>> {
        row.try_get::<Option<String>, _>(column)
            .map_err(db_error)?
            .map(|text| parse_user_id(&text))
            .transpose()
    };
    Ok(BanDef {
        id: Some(row.try_get("id").map_err(db_error)?),
        ban_type: if row.try_get::<i64, _>("type").map_err(db_error)? == 1 {
            BanType::Role
        } else {
            BanType::Server
        },
        user_ids: Vec::new(),
        addresses: Vec::new(),
        roles: Vec::new(),
        round_ids: Vec::new(),
        ban_time: row.try_get("ban_time").map_err(db_error)?,
        expiration_time: row.try_get("expiration_time").map_err(db_error)?,
        reason: row.try_get("reason").map_err(db_error)?,
        severity: NoteSeverity::from_i64(row.try_get("severity").map_err(db_error)?),
        banning_admin: user("banning_admin")?,
        playtime_at_note: row.try_get("playtime_at_note").map_err(db_error)?,
        exempt_flags: row.try_get("exempt_flags").map_err(db_error)?,
        auto_delete: row.try_get::<i64, _>("auto_delete").map_err(db_error)? != 0,
        hidden: row.try_get::<i64, _>("hidden").map_err(db_error)? != 0,
        unban: None,
    })
}

impl Db {
    /// Сохранить бан со всеми его игроками, адресами, ролями и раундами (`AddBanAsync`).
    pub async fn add_ban(&self, ban: &BanDef) -> DbResult<BanDef> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = sqlx::query(
            "INSERT INTO ban (type, playtime_at_note, ban_time, expiration_time, reason, severity, \
             banning_admin, exempt_flags, auto_delete, hidden) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
        )
        .bind(ban.ban_type as i64)
        .bind(ban.playtime_at_note)
        .bind(ban.ban_time)
        .bind(ban.expiration_time)
        .bind(ban.reason.clone())
        .bind(ban.severity as i64)
        .bind(ban.banning_admin.map(|id| id.to_string()))
        .bind(ban.exempt_flags)
        .bind(i64::from(ban.auto_delete))
        .bind(i64::from(ban.hidden))
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let id: i64 = row.try_get("id").map_err(db_error)?;
        for user in &ban.user_ids {
            sqlx::query("INSERT INTO ban_player (user_id, ban_id) VALUES ($1, $2)")
                .bind(user.to_string())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        for address in &ban.addresses {
            sqlx::query("INSERT INTO ban_address (address, ban_id) VALUES ($1, $2)")
                .bind(address.to_string())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        for (role_type, role_id) in &ban.roles {
            sqlx::query("INSERT INTO ban_role (role_type, role_id, ban_id) VALUES ($1, $2, $3)")
                .bind(role_type.clone())
                .bind(role_id.clone())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        for round in &ban.round_ids {
            sqlx::query("INSERT INTO ban_round (round_id, ban_id) VALUES ($1, $2)")
                .bind(*round)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(BanDef {
            id: Some(id),
            ..ban.clone()
        })
    }

    /// Снять бан (`AddUnbanAsync`).
    pub async fn add_unban(&self, unban: &UnbanDef) -> DbResult<()> {
        sqlx::query("INSERT INTO unban (ban_id, unbanning_admin, unban_time) VALUES ($1, $2, $3)")
            .bind(unban.ban_id)
            .bind(unban.unbanning_admin.map(|id| id.to_string()))
            .bind(unban.unban_time)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Бан по номеру (`GetBanAsync(int)`).
    pub async fn get_ban(&self, id: i64) -> DbResult<Option<BanDef>> {
        let row = sqlx::query(
            "SELECT id, type, playtime_at_note, ban_time, expiration_time, reason, severity, \
             banning_admin, exempt_flags, auto_delete, hidden FROM ban WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut bans = vec![ban_row(&row)?];
        self.fill_bans(&mut bans).await?;
        Ok(bans.pop())
    }

    /// Игроки, адреса, роли, раунды и снятия банов.
    async fn fill_bans(&self, bans: &mut [BanDef]) -> DbResult<()> {
        for ban in bans.iter_mut() {
            let Some(id) = ban.id else {
                continue;
            };
            ban.user_ids =
                sqlx::query("SELECT user_id FROM ban_player WHERE ban_id = $1 ORDER BY id")
                    .bind(id)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(db_error)?
                    .iter()
                    .map(|r| parse_user_id(&r.try_get::<String, _>("user_id").map_err(db_error)?))
                    .collect::<DbResult<_>>()?;
            ban.addresses =
                sqlx::query("SELECT address FROM ban_address WHERE ban_id = $1 ORDER BY id")
                    .bind(id)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(db_error)?
                    .iter()
                    .filter_map(|r| IpRange::parse(&r.try_get::<String, _>("address").ok()?))
                    .collect();
            ban.roles = sqlx::query(
                "SELECT role_type, role_id FROM ban_role WHERE ban_id = $1 ORDER BY id",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
            .iter()
            .map(|r| {
                Ok((
                    r.try_get("role_type").map_err(db_error)?,
                    r.try_get("role_id").map_err(db_error)?,
                ))
            })
            .collect::<DbResult<_>>()?;
            ban.round_ids =
                sqlx::query("SELECT round_id FROM ban_round WHERE ban_id = $1 ORDER BY id")
                    .bind(id)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(db_error)?
                    .iter()
                    .map(|r| r.try_get("round_id").map_err(db_error))
                    .collect::<DbResult<_>>()?;
            ban.unban = sqlx::query(
                "SELECT ban_id, unbanning_admin, unban_time FROM unban WHERE ban_id = $1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
            .map(|r| {
                Ok::<_, String>(UnbanDef {
                    ban_id: r.try_get("ban_id").map_err(db_error)?,
                    unbanning_admin: r
                        .try_get::<Option<String>, _>("unbanning_admin")
                        .map_err(db_error)?
                        .map(|t| parse_user_id(&t))
                        .transpose()?,
                    unban_time: r.try_get("unban_time").map_err(db_error)?,
                })
            })
            .transpose()?;
        }
        Ok(())
    }

    /// Исключения игрока из банов (`GetBanExemption`).
    pub async fn get_ban_exemption(&self, user_id: NetUserId) -> DbResult<i64> {
        let row = sqlx::query("SELECT flags FROM ban_exemption WHERE user_id = $1")
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map_or(Ok(exempt::NONE), |r| r.try_get("flags").map_err(db_error))
    }

    /// Задать исключения игрока; ноль — убрать (`UpdateBanExemption`).
    pub async fn update_ban_exemption(&self, user_id: NetUserId, flags: i64) -> DbResult<()> {
        if flags == exempt::NONE {
            sqlx::query("DELETE FROM ban_exemption WHERE user_id = $1")
                .bind(user_id.to_string())
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO ban_exemption (user_id, flags) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET flags = excluded.flags",
        )
        .bind(user_id.to_string())
        .bind(flags)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    /// Баны, под которые попадает игрок (`GetBansAsync`): по идентификатору и адресу, с
    /// учётом исключений и того, новый ли он.
    pub async fn get_bans(
        &self,
        address: Option<IpAddr>,
        user_id: Option<NetUserId>,
        include_unbanned: bool,
        ban_type: BanType,
    ) -> DbResult<Vec<BanDef>> {
        let exemption = match user_id {
            Some(id) => self.get_ban_exemption(id).await?,
            None => exempt::NONE,
        };
        let is_new_player = match user_id {
            Some(id) => self.get_player_record_by_user_id(id).await?.is_none(),
            None => true,
        };
        let mut filter_flags = exemption;
        if filter_flags != exempt::NONE {
            filter_flags |= exempt::BLACKLISTED_RANGE;
        }
        let rows = sqlx::query(concat!(
            "SELECT b.id, b.type, b.playtime_at_note, b.ban_time, b.expiration_time, b.reason, \
             b.severity, b.banning_admin, b.exempt_flags, b.auto_delete, b.hidden FROM ban b WHERE ",
            ban_filter!(),
            " ORDER BY b.id"
        ))
        .bind(ban_type as i64)
        .bind(i64::from(include_unbanned))
        .bind(now_ms())
        .bind(filter_flags)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut bans = rows.iter().map(ban_row).collect::<DbResult<Vec<_>>>()?;
        self.fill_bans(&mut bans).await?;
        let player = PlayerInfo {
            user_id,
            address,
            exempt_flags: exemption,
            is_new_player,
        };
        bans.retain(|ban| ban_matches(ban, &player));
        Ok(bans)
    }

    /// Попадания в баны при входе (`AddServerBanHitsAsync`).
    pub async fn add_server_ban_hits(&self, connection: i64, bans: &[BanDef]) -> DbResult<()> {
        for ban in bans {
            let Some(id) = ban.id else {
                continue;
            };
            sqlx::query("INSERT INTO server_ban_hit (ban_id, connection_id) VALUES ($1, $2)")
                .bind(id)
                .bind(connection)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    }

    /// Сколько раз бан не пустил игрока.
    pub async fn ban_hit_count(&self, ban_id: i64) -> DbResult<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS hits FROM server_ban_hit WHERE ban_id = $1")
            .bind(ban_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        row.try_get("hits").map_err(db_error)
    }
}

/// Время UTC текстом `ГГГГ-ММ-ДД ЧЧ:ММ` (мс от 1970 года).
pub fn format_utc(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Гражданская дата по числу дней (алгоритм Хиннанта).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// Текст отказа во входе по бану (`BanDef.FormatBanMessage`).
pub fn format_ban_message(world: &World, ban: &BanDef) -> String {
    let loc = |key: &str, args: &[(&str, LocArg)]| crate::examine::loc(world, key, args);
    let expires = match ban.expiration_time {
        Some(end) => {
            let minutes = ((end - ban.ban_time) as f64 / 60_000.0).round() as i64;
            loc(
                "ban-expires",
                &[
                    ("duration", LocArg::from(minutes.to_string().as_str())),
                    ("time", LocArg::from(format_utc(end).as_str())),
                ],
            )
        }
        None => {
            let appeal = world
                .resource::<astrocore_console::Config>()
                .get_text(&INFO_LINKS_APPEAL);
            if appeal.trim().is_empty() {
                loc("ban-banned-permanent", &[])
            } else {
                loc(
                    "ban-banned-permanent-appeal",
                    &[("link", LocArg::from(appeal.as_str()))],
                )
            }
        }
    };
    [
        loc("ban-banned-1", &[]),
        loc(
            "ban-banned-2",
            &[("reason", LocArg::from(ban.reason.as_str()))],
        ),
        expires,
        loc("ban-banned-3", &[]),
    ]
    .join("\n")
}

pub fn install(world: &mut World) {
    world.register_cvar(&INFO_LINKS_APPEAL);
}

#[cfg(test)]
mod tests;
