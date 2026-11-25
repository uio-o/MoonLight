use std::sync::RwLock;

use async_trait::async_trait;

use crate::DataLayerError;
use aether_data_contracts::repository::user_telegram_bindings::{
    CreateUserTelegramBindingRecord, StoredUserTelegramBinding, UserTelegramBindingReadRepository,
    UserTelegramBindingWriteRepository, VerifyUserTelegramBindingRecord,
};

/// Test-only stand-in for the Postgres repository. It mirrors the two
/// invariants the SQL leans on: at most one binding per user, and a link code
/// that is single-use.
#[derive(Debug, Default)]
pub struct InMemoryUserTelegramBindingRepository {
    items: RwLock<Vec<StoredUserTelegramBinding>>,
}

impl InMemoryUserTelegramBindingRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserTelegramBinding>,
    {
        Self {
            items: RwLock::new(items.into_iter().collect()),
        }
    }
}

#[async_trait]
impl UserTelegramBindingReadRepository for InMemoryUserTelegramBindingRepository {
    async fn find_user_telegram_binding_by_user(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let items = self.items.read().expect("telegram binding repository lock");
        Ok(items.iter().find(|item| item.user_id == user_id).cloned())
    }

    async fn find_user_telegram_binding_by_active_link_code(
        &self,
        link_code: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let items = self.items.read().expect("telegram binding repository lock");
        Ok(items
            .iter()
            .find(|item| item.link_code == link_code && item.link_code_is_active(now_unix_secs))
            .cloned())
    }

    async fn list_verified_user_telegram_bindings(
        &self,
    ) -> Result<Vec<StoredUserTelegramBinding>, DataLayerError> {
        let items = self.items.read().expect("telegram binding repository lock");
        Ok(items
            .iter()
            .filter(|item| item.is_verified())
            .cloned()
            .collect())
    }
}

#[async_trait]
impl UserTelegramBindingWriteRepository for InMemoryUserTelegramBindingRepository {
    async fn create_user_telegram_binding(
        &self,
        record: CreateUserTelegramBindingRecord,
    ) -> Result<StoredUserTelegramBinding, DataLayerError> {
        record.validate()?;
        let stored = StoredUserTelegramBinding {
            id: record.id,
            user_id: record.user_id,
            chat_id: None,
            chat_type: None,
            chat_title: None,
            username: None,
            link_code: record.link_code,
            link_code_expires_at_unix_secs: record.link_code_expires_at_unix_secs,
            verified_at_unix_secs: None,
            last_notified_at_unix_secs: None,
            notification_count: 0,
            created_at_unix_secs: record.link_code_expires_at_unix_secs,
            updated_at_unix_secs: record.link_code_expires_at_unix_secs,
        };
        let mut items = self
            .items
            .write()
            .expect("telegram binding repository lock");
        items.retain(|item| item.user_id != stored.user_id);
        items.push(stored.clone());
        Ok(stored)
    }

    async fn verify_user_telegram_binding(
        &self,
        record: VerifyUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        record.validate()?;
        let mut items = self
            .items
            .write()
            .expect("telegram binding repository lock");
        let Some(index) = items.iter().position(|item| {
            item.link_code == record.link_code
                && item.link_code_expires_at_unix_secs > record.verified_at_unix_secs
        }) else {
            return Ok(None);
        };
        let item = &mut items[index];
        item.chat_id = Some(record.chat_id);
        item.chat_type = record.chat_type;
        item.chat_title = record.chat_title;
        item.username = record.username;
        item.verified_at_unix_secs = Some(record.verified_at_unix_secs);
        item.link_code = record.rotated_link_code;
        item.link_code_expires_at_unix_secs = record.rotated_link_code_expires_at_unix_secs;
        item.updated_at_unix_secs = record.verified_at_unix_secs;
        Ok(Some(item.clone()))
    }

    async fn delete_user_telegram_binding_for_user(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut items = self
            .items
            .write()
            .expect("telegram binding repository lock");
        let before = items.len();
        items.retain(|item| item.user_id != user_id);
        Ok(items.len() != before)
    }

    async fn record_user_telegram_notification(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        let mut items = self
            .items
            .write()
            .expect("telegram binding repository lock");
        let Some(item) = items
            .iter_mut()
            .find(|item| item.user_id == user_id && item.is_verified())
        else {
            return Ok(None);
        };
        item.last_notified_at_unix_secs = Some(now_unix_secs);
        item.notification_count += 1;
        item.updated_at_unix_secs = now_unix_secs;
        Ok(Some(item.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryUserTelegramBindingRepository;
    use aether_data_contracts::repository::user_telegram_bindings::{
        CreateUserTelegramBindingRecord, UserTelegramBindingReadRepository,
        UserTelegramBindingWriteRepository, VerifyUserTelegramBindingRecord,
    };

    fn create(user_id: &str, code: &str, expires_at: u64) -> CreateUserTelegramBindingRecord {
        CreateUserTelegramBindingRecord {
            id: format!("binding-{user_id}"),
            user_id: user_id.to_string(),
            link_code: code.to_string(),
            link_code_expires_at_unix_secs: expires_at,
        }
    }

    fn verify(code: &str, chat_id: &str, at: u64) -> VerifyUserTelegramBindingRecord {
        VerifyUserTelegramBindingRecord {
            link_code: code.to_string(),
            rotated_link_code: "ZZZZZZZZZZ".to_string(),
            rotated_link_code_expires_at_unix_secs: at,
            chat_id: chat_id.to_string(),
            chat_type: Some("private".to_string()),
            chat_title: None,
            username: None,
            verified_at_unix_secs: at,
        }
    }

    #[tokio::test]
    async fn rebinding_replaces_the_previous_row() {
        let repository = InMemoryUserTelegramBindingRepository::default();
        repository
            .create_user_telegram_binding(create("user-1", "AAAAAAAAAA", 1_800))
            .await
            .expect("first binding");
        repository
            .create_user_telegram_binding(create("user-1", "BBBBBBBBBB", 1_900))
            .await
            .expect("second binding");

        let stored = repository
            .find_user_telegram_binding_by_user("user-1")
            .await
            .expect("lookup")
            .expect("binding");
        assert_eq!(stored.link_code, "BBBBBBBBBB");
        assert!(stored.chat_id.is_none());
    }

    #[tokio::test]
    async fn a_link_code_is_single_use() {
        let repository = InMemoryUserTelegramBindingRepository::default();
        repository
            .create_user_telegram_binding(create("user-1", "AAAAAAAAAA", 1_800))
            .await
            .expect("binding");

        let verified = repository
            .verify_user_telegram_binding(verify("AAAAAAAAAA", "12345", 1_700))
            .await
            .expect("verify")
            .expect("verified row");
        assert_eq!(verified.chat_id.as_deref(), Some("12345"));

        let replay = repository
            .verify_user_telegram_binding(verify("AAAAAAAAAA", "99999", 1_750))
            .await
            .expect("replay");
        assert!(replay.is_none(), "a consumed code must not verify twice");
    }

    #[tokio::test]
    async fn expired_codes_are_not_found() {
        let repository = InMemoryUserTelegramBindingRepository::default();
        repository
            .create_user_telegram_binding(create("user-1", "AAAAAAAAAA", 1_800))
            .await
            .expect("binding");

        assert!(repository
            .find_user_telegram_binding_by_active_link_code("AAAAAAAAAA", 1_799)
            .await
            .expect("lookup")
            .is_some());
        assert!(repository
            .find_user_telegram_binding_by_active_link_code("AAAAAAAAAA", 1_800)
            .await
            .expect("lookup")
            .is_none());
    }

    #[tokio::test]
    async fn only_verified_bindings_are_listed_and_counted() {
        let repository = InMemoryUserTelegramBindingRepository::default();
        repository
            .create_user_telegram_binding(create("user-1", "AAAAAAAAAA", 1_800))
            .await
            .expect("binding");
        repository
            .create_user_telegram_binding(create("user-2", "BBBBBBBBBB", 1_800))
            .await
            .expect("binding");
        repository
            .verify_user_telegram_binding(verify("BBBBBBBBBB", "777", 1_700))
            .await
            .expect("verify");

        let verified = repository
            .list_verified_user_telegram_bindings()
            .await
            .expect("list");
        assert_eq!(verified.len(), 1);
        assert_eq!(verified[0].user_id, "user-2");

        assert!(
            repository
                .record_user_telegram_notification("user-1", 1_750)
                .await
                .expect("record")
                .is_none(),
            "an unverified binding must not accumulate counters"
        );
        let counted = repository
            .record_user_telegram_notification("user-2", 1_750)
            .await
            .expect("record")
            .expect("counted row");
        assert_eq!(counted.notification_count, 1);
    }
}
