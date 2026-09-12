-- Schema for the server pet bot.
--
-- Discord snowflakes are stored as INTEGER (i64). They exceed i32 but fit
-- comfortably in SQLite's 64-bit integer.
-- All timestamps are unix seconds, UTC.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- One living (or most recently deceased) pet per guild.
CREATE TABLE IF NOT EXISTS pets (
    guild_id         INTEGER PRIMARY KEY,
    id               TEXT    NOT NULL,
    name             TEXT    NOT NULL,
    species          TEXT    NOT NULL,
    hunger           REAL    NOT NULL,
    happiness        REAL    NOT NULL,
    health           REAL    NOT NULL,
    energy           REAL    NOT NULL,
    xp               REAL    NOT NULL DEFAULT 0,
    asleep           INTEGER NOT NULL DEFAULT 0,
    alive            INTEGER NOT NULL DEFAULT 1,
    born_at          INTEGER NOT NULL,
    died_at          INTEGER,
    last_tick        INTEGER NOT NULL,
    custom_image_url TEXT,
    adopted_by       INTEGER NOT NULL,
    cause_of_death   TEXT
);

-- Only living pets need ticking; this keeps the background sweep cheap.
CREATE INDEX IF NOT EXISTS idx_pets_alive ON pets (alive);

-- Per-member contribution, scoped to one pet incarnation so the leaderboard
-- resets when a new pet is adopted while history is preserved.
CREATE TABLE IF NOT EXISTS caretakers (
    guild_id     INTEGER NOT NULL,
    pet_id       TEXT    NOT NULL,
    user_id      INTEGER NOT NULL,
    care_points  REAL    NOT NULL DEFAULT 0,
    feeds        INTEGER NOT NULL DEFAULT 0,
    plays        INTEGER NOT NULL DEFAULT 0,
    heals        INTEGER NOT NULL DEFAULT 0,
    last_feed_at INTEGER,
    last_play_at INTEGER,
    last_heal_at INTEGER,
    PRIMARY KEY (guild_id, pet_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_caretakers_board
    ON caretakers (guild_id, pet_id, care_points DESC);

-- Pets that have died, for the memorial listing.
CREATE TABLE IF NOT EXISTS graveyard (
    id       TEXT    PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    name     TEXT    NOT NULL,
    species  TEXT    NOT NULL,
    born_at  INTEGER NOT NULL,
    died_at  INTEGER NOT NULL,
    cause    TEXT    NOT NULL,
    level    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_graveyard_guild ON graveyard (guild_id, died_at DESC);

CREATE TABLE IF NOT EXISTS guild_settings (
    guild_id         INTEGER PRIMARY KEY,
    alert_channel_id INTEGER,
    alerts_enabled   INTEGER NOT NULL DEFAULT 1,
    last_alert       TEXT,
    last_alert_at    INTEGER
);
