use crate::{AppState, GatewayError};
use aether_data::repository::user_telegram_bindings::{
    CreateUserTelegramBindingRecord, StoredUserTelegramBinding, VerifyUserTelegramBindingRecord,
};

impl AppState {
    pub(crate) async fn find_user_telegram_binding(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .find_user_telegram_binding(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_user_telegram_binding_by_active_link_code(
        &self,
        link_code: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .find_user_telegram_binding_by_active_link_code(link_code, now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_verified_user_telegram_bindings(
        &self,
    ) -> Result<Vec<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .list_verified_user_telegram_bindings()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_user_telegram_binding(
        &self,
        record: CreateUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .create_user_telegram_binding(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn verify_user_telegram_binding(
        &self,
        record: VerifyUserTelegramBindingRecord,
    ) -> Result<Option<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .verify_user_telegram_binding(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_user_telegram_binding(
        &self,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_user_telegram_binding(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn record_user_telegram_notification(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredUserTelegramBinding>, GatewayError> {
        self.data
            .record_user_telegram_notification(user_id, now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
