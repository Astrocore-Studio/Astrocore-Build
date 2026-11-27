//! Администраторы в базе: ранги с флагами прав, администратор с рангом, своими флагами
//! (добавленными и отнятыми), заголовком, отстранением и снятыми полномочиями. Итоговые права —
//! флаги ранга плюс свои положительные минус свои отрицательные, как `AdminManager` SS14.
//! Образец — `Admin`, `AdminRank`, `AdminFlag`, `AdminRankFlag` (`Model.cs`) и запросы
//! администраторов `ServerDbBase` (`GetAdminDataForAsync`, `GetAllAdminAndRanksAsync`,
//! `AddAdminAsync`, `UpdateAdminAsync`, `RemoveAdminAsync`, `AddAdminRankAsync`) SS14 @ 2f4a2aad.
//!
//! Нет пока: самих прав и команд администратора, окна прав (6.6).

use std::collections::BTreeSet;

use astrocore_net::NetUserId;
use sqlx::Row;

use crate::database::{Db, DbResult, db_error, parse_user_id};

/// Ранг администратора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRankRecord {
    pub id: i64,
    pub name: String,
    pub flags: BTreeSet<String>,
}

/// Администратор.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecord {
    pub user_id: NetUserId,
    pub title: Option<String>,
    pub deadminned: bool,
    pub suspended: bool,
    pub rank: Option<AdminRankRecord>,
    /// Свои флаги: имя и отрицательный ли (отнимает право ранга).
    pub flags: Vec<(String, bool)>,
}

impl AdminRecord {
    /// Итоговые права: флаги ранга, свои положительные, без своих отрицательных.
    pub fn effective_flags(&self) -> BTreeSet<String> {
        let mut flags: BTreeSet<String> = self
            .rank
            .as_ref()
            .map(|r| r.flags.clone())
            .unwrap_or_default();
        for (flag, negative) in &self.flags {
            if !negative {
                flags.insert(flag.clone());
            }
        }
        for (flag, negative) in &self.flags {
            if *negative {
                flags.remove(flag);
            }
        }
        flags
    }
}

impl Db {
    /// Ранг с флагами.
    pub async fn get_admin_rank(&self, id: i64) -> DbResult<Option<AdminRankRecord>> {
        let Some(row) = sqlx::query("SELECT id, name FROM admin_rank WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let flags = sqlx::query("SELECT flag FROM admin_rank_flag WHERE admin_rank_id = $1")
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
            .iter()
            .map(|r| r.try_get::<String, _>("flag").map_err(db_error))
            .collect::<DbResult<_>>()?;
        Ok(Some(AdminRankRecord {
            id: row.try_get("id").map_err(db_error)?,
            name: row.try_get("name").map_err(db_error)?,
            flags,
        }))
    }

    /// Добавить ранг (`AddAdminRankAsync`): его номер.
    pub async fn add_admin_rank(&self, name: &str, flags: &[&str]) -> DbResult<i64> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = sqlx::query("INSERT INTO admin_rank (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;
        let id: i64 = row.try_get("id").map_err(db_error)?;
        for flag in flags {
            sqlx::query("INSERT INTO admin_rank_flag (flag, admin_rank_id) VALUES ($1, $2)")
                .bind(*flag)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(id)
    }

    /// Удалить ранг: администраторы остаются без ранга (`RemoveAdminRankAsync`).
    pub async fn remove_admin_rank(&self, id: i64) -> DbResult<()> {
        sqlx::query("DELETE FROM admin_rank WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Администратор (`GetAdminDataForAsync`).
    pub async fn get_admin_data_for(&self, user_id: NetUserId) -> DbResult<Option<AdminRecord>> {
        let Some(row) = sqlx::query(
            "SELECT user_id, title, deadminned, suspended, admin_rank_id FROM admin WHERE user_id = $1",
        )
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        else {
            return Ok(None);
        };
        let rank = match row
            .try_get::<Option<i64>, _>("admin_rank_id")
            .map_err(db_error)?
        {
            Some(id) => self.get_admin_rank(id).await?,
            None => None,
        };
        let flags =
            sqlx::query("SELECT flag, negative FROM admin_flag WHERE admin_id = $1 ORDER BY id")
                .bind(user_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?
                .iter()
                .map(|r| {
                    Ok((
                        r.try_get::<String, _>("flag").map_err(db_error)?,
                        r.try_get::<i64, _>("negative").map_err(db_error)? != 0,
                    ))
                })
                .collect::<DbResult<_>>()?;
        Ok(Some(AdminRecord {
            user_id: parse_user_id(&row.try_get::<String, _>("user_id").map_err(db_error)?)?,
            title: row.try_get("title").map_err(db_error)?,
            deadminned: row.try_get::<i64, _>("deadminned").map_err(db_error)? != 0,
            suspended: row.try_get::<i64, _>("suspended").map_err(db_error)? != 0,
            rank,
            flags,
        }))
    }

    /// Добавить или заменить администратора со своими флагами (`AddAdminAsync`,
    /// `UpdateAdminAsync`).
    pub async fn set_admin(&self, admin: &AdminRecord) -> DbResult<()> {
        let id = admin.user_id.to_string();
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            "INSERT INTO admin (user_id, title, deadminned, suspended, admin_rank_id) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (user_id) DO UPDATE SET \
             title = excluded.title, deadminned = excluded.deadminned, \
             suspended = excluded.suspended, admin_rank_id = excluded.admin_rank_id",
        )
        .bind(id.clone())
        .bind(admin.title.clone())
        .bind(i64::from(admin.deadminned))
        .bind(i64::from(admin.suspended))
        .bind(admin.rank.as_ref().map(|r| r.id))
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query("DELETE FROM admin_flag WHERE admin_id = $1")
            .bind(id.clone())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        for (flag, negative) in &admin.flags {
            sqlx::query("INSERT INTO admin_flag (flag, negative, admin_id) VALUES ($1, $2, $3)")
                .bind(flag.clone())
                .bind(i64::from(*negative))
                .bind(id.clone())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    /// Запомнить, что права сняты на время (`UpdateAdminDeadminnedAsync`).
    pub async fn update_admin_deadminned(
        &self,
        user_id: NetUserId,
        deadminned: bool,
    ) -> DbResult<()> {
        sqlx::query("UPDATE admin SET deadminned = $1 WHERE user_id = $2")
            .bind(i64::from(deadminned))
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Снять администратора (`RemoveAdminAsync`).
    pub async fn remove_admin(&self, user_id: NetUserId) -> DbResult<()> {
        sqlx::query("DELETE FROM admin WHERE user_id = $1")
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Все администраторы с последними именами и все ранги (`GetAllAdminAndRanksAsync`).
    pub async fn get_all_admins_and_ranks(
        &self,
    ) -> DbResult<(Vec<(AdminRecord, Option<String>)>, Vec<AdminRankRecord>)> {
        let ids: Vec<i64> = sqlx::query("SELECT id FROM admin_rank ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
            .iter()
            .map(|r| r.try_get("id").map_err(db_error))
            .collect::<DbResult<_>>()?;
        let mut ranks = Vec::new();
        for id in ids {
            ranks.extend(self.get_admin_rank(id).await?);
        }
        let users: Vec<(String, Option<String>)> = sqlx::query(
            "SELECT admin.user_id, player.last_seen_user_name FROM admin \
             LEFT JOIN player ON player.user_id = admin.user_id ORDER BY admin.user_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?
        .iter()
        .map(|r| {
            Ok((
                r.try_get("user_id").map_err(db_error)?,
                r.try_get("last_seen_user_name").map_err(db_error)?,
            ))
        })
        .collect::<DbResult<_>>()?;
        let mut admins = Vec::new();
        for (user, name) in users {
            if let Some(admin) = self.get_admin_data_for(parse_user_id(&user)?).await? {
                admins.push((admin, name));
            }
        }
        Ok((admins, ranks))
    }
}

#[cfg(test)]
mod tests;
