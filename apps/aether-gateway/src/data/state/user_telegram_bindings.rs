use aether_data::repository::user_telegram_bindings::{
    CreateUserTelegramBindingRecord, StoredUserTelegramBinding, VerifyUserTelegramBindingRecord,
};
use aether_data::DataLayerError;

use super::GatewayDataState;

impl GatewayDataState {
    pub(crate) async fn find_user_telegram_binding(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_reader {
            Some(repository) => repository.find_user_telegram_binding_by_user(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_user_telegram_binding_by_active_link_code(
        &self,
        link_code: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_reader {
            Some(repository) => {
                repository
                    .find_user_telegram_binding_by_active_link_code(link_code, now_unix_secs)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn list_verified_user_telegram_bindings(
        &self,
    ) -> Result<Vec<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_reader {
            Some(repository) => repository.list_verified_user_telegram_bindings().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn create_user_telegram_binding(
        &self,
        record: CreateUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_writer {
            Some(repository) => repository.create_user_telegram_binding(record).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn verify_user_telegram_binding(
        &self,
        record: VerifyUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_writer {
            Some(repository) => repository.verify_user_telegram_binding(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_user_telegram_binding(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.user_telegram_binding_writer {
            Some(repository) => repository.delete_user_telegram_binding_for_user(user_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn record_user_telegram_notification(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, DataLayerError> {
        match &self.user_telegram_binding_writer {
            Some(repository) => {
                repository
                    .record_user_telegram_notification(user_id, now_unix_secs)
                    .await
            }
            None => Ok(None),
        }
    }
}
