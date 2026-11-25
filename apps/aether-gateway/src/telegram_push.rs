use crate::handlers::shared::{
    decrypt_or_migrate_system_config_secret, encrypt_system_config_secret, system_config_bool,
    system_config_string,
};
use crate::{AppState, GatewayError};
use aether_data::repository::user_telegram_bindings::{
    USER_TELEGRAM_LINK_CODE_ALPHABET, USER_TELEGRAM_LINK_CODE_LEN,
};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

pub(crate) const TELEGRAM_PUSH_ENABLED_KEY: &str = "module.telegram_push.enabled";
pub(crate) const TELEGRAM_PUSH_BOT_TOKEN_KEY: &str = "module.telegram_push.bot_token";
pub(crate) const TELEGRAM_PUSH_BOT_USERNAME_KEY: &str = "module.telegram_push.bot_username";
pub(crate) const TELEGRAM_PUSH_CHAT_ID_KEY: &str = "module.telegram_push.chat_id";
pub(crate) const TELEGRAM_PUSH_PUBLIC_URL_KEY: &str = "module.telegram_push.public_url";
pub(crate) const TELEGRAM_PUSH_TEMPLATE_KEY: &str = "module.telegram_push.template";
pub(crate) const TELEGRAM_PUSH_WEBHOOK_SECRET_KEY: &str = "module.telegram_push.webhook_secret";

// The Bot API endpoint is fixed. Telegram has no self-hosted mode worth
// supporting here, and pinning the host keeps the request path (which carries
// the bot token) from being steered somewhere else by configuration.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const TELEGRAM_API_HOST: &str = "api.telegram.org";
const TELEGRAM_API_PORT: u16 = 443;
const TELEGRAM_DNS_TIMEOUT: Duration = Duration::from_secs(10);
const TELEGRAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PUSH_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TELEGRAM_BOT_TOKEN_BYTES: usize = 256;
const MAX_TELEGRAM_CHAT_ID_BYTES: usize = 64;
const MAX_TELEGRAM_TEMPLATE_BYTES: usize = 256 * 1024;
const MAX_TELEGRAM_TITLE_BYTES: usize = 512;
const MAX_TELEGRAM_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_TELEGRAM_RENDERED_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Telegram rejects `sendMessage` payloads over 4096 UTF-16 code units.
const MAX_TELEGRAM_MESSAGE_UNITS: usize = 4096;
/// Kept as a `char` so its UTF-16 length comes from `char::len_utf16`;
/// `str` has no `len_utf16` in stable Rust.
const TELEGRAM_TRUNCATION_SUFFIX: char = '…';
const MAX_TELEGRAM_PUBLIC_URL_BYTES: usize = 512;
const MAX_TELEGRAM_WEBHOOK_SECRET_BYTES: usize = 256;
const MAX_TELEGRAM_BOT_USERNAME_BYTES: usize = 64;
/// Where Telegram posts inbound updates. Kept next to the API base so the
/// externally advertised URL and the locally mounted route cannot drift apart.
const TELEGRAM_WEBHOOK_PATH: &str = "/api/telegram/webhook";
/// How long a minted link code stays redeemable. Short enough that a code seen
/// over a shoulder expires before it can be used elsewhere.
pub(crate) const USER_TELEGRAM_LINK_CODE_TTL_SECS: u64 = 15 * 60;

#[derive(Clone)]
pub(crate) struct TelegramPushConfig {
    pub(crate) enabled: bool,
    pub(crate) bot_token: Option<String>,
    pub(crate) chat_id: Option<String>,
    pub(crate) template: Option<String>,
}

impl std::fmt::Debug for TelegramPushConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelegramPushConfig")
            .field("enabled", &self.enabled)
            .field("bot_token", &self.bot_token.as_ref().map(|_| "[REDACTED]"))
            .field("chat_id", &self.chat_id)
            .field("template", &self.template.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub(crate) async fn telegram_push_module_enabled(state: &AppState) -> Result<bool, GatewayError> {
    let value = state
        .read_system_config_json_value(TELEGRAM_PUSH_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(value.as_ref(), false))
}

pub(crate) async fn telegram_push_configured(state: &AppState) -> Result<bool, GatewayError> {
    let config = read_telegram_push_config(state).await?;
    Ok(config.bot_token.is_some() && config.chat_id.is_some())
}

pub(crate) async fn read_telegram_push_config(
    state: &AppState,
) -> Result<TelegramPushConfig, GatewayError> {
    let enabled = telegram_push_module_enabled(state).await?;
    let bot_token = read_telegram_bot_token(state).await?;
    let chat_id = state
        .read_system_config_json_value(TELEGRAM_PUSH_CHAT_ID_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let template = state
        .read_system_config_json_value(TELEGRAM_PUSH_TEMPLATE_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)));
    if let Some(template) = template.as_deref() {
        validate_telegram_field("template", template, MAX_TELEGRAM_TEMPLATE_BYTES)?;
    }

    Ok(TelegramPushConfig {
        enabled,
        bot_token,
        chat_id,
        template,
    })
}

/// The bot token is stored as an encrypted system-config secret, so it is read
/// back through the same decrypt-or-migrate path the other push channels use.
pub(crate) async fn read_telegram_bot_token(
    state: &AppState,
) -> Result<Option<String>, GatewayError> {
    let Some(value) = state
        .read_system_config_json_value(TELEGRAM_PUSH_BOT_TOKEN_KEY)
        .await?
    else {
        return Ok(None);
    };
    let Some(value) = system_config_string(Some(&value)) else {
        return Ok(None);
    };
    let value =
        decrypt_or_migrate_system_config_secret(state, TELEGRAM_PUSH_BOT_TOKEN_KEY, value).await?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    validate_telegram_field("bot_token", &value, MAX_TELEGRAM_BOT_TOKEN_BYTES)?;
    Ok(Some(value))
}

/// Telegram must reach the webhook from the public internet. This deployment
/// terminates TLS at a reverse proxy and has no `PUBLIC_URL` setting, so the
/// operator records the external address here instead of in the environment.
pub(crate) async fn read_telegram_public_base_url(
    state: &AppState,
) -> Result<Option<String>, GatewayError> {
    let Some(value) = state
        .read_system_config_json_value(TELEGRAM_PUSH_PUBLIC_URL_KEY)
        .await?
    else {
        return Ok(None);
    };
    let Some(value) = system_config_string(Some(&value)) else {
        return Ok(None);
    };
    match normalize_telegram_public_base_url(&value) {
        // A blank value is the same as unconfigured, never an empty base URL.
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        // A malformed address is an operator mistake worth surfacing on the
        // settings page rather than silently disabling per-user binding.
        Err(message) => Err(GatewayError::Internal(message.to_string())),
    }
}

/// Canonicalize an externally advertised base URL. Telegram refuses plain HTTP
/// and loopback hosts, and credentials must never end up in a URL Telegram
/// stores and replays on every update.
pub(crate) fn normalize_telegram_public_base_url(raw: &str) -> Result<String, &'static str> {
    let value = raw.trim().trim_end_matches('/');
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.len() > MAX_TELEGRAM_PUBLIC_URL_BYTES {
        return Err("Telegram 外部访问地址过长");
    }
    let parsed = url::Url::parse(value).map_err(|_| "Telegram 外部访问地址格式无效")?;
    if parsed.scheme() != "https" {
        return Err("Telegram 外部访问地址必须使用 https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Telegram 外部访问地址不能包含账号或密码");
    }
    let host = parsed.host_str().ok_or("Telegram 外部访问地址缺少主机名")?;
    if host.eq_ignore_ascii_case("localhost")
        || matches!(host, "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
    {
        return Err("Telegram 外部访问地址不能是本机回环地址");
    }
    Ok(value.to_string())
}

fn telegram_webhook_url(public_base_url: &str) -> String {
    format!("{public_base_url}{TELEGRAM_WEBHOOK_PATH}")
}

/// Shared secret stamped onto the webhook by `setWebhook` and echoed back by
/// Telegram on every update. Without it the receiver would accept callbacks from
/// anyone who learns the path, and a guessed link code could then be claimed
/// from a foreign chat.
pub(crate) async fn read_telegram_webhook_secret(
    state: &AppState,
) -> Result<Option<String>, GatewayError> {
    let Some(value) = state
        .read_system_config_json_value(TELEGRAM_PUSH_WEBHOOK_SECRET_KEY)
        .await?
    else {
        return Ok(None);
    };
    let Some(value) = system_config_string(Some(&value)) else {
        return Ok(None);
    };
    let value =
        decrypt_or_migrate_system_config_secret(state, TELEGRAM_PUSH_WEBHOOK_SECRET_KEY, value)
            .await?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    validate_telegram_field("webhook_secret", &value, MAX_TELEGRAM_WEBHOOK_SECRET_BYTES)?;
    Ok(Some(value))
}

/// Mint the webhook secret on first use and store it encrypted, so the receiver
/// is never left accepting unauthenticated callbacks. Re-registering the webhook
/// reuses the stored secret, which is what makes repeated `setWebhook` calls
/// safe to issue.
async fn ensure_telegram_webhook_secret(state: &AppState) -> Result<String, GatewayError> {
    if let Some(secret) = read_telegram_webhook_secret(state).await? {
        return Ok(secret);
    }
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let stored = encrypt_system_config_secret(state, TELEGRAM_PUSH_WEBHOOK_SECRET_KEY, &secret)
        .ok_or_else(|| GatewayError::Internal("Telegram Webhook 密钥加密不可用".to_string()))?;
    state
        .upsert_system_config_json_value(
            TELEGRAM_PUSH_WEBHOOK_SECRET_KEY,
            &json!(stored),
            Some("Telegram Webhook 校验密钥"),
        )
        .await?;
    Ok(secret)
}

pub(crate) async fn read_telegram_bot_username(
    state: &AppState,
) -> Result<Option<String>, GatewayError> {
    let Some(value) = state
        .read_system_config_json_value(TELEGRAM_PUSH_BOT_USERNAME_KEY)
        .await?
    else {
        return Ok(None);
    };
    let Some(value) = system_config_string(Some(&value)) else {
        return Ok(None);
    };
    match normalize_telegram_bot_username(&value) {
        Some(value) => Ok(Some(value)),
        None => Err(GatewayError::Internal(
            "Telegram Bot Username 格式无效".to_string(),
        )),
    }
}

/// Accept both `@moonlight_bot` and `moonlight_bot`; the value only ever feeds a
/// `https://t.me/<username>` deep link, so it is validated as a handle.
pub(crate) fn normalize_telegram_bot_username(raw: &str) -> Option<String> {
    let value = raw.trim().trim_start_matches('@');
    if value.is_empty()
        || value.len() > MAX_TELEGRAM_BOT_USERNAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
    {
        return None;
    }
    Some(value.to_string())
}

/// Point Telegram at this instance. Registration is idempotent, so it is safe to
/// re-issue whenever a link code is minted: the operator may have re-deployed
/// behind a new external address, and Telegram silently discards updates for a
/// webhook that no longer resolves.
pub(crate) async fn register_telegram_webhook(
    state: &AppState,
    bot_token: &str,
) -> Result<(), GatewayError> {
    let Some(public_base_url) = read_telegram_public_base_url(state).await? else {
        return Err(GatewayError::Internal(
            "未配置 Telegram 外部访问地址，无法接收用户绑定指令".to_string(),
        ));
    };
    let secret = ensure_telegram_webhook_secret(state).await?;
    let payload = json!({
        "url": telegram_webhook_url(&public_base_url),
        "allowed_updates": ["message"],
        "drop_pending_updates": false,
        "secret_token": secret,
    });
    telegram_bot_api_call(bot_token, "setWebhook", payload).await?;
    Ok(())
}

/// One Bot API round trip. Callers must have validated the token with
/// `is_safe_telegram_bot_token` first, because it is interpolated into the path.
async fn telegram_bot_api_call(
    bot_token: &str,
    method: &str,
    payload: Value,
) -> Result<Value, GatewayError> {
    let client = build_pinned_telegram_client()
        .await
        .map_err(|message| GatewayError::Internal(message.to_string()))?;
    let url = format!("{TELEGRAM_API_BASE}/bot{bot_token}/{method}");
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| GatewayError::Internal(telegram_request_error_message(&err)))?;
    let status = response.status();
    let body = aether_http::read_response_bytes_with_limit(response, MAX_PUSH_RESPONSE_BYTES)
        .await
        .map_err(|err| GatewayError::Internal(telegram_response_body_error_message(&err)))?;
    let body = String::from_utf8_lossy(&body);
    if !status.is_success() {
        return Err(GatewayError::Internal(telegram_api_failure_message(
            method, status, &body,
        )));
    }
    serde_json::from_str::<Value>(&body)
        .map_err(|_| GatewayError::Internal("Telegram 响应解析失败".to_string()))
}

/// Render a title/body pair through the configured template. Per-user
/// notifications reuse the administrator's template so both channels read alike.
pub(crate) fn render_telegram_notification_text(
    template: Option<&str>,
    title: &str,
    markdown_body: &str,
) -> Result<String, GatewayError> {
    render_telegram_message(template, title, markdown_body)
}

/// Human-transcribable one-time code. The alphabet drops `I`, `O`, `0` and `1`
/// so a code read off a screen cannot be mistyped into a different valid code.
pub(crate) fn generate_user_telegram_link_code() -> String {
    let alphabet = USER_TELEGRAM_LINK_CODE_ALPHABET.as_bytes();
    let mut entropy = Uuid::new_v4().as_u128();
    let mut code = String::with_capacity(USER_TELEGRAM_LINK_CODE_LEN);
    while code.len() < USER_TELEGRAM_LINK_CODE_LEN {
        // Consume the low five bits at a time: an unbiased slice of the 122
        // random bits a v4 UUID carries, with no modulo bias.
        let index = (entropy & 0b1_1111) as usize;
        code.push(alphabet[index] as char);
        entropy >>= 5;
    }
    code
}

pub(crate) async fn send_telegram_push(
    _state: &AppState,
    config: &TelegramPushConfig,
    title: &str,
    markdown_body: &str,
) -> Result<(), GatewayError> {
    let Some(bot_token) = config.bot_token.as_deref() else {
        return Err(GatewayError::Internal(
            "未配置 Telegram Bot Token".to_string(),
        ));
    };
    let bot_token = bot_token.trim();
    if bot_token.is_empty() {
        return Err(GatewayError::Internal(
            "Telegram Bot Token 不能为空".to_string(),
        ));
    }
    if !is_safe_telegram_bot_token(bot_token) {
        return Err(GatewayError::Internal(
            "Telegram Bot Token 格式无效".to_string(),
        ));
    }
    let Some(chat_id) = config.chat_id.as_deref() else {
        return Err(GatewayError::Internal(
            "未配置 Telegram Chat ID".to_string(),
        ));
    };
    let chat_id = chat_id.trim();
    if chat_id.is_empty() {
        return Err(GatewayError::Internal(
            "Telegram Chat ID 不能为空".to_string(),
        ));
    }
    if !is_safe_telegram_chat_id(chat_id) {
        return Err(GatewayError::Internal(
            "Telegram Chat ID 格式无效".to_string(),
        ));
    }
    validate_telegram_field("title", title, MAX_TELEGRAM_TITLE_BYTES)?;
    validate_telegram_field("body", markdown_body, MAX_TELEGRAM_BODY_BYTES)?;
    let rendered = render_telegram_message(config.template.as_deref(), title, markdown_body)?;
    let client = build_pinned_telegram_client()
        .await
        .map_err(|message| GatewayError::Internal(message.to_string()))?;
    post_telegram_message(&client, bot_token, chat_id, &rendered).await
}

/// Send one already-rendered message to an explicit chat. Unlike
/// `send_telegram_push` this does not read any configuration: per-user
/// notifications and webhook replies both know their destination already, and
/// neither may borrow the administrator's broadcast chat.
pub(crate) async fn send_telegram_text(
    bot_token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), GatewayError> {
    if !is_safe_telegram_bot_token(bot_token) {
        return Err(GatewayError::Internal(
            "Telegram Bot Token 格式无效".to_string(),
        ));
    }
    if !is_safe_telegram_chat_id(chat_id) {
        return Err(GatewayError::Internal(
            "Telegram Chat ID 格式无效".to_string(),
        ));
    }
    validate_telegram_field("text", text, MAX_TELEGRAM_BODY_BYTES)?;
    let rendered = truncate_telegram_message(text.to_string());
    let client = build_pinned_telegram_client()
        .await
        .map_err(|message| GatewayError::Internal(message.to_string()))?;
    post_telegram_message(&client, bot_token, chat_id, &rendered).await
}

/// One `sendMessage` round trip with the markup fallback folded into a loop, so
/// the retry needs no boxing and no duplicated response handling.
async fn post_telegram_message(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: &str,
    rendered: &str,
) -> Result<(), GatewayError> {
    let url = format!("{TELEGRAM_API_BASE}/bot{bot_token}/sendMessage");
    let html_payload = json!({
        "chat_id": chat_id,
        "text": markdown_to_telegram_html(rendered),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    let plain_payload = json!({
        "chat_id": chat_id,
        "text": strip_inline_markdown(rendered),
        "disable_web_page_preview": true,
    });
    let mut payload = &html_payload;
    let mut html = true;
    loop {
        let response = client
            .post(&url)
            .json(payload)
            .send()
            .await
            .map_err(|err| GatewayError::Internal(telegram_request_error_message(&err)))?;
        let status = response.status();
        let body = aether_http::read_response_bytes_with_limit(response, MAX_PUSH_RESPONSE_BYTES)
            .await
            .map_err(|err| GatewayError::Internal(telegram_response_body_error_message(&err)))?;
        let body = String::from_utf8_lossy(&body);
        if status.is_success() {
            return Ok(());
        }

        // Telegram discards the whole message when HTML parsing fails, so a
        // single stray character in a hand-written template would otherwise kill
        // the channel for every future notification. Retry once as plain text.
        if html && status == http::StatusCode::BAD_REQUEST && telegram_error_is_parse_failure(&body)
        {
            html = false;
            payload = &plain_payload;
            continue;
        }
        return Err(GatewayError::Internal(telegram_failure_message(
            status, &body,
        )));
    }
}

/// Build the production client with the API hostname pinned to one validated
/// DNS answer. Letting reqwest resolve the host again at connect time would
/// allow a rebinding attack to steer the bot token elsewhere.
async fn build_pinned_telegram_client() -> Result<reqwest::Client, &'static str> {
    let addresses = aether_http::lookup_host_with_limits(
        TELEGRAM_API_HOST,
        TELEGRAM_API_PORT,
        TELEGRAM_DNS_TIMEOUT,
    )
    .await
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::TimedOut => "Telegram API DNS resolution timed out",
        _ => "Telegram API DNS resolution failed",
    })?;
    if addresses.is_empty() {
        return Err("Telegram API DNS resolution returned no addresses");
    }
    if addresses
        .iter()
        .any(|address| aether_http::is_private_or_reserved_ip(address.ip()))
    {
        return Err("Telegram API resolved to a private or reserved address");
    }

    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(TELEGRAM_DNS_TIMEOUT)
        .timeout(TELEGRAM_REQUEST_TIMEOUT)
        .resolve_to_addrs(TELEGRAM_API_HOST, &addresses)
        .build()
        .map_err(|_| "Telegram HTTP client initialization failed")
}

/// The token is interpolated into the request path, so only path-safe
/// characters may appear. A `/`, `?`, `#` or `.` in the token could otherwise
/// redirect the request to another host or endpoint.
fn is_safe_telegram_bot_token(bot_token: &str) -> bool {
    let Some((id, secret)) = bot_token.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.len() <= 32
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && !secret.is_empty()
        && secret.len() <= 200
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Numeric ids (groups and channels are negative) or a public `@handle`.
fn is_safe_telegram_chat_id(chat_id: &str) -> bool {
    if let Some(handle) = chat_id.strip_prefix('@') {
        return !handle.is_empty()
            && handle.len() <= MAX_TELEGRAM_CHAT_ID_BYTES
            && handle
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    }
    let digits = chat_id.strip_prefix('-').unwrap_or(chat_id);
    !digits.is_empty() && digits.len() <= 32 && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_telegram_field(field: &str, value: &str, max_bytes: usize) -> Result<(), GatewayError> {
    if value.len() > max_bytes || value.bytes().any(|byte| byte == 0) {
        return Err(GatewayError::Internal(format!(
            "Telegram {field} exceeds the allowed size or contains a NUL byte"
        )));
    }
    Ok(())
}

/// Telegram has no separate title field, so the default layout stacks the two.
fn render_telegram_message(
    template: Option<&str>,
    title: &str,
    markdown_body: &str,
) -> Result<String, GatewayError> {
    validate_telegram_field("title", title, MAX_TELEGRAM_TITLE_BYTES)?;
    validate_telegram_field("body", markdown_body, MAX_TELEGRAM_BODY_BYTES)?;
    let template = template
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("{title}\n\n{body}");
    validate_telegram_field("template", template, MAX_TELEGRAM_TEMPLATE_BYTES)?;

    let mut rendered = String::with_capacity(template.len().min(MAX_TELEGRAM_RENDERED_BODY_BYTES));
    let mut cursor = 0usize;
    while cursor < template.len() {
        let remaining = &template[cursor..];
        let title_match = remaining.find("{title}");
        let body_match = remaining.find("{body}");
        let next = match (title_match, body_match) {
            (None, None) => {
                append_telegram_rendered_part(&mut rendered, remaining)?;
                cursor = template.len();
                continue;
            }
            (Some(index), None) => (index, "{title}", title),
            (None, Some(index)) => (index, "{body}", markdown_body),
            (Some(title_index), Some(body_index)) if title_index <= body_index => {
                (title_index, "{title}", title)
            }
            (Some(_), Some(body_index)) => (body_index, "{body}", markdown_body),
        };
        append_telegram_rendered_part(&mut rendered, &remaining[..next.0])?;
        append_telegram_rendered_part(&mut rendered, next.2)?;
        cursor += next.0 + next.1.len();
    }
    Ok(truncate_telegram_message(rendered))
}

fn append_telegram_rendered_part(output: &mut String, part: &str) -> Result<(), GatewayError> {
    let next_len = output
        .len()
        .checked_add(part.len())
        .ok_or_else(|| GatewayError::Internal("Telegram rendered body is too large".to_string()))?;
    if next_len > MAX_TELEGRAM_RENDERED_BODY_BYTES {
        return Err(GatewayError::Internal(
            "Telegram rendered body exceeds the allowed size".to_string(),
        ));
    }
    output.push_str(part);
    Ok(())
}

fn truncate_telegram_message(text: String) -> String {
    let budget = MAX_TELEGRAM_MESSAGE_UNITS.saturating_sub(TELEGRAM_TRUNCATION_SUFFIX.len_utf16());
    let mut units = 0usize;
    let mut cut = None;
    for (index, ch) in text.char_indices() {
        let next = units + ch.len_utf16();
        if next > budget {
            cut = Some(index);
            break;
        }
        units = next;
    }
    match cut {
        Some(index) => {
            let mut truncated = text[..index].to_string();
            truncated.push(TELEGRAM_TRUNCATION_SUFFIX);
            truncated
        }
        None => text,
    }
}

#[derive(Clone, Copy)]
enum TelegramInline {
    Bold,
    Code,
}

/// Minimal markdown to Telegram HTML. Only the constructs the built-in
/// notification templates actually use are translated; everything else is
/// passed through as escaped text, so an unknown construct degrades to
/// literal characters instead of failing the request.
fn markdown_to_telegram_html(markdown: &str) -> String {
    let mut output = String::with_capacity(markdown.len() + 16);
    let mut cursor = 0usize;
    while cursor < markdown.len() {
        let remaining = &markdown[cursor..];
        let bold_at = remaining.find("**");
        let code_at = remaining.find('`');
        let next = match (bold_at, code_at) {
            (Some(bold), Some(code)) if code < bold => Some((code, TelegramInline::Code)),
            (Some(bold), _) => Some((bold, TelegramInline::Bold)),
            (None, Some(code)) => Some((code, TelegramInline::Code)),
            (None, None) => None,
        };
        let Some((index, construct)) = next else {
            push_escaped_html_text(&mut output, remaining);
            break;
        };
        push_escaped_html_text(&mut output, &remaining[..index]);
        let (delimiter, tag) = match construct {
            TelegramInline::Bold => ("**", "b"),
            TelegramInline::Code => ("`", "code"),
        };
        let after_delimiter = &remaining[index + delimiter.len()..];
        match after_delimiter.find(delimiter) {
            Some(close) => {
                let inner = &after_delimiter[..close];
                output.push('<');
                output.push_str(tag);
                output.push('>');
                push_escaped_html_text(&mut output, inner);
                output.push_str("</");
                output.push_str(tag);
                output.push('>');
                cursor += index + delimiter.len() + close + delimiter.len();
            }
            None => {
                // Unterminated delimiter: emit it literally rather than
                // swallowing the rest of the message.
                push_escaped_html_text(&mut output, delimiter);
                cursor += index + delimiter.len();
            }
        }
    }
    output
}

fn push_escaped_html_text(output: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(ch),
        }
    }
}

/// Fallback body for the plain-text retry: drop the inline delimiters so the
/// message stays readable without any markup.
fn strip_inline_markdown(markdown: &str) -> String {
    markdown.replace("**", "").replace('`', "")
}

fn telegram_error_is_parse_failure(body: &str) -> bool {
    let Ok(payload) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    payload
        .get("description")
        .and_then(|value| value.as_str())
        .is_some_and(|description| {
            let description = description.to_ascii_lowercase();
            description.contains("parse") || description.contains("entit")
        })
}

/// Surface only Telegram's human-readable `description`, never the raw body:
/// parse errors echo the request path, which carries the bot token.
fn telegram_failure_message(status: http::StatusCode, body: &str) -> String {
    if let Some(description) = telegram_error_description(body) {
        return format!("Telegram 推送失败: {description}");
    }
    format!("Telegram 返回 HTTP {status}")
}

fn telegram_api_failure_message(method: &str, status: http::StatusCode, body: &str) -> String {
    if let Some(description) = telegram_error_description(body) {
        return format!("Telegram {method} 失败: {description}");
    }
    format!("Telegram {method} 返回 HTTP {status}")
}

/// Telegram's `description` is the only part of an error body that is safe to
/// show: the rest echoes the request path, which carries the bot token.
fn telegram_error_description(body: &str) -> Option<String> {
    let payload = serde_json::from_str::<Value>(body).ok()?;
    let description = payload
        .get("description")
        .and_then(|value| value.as_str())?;
    let description = description
        .strip_prefix("Bad Request:")
        .unwrap_or(description)
        .trim();
    (!description.is_empty()).then(|| description.chars().take(200).collect())
}

fn telegram_request_error_message(error: &reqwest::Error) -> String {
    format!("Telegram 请求失败 ({})", telegram_reqwest_error_kind(error))
}

fn telegram_response_body_error_message(error: &aether_http::ResponseBodyReadError) -> String {
    match error {
        aether_http::ResponseBodyReadError::TooLarge { max_bytes } => {
            format!("Telegram 响应超过 {max_bytes} 字节")
        }
        aether_http::ResponseBodyReadError::Read(error) => {
            format!(
                "Telegram 响应读取失败 ({})",
                telegram_reqwest_error_kind(error)
            )
        }
    }
}

fn telegram_reqwest_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "transport"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        generate_user_telegram_link_code, is_safe_telegram_bot_token, is_safe_telegram_chat_id,
        markdown_to_telegram_html, normalize_telegram_bot_username,
        normalize_telegram_public_base_url, render_telegram_message, strip_inline_markdown,
        telegram_api_failure_message, telegram_error_is_parse_failure, telegram_failure_message,
        telegram_webhook_url, truncate_telegram_message, USER_TELEGRAM_LINK_CODE_ALPHABET,
        USER_TELEGRAM_LINK_CODE_LEN,
    };

    #[test]
    fn telegram_bot_token_accepts_botfather_shape() {
        assert!(is_safe_telegram_bot_token(
            "123456789:AAHfjK9xQ2ZtLmNoPqRsTuVwXyZ0123456789"
        ));
    }

    #[test]
    fn telegram_bot_token_rejects_path_unsafe_values() {
        // The token is interpolated into the request path, so any character
        // that could redirect the request must be rejected outright.
        for invalid in [
            "",
            "123456789",
            ":AAHfjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:",
            "123456789:AAH/fjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:AAH?fjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:AAH#fjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:AAH.fjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "12a456789:AAHfjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:AAH fjK9xQ2ZtLmNoPqRsTuVwXyZ",
            "123456789:AAHfjK9xQ2ZtLmNoPqRsTuVwXyZ:extra",
        ] {
            assert!(!is_safe_telegram_bot_token(invalid), "{invalid}");
        }
    }

    #[test]
    fn telegram_chat_id_accepts_numeric_and_handle_forms() {
        for valid in ["123456789", "-1001234567890", "@moonlight_alerts"] {
            assert!(is_safe_telegram_chat_id(valid), "{valid}");
        }
    }

    #[test]
    fn telegram_chat_id_rejects_junk() {
        for invalid in [
            "",
            "-",
            "@",
            "@moon light",
            "12 345",
            "12345\n",
            "0x10",
            "+12345",
        ] {
            assert!(!is_safe_telegram_chat_id(invalid), "{invalid}");
        }
    }

    #[test]
    fn telegram_message_stacks_title_and_body_by_default() {
        let rendered = render_telegram_message(None, "号池异常", "请检查服务状态。")
            .expect("default template should render");
        assert_eq!(rendered, "号池异常\n\n请检查服务状态。");
    }

    #[test]
    fn telegram_message_honors_custom_template() {
        let rendered = render_telegram_message(Some("{body}"), "标题", "正文")
            .expect("custom template should render");
        assert_eq!(rendered, "正文");
    }

    #[test]
    fn telegram_message_falls_back_for_blank_template() {
        assert_eq!(
            render_telegram_message(Some("   "), "标题", "正文").expect("blank should render"),
            "标题\n\n正文"
        );
    }

    #[test]
    fn telegram_message_rejects_expansion_bombs_and_oversized_content() {
        let template = "x".repeat(super::MAX_TELEGRAM_TEMPLATE_BYTES + 1);
        assert!(render_telegram_message(Some(&template), "标题", "正文").is_err());
        let body = "x".repeat(super::MAX_TELEGRAM_BODY_BYTES + 1);
        assert!(render_telegram_message(None, "标题", &body).is_err());
    }

    #[test]
    fn telegram_message_truncates_to_the_api_unit_limit() {
        // U+1F600 counts as two UTF-16 code units, so a naive char count would
        // still be rejected by Telegram.
        let message = "😀".repeat(super::MAX_TELEGRAM_MESSAGE_UNITS);
        let truncated = truncate_telegram_message(message);
        assert!(truncated.encode_utf16().count() <= super::MAX_TELEGRAM_MESSAGE_UNITS);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn telegram_message_keeps_short_bodies_untouched() {
        let message = "短消息".to_string();
        assert_eq!(truncate_telegram_message(message.clone()), message);
    }

    #[test]
    fn telegram_html_converts_bold_and_code() {
        assert_eq!(
            markdown_to_telegram_html("号池 **异常** 请查看 `provider-1`"),
            "号池 <b>异常</b> 请查看 <code>provider-1</code>"
        );
    }

    #[test]
    fn telegram_html_escapes_markup_characters() {
        assert_eq!(
            markdown_to_telegram_html("<script>&</script>"),
            "&lt;script&gt;&amp;&lt;/script&gt;"
        );
    }

    #[test]
    fn telegram_html_emits_unterminated_delimiters_literally() {
        assert_eq!(markdown_to_telegram_html("**未闭合"), "**未闭合");
        assert_eq!(markdown_to_telegram_html("`未闭合"), "`未闭合");
        assert_eq!(markdown_to_telegram_html("**"), "**");
    }

    #[test]
    fn telegram_plain_fallback_drops_inline_delimiters() {
        assert_eq!(strip_inline_markdown("**粗体** 与 `代码`"), "粗体 与 代码");
    }

    #[test]
    fn telegram_parse_failure_is_detected_from_description() {
        assert!(telegram_error_is_parse_failure(
            r#"{"ok":false,"error_code":400,"description":"Bad Request: can't parse entities: Unsupported start tag"}"#
        ));
        assert!(telegram_error_is_parse_failure(
            r#"{"ok":false,"error_code":400,"description":"Bad Request: cannot find end of the entity"}"#
        ));
        assert!(!telegram_error_is_parse_failure(
            r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#
        ));
        assert!(!telegram_error_is_parse_failure("not json"));
    }

    #[test]
    fn telegram_failure_message_strips_prefix_and_drops_raw_body() {
        let message = telegram_failure_message(
            http::StatusCode::BAD_REQUEST,
            r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#,
        );
        assert_eq!(message, "Telegram 推送失败: chat not found");
        assert!(!message.contains("error_code"));
    }

    #[test]
    fn telegram_failure_message_falls_back_to_status() {
        let message = telegram_failure_message(http::StatusCode::BAD_GATEWAY, "<html>oops</html>");
        assert_eq!(message, "Telegram 返回 HTTP 502 Bad Gateway");
        assert!(!message.contains("html"));
    }

    #[test]
    fn telegram_api_failure_message_names_the_method() {
        let message = telegram_api_failure_message(
            "setWebhook",
            http::StatusCode::BAD_REQUEST,
            r#"{"ok":false,"description":"Bad Request: bad webhook: HTTPS url expected"}"#,
        );
        assert_eq!(
            message,
            "Telegram setWebhook 失败: bad webhook: HTTPS url expected"
        );
    }

    #[test]
    fn telegram_public_base_url_accepts_https_and_drops_the_trailing_slash() {
        assert_eq!(
            normalize_telegram_public_base_url(" https://ae.example.com/ ").expect("valid"),
            "https://ae.example.com"
        );
        assert_eq!(
            normalize_telegram_public_base_url("").expect("blank stays blank"),
            ""
        );
    }

    #[test]
    fn telegram_public_base_url_rejects_unreachable_or_unsafe_values() {
        for raw in [
            "http://ae.example.com",
            "ae.example.com",
            "https://user:secret@ae.example.com",
            "https://127.0.0.1",
            "https://localhost:8443",
            "https://[::1]",
            "not a url",
        ] {
            assert!(
                normalize_telegram_public_base_url(raw).is_err(),
                "{raw} should be rejected"
            );
        }
    }

    #[test]
    fn telegram_webhook_url_appends_the_receiver_path() {
        assert_eq!(
            telegram_webhook_url("https://ae.example.com"),
            "https://ae.example.com/api/telegram/webhook"
        );
    }

    #[test]
    fn telegram_bot_username_accepts_bare_and_prefixed_handles() {
        assert_eq!(
            normalize_telegram_bot_username(" @moonlight_bot "),
            Some("moonlight_bot".to_string())
        );
        assert_eq!(
            normalize_telegram_bot_username("moonlight_bot"),
            Some("moonlight_bot".to_string())
        );
        for raw in ["", "@", "@moon light", "@1bot", "@机器人", &"a".repeat(65)] {
            assert_eq!(normalize_telegram_bot_username(raw), None, "{raw}");
        }
    }

    #[test]
    fn user_telegram_link_code_uses_only_unambiguous_characters() {
        for _ in 0..200 {
            let code = generate_user_telegram_link_code();
            assert_eq!(code.len(), USER_TELEGRAM_LINK_CODE_LEN);
            assert!(code
                .bytes()
                .all(|byte| USER_TELEGRAM_LINK_CODE_ALPHABET.as_bytes().contains(&byte)));
        }
    }

    #[test]
    fn user_telegram_link_codes_do_not_repeat_across_a_large_sample() {
        let codes = (0..500)
            .map(|_| generate_user_telegram_link_code())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(codes.len(), 500);
    }
}
