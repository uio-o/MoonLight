mod memory;

pub use aether_data_contracts::repository::user_telegram_bindings::{
    CreateUserTelegramBindingRecord, StoredUserTelegramBinding, UserTelegramBindingReadRepository,
    UserTelegramBindingRepository, UserTelegramBindingWriteRepository, VerifyUserTelegramBindingRecord,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxUserTelegramBindingRepository;
pub use memory::InMemoryUserTelegramBindingRepository;
