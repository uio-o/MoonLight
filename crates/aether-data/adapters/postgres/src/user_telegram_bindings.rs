use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Row};

use aether_data_contracts::repository::user_telegram_bindings::{
    CreateUserTelegramBindingRecord, StoredUserTelegramBinding, UserTelegramBindingReadRepository,
    UserTelegramBindingWriteRepository, VerifyUserTelegramBindingRecord,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;

const SELECT_USER_TELEGRAM_BINDING_BY_USER_SQL: &str = r#"
SELECT
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM user_telegram_bindings
WHERE user_id = $1
"#;

const SELECT_USER_TELEGRAM_BINDING_BY_ACTIVE_LINK_CODE_SQL: &str = r#"
SELECT
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM user_telegram_bindings
WHERE link_code = $1
  AND verified_at IS NULL
  AND link_code_expires_at > TO_TIMESTAMP($2::double precision)
"#;

const SELECT_VERIFIED_USER_TELEGRAM_BINDINGS_SQL: &str = r#"
SELECT
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM user_telegram_bindings
WHERE verified_at IS NOT NULL
  AND chat_id IS NOT NULL
ORDER BY user_id
"#;

/// Re-binding replaces the previous row instead of stacking a second one, so
/// `chat_id` is cleared explicitly: a stale verified chat would otherwise keep
/// receiving another account's notifications after the user moved chats.
const CREATE_USER_TELEGRAM_BINDING_SQL: &str = r#"
INSERT INTO user_telegram_bindings (
  id,
  user_id,
  chat_id,
  link_code,
  link_code_expires_at,
  created_at,
  updated_at
)
VALUES ($1, $2, NULL, $3, TO_TIMESTAMP($4::double precision), NOW(), NOW())
ON CONFLICT (user_id) DO UPDATE SET
  chat_id = NULL,
  chat_type = NULL,
  chat_title = NULL,
  username = NULL,
  link_code = EXCLUDED.link_code,
  link_code_expires_at = EXCLUDED.link_code_expires_at,
  verified_at = NULL,
  last_notified_at = NULL,
  notification_count = 0,
  updated_at = NOW()
RETURNING
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

/// The `verified_at IS NULL` guard is what makes a code single-use: a replay
/// finds no row even though the code is still stored. The code is rotated on
/// success for the same reason.
const VERIFY_USER_TELEGRAM_BINDING_SQL: &str = r#"
UPDATE user_telegram_bindings
SET
  chat_id = $2,
  chat_type = $3,
  chat_title = $4,
  username = $5,
  verified_at = TO_TIMESTAMP($6::double precision),
  link_code = $7,
  link_code_expires_at = TO_TIMESTAMP($8::double precision),
  updated_at = NOW()
WHERE link_code = $1
  AND verified_at IS NULL
  AND link_code_expires_at > NOW()
RETURNING
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

/// Only verified rows are touched, so an unverified leftover can never start
/// accumulating notification counters.
const RECORD_USER_TELEGRAM_NOTIFICATION_SQL: &str = r#"
UPDATE user_telegram_bindings
SET
  last_notified_at = TO_TIMESTAMP($2::double precision),
  notification_count = notification_count + 1,
  updated_at = NOW()
WHERE user_id = $1
  AND verified_at IS NOT NULL
  AND chat_id IS NOT NULL
RETURNING
  id,
  user_id,
  chat_id,
  chat_type,
  chat_title,
  username,
  link_code,
  EXTRACT(EPOCH FROM link_code_expires_at)::bigint AS link_code_expires_at_unix_secs,
  EXTRACT(EPOCH FROM verified_at)::bigint AS verified_at_unix_secs,
  EXTRACT(EPOCH FROM last_notified_at)::bigint AS last_notified_at_unix_secs,
  notification_count,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_secs,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const DELETE_USER_TELEGRAM_BINDING_FOR_USER_SQL: &str = r#"
DELETE FROM user_telegram_bindings
WHERE user_id = $1
"#;

#[derive(Debug, Clone)]
pub struct SqlxUserTelegramBindingRepository {
    pool: PgPool,
}

impl SqlxUserTelegramBindingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn map_row(row: &PgRow) -> Result<StoredUserTelegramBinding, DataLayerError> {
        Ok(StoredUserTelegramBinding {
            id: row.try_get("id").map_postgres_err()?,
            user_id: row.try_get("user_id").map_postgres_err()?,
            chat_id: row.try_get("chat_id").map_postgres_err()?,
            chat_type: row.try_get("chat_type").map_postgres_err()?,
            chat_title: row.try_get("chat_title").map_postgres_err()?,
            username: row.try_get("username").map_postgres_err()?,
            link_code: row.try_get("link_code").map_postgres_err()?,
            link_code_expires_at_unix_secs: non_negative(
                row,
                "link_code_expires_at_unix_secs",
            )?,
            verified_at_unix_secs: optional_unix_secs(row, "verified_at_unix_secs")?,
            last_notified_at_unix_secs: optional_unix_secs(row, "last_notified_at_unix_secs")?,
            notification_count: non_negative(row, "notification_count")?,
            created_at_unix_secs: non_negative(row, "created_at_unix_secs")?,
            updated_at_unix_secs: non_negative(row, "updated_at_unix_secs")?,
        })
    }
}

fn non_negative(row: &PgRow, column: &str) -> Result<u64, DataLayerError> {
    let value = row.try_get::<i64, _>(column).map_postgres_err()?;
    u64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("user_telegram_bindings.{column} is negative"))
    })
}

fn optional_unix_secs(row: &PgRow, column: &str) -> Result<Option<u64>, DataLayerError> {
    let value = row.try_get::<Option<i64>, _>(column).map_postgres_err()?;
    value.map(|value| non_negative_value(value, column)).transpose()
}

fn non_negative_value(value: i64, column: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("user_telegram_bindings.{column} is negative"))
    })
}

#[async_trait]
impl UserTelegramBindingReadRepository for SqlxUserTelegramBindingRepository {
    async fn find_user_telegram_binding_by_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let row = sqlx::query(SELECT_USER_TELEGRAM_BINDING_BY_USER_SQL)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;

        row.as_ref().map(Self::map_row).transpose()
    }

    async fn find_user_telegram_binding_by_active_link_code(
        &self,
        link_code: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let row = sqlx::query(SELECT_USER_TELEGRAM_BINDING_BY_ACTIVE_LINK_CODE_SQL)
            .bind(link_code)
            .bind(now_unix_secs as f64)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;

        row.as_ref().map(Self::map_row).transpose()
    }

    async fn list_verified_user_telegram_bindings(
        &self,
    ) -> Result<Vec<StoredUserTelegramBinding>, DataLayerError> {
        let rows = sqlx::query(SELECT_VERIFIED_USER_TELEGRAM_BINDINGS_SQL)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;

        rows.iter().map(Self::map_row).collect()
    }
}

#[async_trait]
impl UserTelegramBindingWriteRepository for SqlxUserTelegramBindingRepository {
    async fn create_user_telegram_binding(
        &self,
        record: CreateUserTelegramBindingRecord,
    ) -> Result<StoredUserTelegramBinding, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(CREATE_USER_TELEGRAM_BINDING_SQL)
            .bind(record.id)
            .bind(record.user_id)
            .bind(record.link_code)
            .bind(record.link_code_expires_at_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        Self::map_row(&row)
    }

    async fn verify_user_telegram_binding(
        &self,
        record: VerifyUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(VERIFY_USER_TELEGRAM_BINDING_SQL)
            .bind(record.link_code)
            .bind(record.chat_id)
            .bind(record.chat_type)
            .bind(record.chat_title)
            .bind(record.username)
            .bind(record.verified_at_unix_secs as f64)
            .bind(record.rotated_link_code)
            .bind(record.rotated_link_code_expires_at_unix_secs as f64)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;

        row.as_ref().map(Self::map_row).transpose()
    }

    async fn delete_user_telegram_binding_for_user(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(DELETE_USER_TELEGRAM_BINDING_FOR_USER_SQL)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        Ok(result.rows_affected() > 0)
    }

    async fn record_user_telegram_notification(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let row = sqlx::query(RECORD_USER_TELEGRAM_NOTIFICATION_SQL)
            .bind(user_id)
            .bind(now_unix_secs as f64)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;

        row.as_ref().map(Self::map_row).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CREATE_USER_TELEGRAM_BINDING_SQL, RECORD_USER_TELEGRAM_NOTIFICATION_SQL,
        SELECT_USER_TELEGRAM_BINDING_BY_ACTIVE_LINK_CODE_SQL, VERIFY_USER_TELEGRAM_BINDING_SQL,
        SqlxUserTelegramBindingRepository,
    };
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let _repository = SqlxUserTelegramBindingRepository::new(pool);
    }

    #[test]
    fn rebinding_clears_the_previous_chat_instead_of_stacking_a_second_one() {
        assert!(CREATE_USER_TELEGRAM_BINDING_SQL.contains("ON CONFLICT (user_id) DO UPDATE SET"));
        for column in ["chat_id", "chat_type", "chat_title", "username", "verified_at"] {
            assert!(
                CREATE_USER_TELEGRAM_BINDING_SQL.contains(&format!("{column} = NULL,")),
                "re-binding must reset {column}"
            );
        }
        assert!(CREATE_USER_TELEGRAM_BINDING_SQL.contains("link_code = EXCLUDED.link_code,"));
        assert!(CREATE_USER_TELEGRAM_BINDING_SQL.contains("notification_count = 0,"));
    }

    #[test]
    fn verification_is_single_use_and_rotates_the_code() {
        assert!(VERIFY_USER_TELEGRAM_BINDING_SQL.contains("WHERE link_code = $1"));
        assert!(VERIFY_USER_TELEGRAM_BINDING_SQL.contains("AND verified_at IS NULL"));
        assert!(VERIFY_USER_TELEGRAM_BINDING_SQL.contains("AND link_code_expires_at > NOW()"));
        assert!(VERIFY_USER_TELEGRAM_BINDING_SQL.contains("link_code = $7,"));
        assert!(VERIFY_USER_TELEGRAM_BINDING_SQL.contains("verified_at = TO_TIMESTAMP($6"));
    }

    #[test]
    fn active_link_code_lookup_rejects_verified_and_expired_rows() {
        assert!(
            SELECT_USER_TELEGRAM_BINDING_BY_ACTIVE_LINK_CODE_SQL.contains("AND verified_at IS NULL")
        );
        assert!(SELECT_USER_TELEGRAM_BINDING_BY_ACTIVE_LINK_CODE_SQL
            .contains("AND link_code_expires_at > TO_TIMESTAMP($2::double precision)"));
    }

    #[test]
    fn notification_counters_only_advance_for_verified_rows() {
        assert!(RECORD_USER_TELEGRAM_NOTIFICATION_SQL.contains("AND verified_at IS NOT NULL"));
        assert!(RECORD_USER_TELEGRAM_NOTIFICATION_SQL.contains("AND chat_id IS NOT NULL"));
        assert!(RECORD_USER_TELEGRAM_NOTIFICATION_SQL
            .contains("notification_count = notification_count + 1,"));
    }
}
