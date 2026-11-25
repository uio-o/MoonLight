use crate::handlers::shared::{
    decrypt_or_migrate_system_config_secret, system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) const TELEGRAM_PUSH_ENABLED_KEY: &str = "module.telegram_push.enabled";
pub(crate) const TELEGRAM_PUSH_BOT_TOKEN_KEY: &str = "module.telegram_push.bot_token";
pub(crate) const TELEGRAM_PUSH_CHAT_ID_KEY: &str = "module.telegram_push.chat_id";
pub(crate) const TELEGRAM_PUSH_TEMPLATE_KEY: &str = "module.telegram_push.template";

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
async fn read_telegram_bot_token(state: &AppState) -> Result<Option<String>, GatewayError> {
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
    let url = format!("{TELEGRAM_API_BASE}/bot{bot_token}/sendMessage");

    let response = client
        .post(&url)
        .json(&json!({
            "chat_id": chat_id,
            "text": markdown_to_telegram_html(&rendered),
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        }))
        .send()
        .await
        .map_err(|err| GatewayError::Internal(telegram_request_error_message(&err)))?;
    let status = response.status();
    let body = aether_http::read_response_bytes_with_limit(response, MAX_PUSH_RESPONSE_BYTES)
        .await
        .map_err(|err| GatewayError::Internal(telegram_response_body_error_message(&err)))?;
    let text = String::from_utf8_lossy(&body);
    if status.is_success() {
        return Ok(());
    }

    // Telegram discards the whole message when HTML parsing fails, so a single
    // stray character in a hand-written template would otherwise kill the
    // channel for every future notification. Retry once as plain text.
    if status == http::StatusCode::BAD_REQUEST && telegram_error_is_parse_failure(&text) {
        let response = client
            .post(&url)
            .json(&json!({
                "chat_id": chat_id,
                "text": strip_inline_markdown(&rendered),
                "disable_web_page_preview": true,
            }))
            .send()
            .await
            .map_err(|err| GatewayError::Internal(telegram_request_error_message(&err)))?;
        let retry_status = response.status();
        let retry_body =
            aether_http::read_response_bytes_with_limit(response, MAX_PUSH_RESPONSE_BYTES)
                .await
                .map_err(|err| {
                    GatewayError::Internal(telegram_response_body_error_message(&err))
                })?;
        let retry_text = String::from_utf8_lossy(&retry_body);
        if retry_status.is_success() {
            return Ok(());
        }
        let detail = telegram_failure_message(retry_status, &retry_text);
        return Err(GatewayError::Internal(detail));
    }
    let detail = telegram_failure_message(status, &text);
    Err(GatewayError::Internal(detail))
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
    if let Ok(payload) = serde_json::from_str::<Value>(body) {
        if let Some(description) = payload.get("description").and_then(|value| value.as_str()) {
            let description = description
                .strip_prefix("Bad Request:")
                .unwrap_or(description)
                .trim();
            if !description.is_empty() {
                let description: String = description.chars().take(200).collect();
                return format!("Telegram 推送失败: {description}");
            }
        }
    }
    format!("Telegram 返回 HTTP {status}")
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
        is_safe_telegram_bot_token, is_safe_telegram_chat_id, markdown_to_telegram_html,
        render_telegram_message, strip_inline_markdown, telegram_error_is_parse_failure,
        telegram_failure_message, truncate_telegram_message,
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
}
