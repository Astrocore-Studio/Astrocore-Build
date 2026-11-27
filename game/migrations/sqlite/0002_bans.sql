-- Баны: на сервер и на роли, по игрокам, адресам и раундам; снятия, исключения, попадания.
-- Образец — Content.Server.Database/Model.Ban.cs и Model.cs SS14 @ 2f4a2aad (ADR-0011).

CREATE TABLE ban (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type BIGINT NOT NULL,
    playtime_at_note BIGINT NOT NULL DEFAULT 0,
    ban_time BIGINT NOT NULL,
    expiration_time BIGINT,
    reason TEXT NOT NULL,
    severity BIGINT NOT NULL DEFAULT 0,
    banning_admin TEXT,
    last_edited_by_id TEXT,
    last_edited_at BIGINT,
    exempt_flags BIGINT NOT NULL DEFAULT 0,
    auto_delete BIGINT NOT NULL DEFAULT 0,
    hidden BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT no_exempt_on_role_ban CHECK (type = 0 OR exempt_flags = 0)
);
CREATE INDEX ix_ban_type ON ban (type);

CREATE TABLE ban_player (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    ban_id BIGINT NOT NULL REFERENCES ban (id) ON DELETE CASCADE,
    UNIQUE (user_id, ban_id)
);
CREATE INDEX ix_ban_player_ban_id ON ban_player (ban_id);

CREATE TABLE ban_address (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL,
    ban_id BIGINT NOT NULL REFERENCES ban (id) ON DELETE CASCADE,
    UNIQUE (address, ban_id)
);
CREATE INDEX ix_ban_address_ban_id ON ban_address (ban_id);

CREATE TABLE ban_role (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_type TEXT NOT NULL,
    role_id TEXT NOT NULL,
    ban_id BIGINT NOT NULL REFERENCES ban (id) ON DELETE CASCADE,
    UNIQUE (role_type, role_id, ban_id)
);
CREATE INDEX ix_ban_role_ban_id ON ban_role (ban_id);

CREATE TABLE ban_round (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    round_id BIGINT NOT NULL REFERENCES round (id) ON DELETE CASCADE,
    ban_id BIGINT NOT NULL REFERENCES ban (id) ON DELETE CASCADE,
    UNIQUE (round_id, ban_id)
);
CREATE INDEX ix_ban_round_ban_id ON ban_round (ban_id);

CREATE TABLE unban (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ban_id BIGINT NOT NULL UNIQUE REFERENCES ban (id) ON DELETE CASCADE,
    unbanning_admin TEXT,
    unban_time BIGINT NOT NULL
);

CREATE TABLE ban_exemption (
    user_id TEXT NOT NULL PRIMARY KEY,
    flags BIGINT NOT NULL,
    CONSTRAINT flags_not_zero CHECK (flags != 0)
);

CREATE TABLE server_ban_hit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ban_id BIGINT NOT NULL REFERENCES ban (id) ON DELETE CASCADE,
    connection_id BIGINT NOT NULL REFERENCES connection_log (id) ON DELETE CASCADE
);
CREATE INDEX ix_server_ban_hit_ban_id ON server_ban_hit (ban_id);
CREATE INDEX ix_server_ban_hit_connection_id ON server_ban_hit (connection_id);
