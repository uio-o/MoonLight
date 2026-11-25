mod memory;

pub use aether_data_contracts::repository::user_telegram_bindings::{
    validate_user_telegram_chat_id, CreateUserTelegramBindingRecord, StoredUserTelegramBinding,
    UserTelegramBindingReadRepository, UserTelegramBindingRepository,
    UserTelegramBindingWriteRepository, VerifyUserTelegramBindingRecord,
    USER_TELEGRAM_LINK_CODE_ALPHABET, USER_TELEGRAM_LINK_CODE_LEN,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxUserTelegramBindingRepository;
pub use memory::InMemoryUserTelegramBindingRepository;
