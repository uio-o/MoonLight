use async_trait::async_trait;

/// Alphabet for link codes: no `I`, `O`, `0` or `1`, because a user has to
/// retype the code into Telegram and those four are the ones people misread.
pub const USER_TELEGRAM_LINK_CODE_ALPHABET: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const USER_TELEGRAM_LINK_CODE_LEN: usize = 10;
pub const MAX_USER_TELEGRAM_CHAT_ID_BYTES: usize = 64;
pub const MAX_USER_TELEGRAM_CHAT_TITLE_BYTES: usize = 255;
pub const MAX_USER_TELEGRAM_USERNAME_BYTES: usize = 255;

/// A per-user Telegram binding.
///
/// `chat_id` stays `NULL` until the link code round-trips through the bot, so a
/// pending row can never collide with another user's pending row: Postgres
/// treats NULLs in a unique index as distinct, while a verified chat is locked
/// to exactly one account.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserTelegramBinding {
    pub id: String,
    pub user_id: String,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub chat_title: Option<String>,
    pub username: Option<String>,
    pub link_code: String,
    pub link_code_expires_at_unix_secs: u64,
    pub verified_at_unix_secs: Option<u64>,
    pub last_notified_at_unix_secs: Option<u64>,
    pub notification_count: u64,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

impl StoredUserTelegramBinding {
    pub fn is_verified(&self) -> bool {
        self.verified_at_unix_secs.is_some() && self.chat_id.is_some()
    }

    /// A code is only usable while the row is still unverified and unexpired.
    pub fn link_code_is_active(&self, now_unix_secs: u64) -> bool {
        !self.is_verified() && now_unix_secs < self.link_code_expires_at_unix_secs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreateUserTelegramBindingRecord {
    pub id: String,
    pub user_id: String,
    pub link_code: String,
    pub link_code_expires_at_unix_secs: u64,
}

impl CreateUserTelegramBindingRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "user_telegram_bindings.id is empty".to_string(),
            ));
        }
        if self.user_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "user_telegram_bindings.user_id is empty".to_string(),
            ));
        }
        validate_user_telegram_link_code(&self.link_code)?;
        validate_user_telegram_unix_secs(self.link_code_expires_at_unix_secs)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerifyUserTelegramBindingRecord {
    /// Code the user sent to the bot; also the lookup key.
    pub link_code: String,
    /// Replacement code written back on success so the old one cannot be replayed.
    pub rotated_link_code: String,
    pub rotated_link_code_expires_at_unix_secs: u64,
    pub chat_id: String,
    pub chat_type: Option<String>,
    pub chat_title: Option<String>,
    pub username: Option<String>,
    pub verified_at_unix_secs: u64,
}

impl VerifyUserTelegramBindingRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_user_telegram_link_code(&self.link_code)?;
        validate_user_telegram_link_code(&self.rotated_link_code)?;
        validate_user_telegram_unix_secs(self.rotated_link_code_expires_at_unix_secs)?;
        validate_user_telegram_chat_id(&self.chat_id)?;
        validate_optional_user_telegram_text(
            self.chat_type.as_deref(),
            32,
            "user_telegram_bindings.chat_type",
        )?;
        validate_optional_user_telegram_text(
            self.chat_title.as_deref(),
            MAX_USER_TELEGRAM_CHAT_TITLE_BYTES,
            "user_telegram_bindings.chat_title",
        )?;
        validate_optional_user_telegram_text(
            self.username.as_deref(),
            MAX_USER_TELEGRAM_USERNAME_BYTES,
            "user_telegram_bindings.username",
        )?;
        validate_user_telegram_unix_secs(self.verified_at_unix_secs)?;
        Ok(())
    }
}

/// Link codes travel through a `t.me/<bot>?start=<code>` deep link, so they are
/// restricted to the URL- and human-safe alphabet above.
pub fn validate_user_telegram_link_code(value: &str) -> Result<(), crate::DataLayerError> {
    if value.len() != USER_TELEGRAM_LINK_CODE_LEN {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "Telegram 绑定码必须是 {USER_TELEGRAM_LINK_CODE_LEN} 位"
        )));
    }
    if !value
        .chars()
        .all(|ch| USER_TELEGRAM_LINK_CODE_ALPHABET.contains(ch))
    {
        return Err(crate::DataLayerError::InvalidInput(
            "Telegram 绑定码包含非法字符".to_string(),
        ));
    }
    Ok(())
}

/// Telegram chat ids are decimal, optionally negative for groups and channels.
/// The value is never padded by its callers, so surrounding whitespace is
/// rejected rather than trimmed away: a stored chat id is interpolated straight
/// into the Bot API request and must match byte for byte.
pub fn validate_user_telegram_chat_id(value: &str) -> Result<(), crate::DataLayerError> {
    if value.is_empty() || value.len() > MAX_USER_TELEGRAM_CHAT_ID_BYTES {
        return Err(crate::DataLayerError::InvalidInput(
            "Telegram Chat ID 为空或过长".to_string(),
        ));
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(crate::DataLayerError::InvalidInput(
            "Telegram Chat ID 必须是十进制数字".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_user_telegram_text(
    value: Option<&str>,
    max_bytes: usize,
    field: &str,
) -> Result<(), crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > max_bytes {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} exceeds {max_bytes} bytes"
        )));
    }
    if value
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} contains control characters"
        )));
    }
    Ok(())
}

fn validate_user_telegram_unix_secs(value: u64) -> Result<(), crate::DataLayerError> {
    if i64::try_from(value).is_err() {
        return Err(crate::DataLayerError::InvalidInput(
            "user_telegram_bindings timestamp exceeds supported storage range".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
pub trait UserTelegramBindingReadRepository: Send + Sync {
    async fn find_user_telegram_binding_by_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserTelegramBinding>, crate::DataLayerError>;

    /// Looks a code up only while it can still be consumed, so an expired or
    /// already-verified code is indistinguishable from an unknown one.
    async fn find_user_telegram_binding_by_active_link_code(
        &self,
        link_code: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, crate::DataLayerError>;

    async fn list_verified_user_telegram_bindings(
        &self,
    ) -> Result<Vec<StoredUserTelegramBinding>, crate::DataLayerError>;
}

#[async_trait]
pub trait UserTelegramBindingWriteRepository: Send + Sync {
    /// Replaces any binding the user already had, so a re-bind starts clean
    /// instead of leaving a stale verified row behind.
    async fn create_user_telegram_binding(
        &self,
        record: CreateUserTelegramBindingRecord,
    ) -> Result<StoredUserTelegramBinding, crate::DataLayerError>;

    /// Consumes a code atomically; returns `None` when it was already used,
    /// expired, or never existed.
    async fn verify_user_telegram_binding(
        &self,
        record: VerifyUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, crate::DataLayerError>;

    async fn delete_user_telegram_binding_for_user(
        &self,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn record_user_telegram_notification(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, crate::DataLayerError>;
}

pub trait UserTelegramBindingRepository:
    UserTelegramBindingReadRepository + UserTelegramBindingWriteRepository
{
}

impl<T> UserTelegramBindingRepository for T where
    T: UserTelegramBindingReadRepository + UserTelegramBindingWriteRepository
{
}

#[cfg(test)]
mod tests {
    use super::{
        validate_user_telegram_chat_id, validate_user_telegram_link_code,
        CreateUserTelegramBindingRecord, StoredUserTelegramBinding,
        USER_TELEGRAM_LINK_CODE_ALPHABET, USER_TELEGRAM_LINK_CODE_LEN,
    };

    fn binding() -> StoredUserTelegramBinding {
        StoredUserTelegramBinding {
            id: "binding-1".to_string(),
            user_id: "user-1".to_string(),
            chat_id: None,
            chat_type: None,
            chat_title: None,
            username: None,
            link_code: "ABCDEFGHJK".to_string(),
            link_code_expires_at_unix_secs: 1_800_000_100,
            verified_at_unix_secs: None,
            last_notified_at_unix_secs: None,
            notification_count: 0,
            created_at_unix_secs: 1_800_000_000,
            updated_at_unix_secs: 1_800_000_000,
        }
    }

    fn record() -> CreateUserTelegramBindingRecord {
        CreateUserTelegramBindingRecord {
            id: "binding-1".to_string(),
            user_id: "user-1".to_string(),
            link_code: "ABCDEFGHJK".to_string(),
            link_code_expires_at_unix_secs: 1_800_000_100,
        }
    }

    #[test]
    fn link_code_rejects_wrong_length_and_out_of_alphabet_characters() {
        for code in ["", "ABC", "ABCDEFGHIJ", "ABCDEFGHJ1", "abcdefghjk"] {
            assert!(
                validate_user_telegram_link_code(code).is_err(),
                "{code} should be rejected"
            );
        }
        assert!(validate_user_telegram_link_code("ABCDEFGHJK").is_ok());
        assert!(validate_user_telegram_link_code("23456789MN").is_ok());
    }

    #[test]
    fn link_code_alphabet_excludes_characters_people_misread() {
        for ch in ['I', 'O', '0', '1'] {
            assert!(!USER_TELEGRAM_LINK_CODE_ALPHABET.contains(ch));
        }
        assert_eq!(USER_TELEGRAM_LINK_CODE_ALPHABET.len(), 32);
    }

    #[test]
    fn chat_id_accepts_private_and_group_forms_only() {
        for chat_id in ["123456789", "-1001234567890"] {
            assert!(
                validate_user_telegram_chat_id(chat_id).is_ok(),
                "{chat_id} should be accepted"
            );
        }
        for chat_id in ["", "-", "12.5", "12a", " 123 ", "١٢٣"] {
            assert!(
                validate_user_telegram_chat_id(chat_id).is_err(),
                "{chat_id} should be rejected"
            );
        }
    }

    #[test]
    fn pending_binding_is_inactive_once_expired_or_verified() {
        let pending = binding();
        assert!(!pending.is_verified());
        assert!(pending.link_code_is_active(1_800_000_050));
        assert!(!pending.link_code_is_active(1_800_000_100));
        assert!(!pending.link_code_is_active(1_800_000_200));

        let mut verified = binding();
        verified.chat_id = Some("123456789".to_string());
        verified.verified_at_unix_secs = Some(1_800_000_010);
        assert!(verified.is_verified());
        assert!(!verified.link_code_is_active(1_800_000_050));
    }

    #[test]
    fn create_record_rejects_timestamps_outside_sql_storage_range() {
        let mut invalid = record();
        invalid.link_code_expires_at_unix_secs = i64::MAX as u64 + 1;
        assert!(invalid.validate().is_err());
        assert!(record().validate().is_ok());
    }

    #[test]
    fn create_record_rejects_empty_identifiers() {
        let mut invalid = record();
        invalid.user_id = "  ".to_string();
        assert!(invalid.validate().is_err());

        let mut invalid = record();
        invalid.link_code = "short".to_string();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn link_code_length_constant_matches_the_alphabet_capacity() {
        assert_eq!(USER_TELEGRAM_LINK_CODE_LEN, 10);
    }
}
