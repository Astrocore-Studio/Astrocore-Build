-- Администраторы и ранги, журнал действий (admin logs).
-- Образец — Content.Server.Database/Model.cs SS14 @ 2f4a2aad (ADR-0011).

CREATE TABLE admin_rank (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL
);

CREATE TABLE admin_rank_flag (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    flag TEXT NOT NULL,
    admin_rank_id BIGINT NOT NULL REFERENCES admin_rank (id) ON DELETE CASCADE,
    UNIQUE (flag, admin_rank_id)
);

CREATE TABLE admin (
    user_id TEXT NOT NULL PRIMARY KEY,
    title TEXT,
    deadminned BIGINT NOT NULL DEFAULT 0,
    suspended BIGINT NOT NULL DEFAULT 0,
    admin_rank_id BIGINT REFERENCES admin_rank (id) ON DELETE SET NULL
);

CREATE TABLE admin_flag (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    flag TEXT NOT NULL,
    negative BIGINT NOT NULL DEFAULT 0,
    admin_id TEXT NOT NULL REFERENCES admin (user_id) ON DELETE CASCADE,
    UNIQUE (flag, admin_id)
);

CREATE TABLE admin_log (
    round_id BIGINT NOT NULL REFERENCES round (id) ON DELETE CASCADE,
    id BIGINT NOT NULL,
    type BIGINT NOT NULL,
    impact BIGINT NOT NULL,
    date BIGINT NOT NULL,
    message TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (round_id, id)
);
CREATE INDEX ix_admin_log_date ON admin_log (date);
CREATE INDEX ix_admin_log_type ON admin_log (type);

CREATE TABLE admin_log_player (
    round_id BIGINT NOT NULL,
    log_id BIGINT NOT NULL,
    player_user_id TEXT NOT NULL REFERENCES player (user_id) ON DELETE CASCADE,
    PRIMARY KEY (round_id, log_id, player_user_id),
    FOREIGN KEY (round_id, log_id) REFERENCES admin_log (round_id, id) ON DELETE CASCADE
);
CREATE INDEX ix_admin_log_player_player_user_id ON admin_log_player (player_user_id);
