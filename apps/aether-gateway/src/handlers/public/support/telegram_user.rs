//! Per-user Telegram binding and delivery.
//!
//! `module.telegram_push.chat_id` reaches operators through one broadcast chat.
//! This module reaches end users instead: a user proves ownership of a chat by
//! sending the bot a one-time link code, and per-user notifications then go to
//! that chat. The round trip through the bot is what makes the destination
//! trustworthy — a chat id typed into a settings form proves nothing, because
//! anyone can name someone else's chat.

use super::support_auth::auth_session::resolve_authenticated_local_user;
use super::{
    build_auth_error_response, http, json, mark_sensitive_response_no_store, Body, Bytes,
    GatewayPublicRequestContext, IntoResponse, Json, Response,
};
use crate::clock::current_unix_secs;
use crate::handlers::shared::unix_secs_to_rfc3339;
use crate::important_notification::{
    resolve_user_telegram_notification, single_report, ImportantNotification,
    ImportantNotificationDeliveryReport,
};
use crate::state::AppState;
use crate::telegram_push::{
    generate_user_telegram_link_code, read_telegram_bot_token, read_telegram_bot_username,
    read_telegram_push_config, read_telegram_public_base_url, read_telegram_webhook_secret,
    register_telegram_webhook, render_telegram_notification_text, send_telegram_text,
    telegram_push_module_enabled, USER_TELEGRAM_LINK_CODE_TTL_SECS,
};
use crate::{headers::header_value_str, GatewayError};
use aether_data::repository::user_telegram_bindings::{
    validate_user_telegram_chat_id, CreateUserTelegramBindingRecord,
    VerifyUserTelegramBindingRecord,
};
use serde::Deserialize;
use tracing::warn;
use uuid::Uuid;

/// Telegram stamps the registered secret on every update with this header name.
const TELEGRAM_WEBHOOK_SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";
/// An update carrying a `/start` command is a few hundred bytes.
const MAX_TELEGRAM_WEBHOOK_BODY_BYTES: usize = 64 * 1024;
const TELEGRAM_BIND_INSTRUCTIONS: &str =
    "请打开 MoonLight 的「设置 → Telegram 绑定」页面，把页面上显示的绑定码发给我，格式：/start 绑定码";
const TELEGRAM_BIND_SUCCESS_MESSAGE: &str =
    "绑定成功！之后这个账号的通知会发送到这个会话。";
const TELEGRAM_BIND_UNKNOWN_CODE_MESSAGE: &str =
    "绑定码无效或已过期，请在 MoonLight 的设置页面重新生成。";
const TELEGRAM_BIND_CONSUMED_CODE_MESSAGE: &str =
    "绑定码已被使用或已过期，请在 MoonLight 的设置页面重新生成。";
const TELEGRAM_BIND_CONFLICT_MESSAGE: &str =
    "绑定失败：这个 Telegram 会话可能已绑定到其他账号，请先在那里解绑。";

pub(super) async fn maybe_build_local_telegram_user_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("telegram_user") {
        return None;
    }

    match decision.route_kind.as_deref() {
        Some("binding") if request_context.request_method == http::Method::GET => {
            Some(handle_binding_status(state, request_context, headers).await)
        }
        Some("binding") if request_context.request_method == http::Method::POST => {
            Some(handle_binding_create(state, request_context, headers).await)
        }
        Some("binding") if request_context.request_method == http::Method::DELETE => {
            Some(handle_binding_delete(state, request_context, headers).await)
        }
        Some("webhook") if request_context.request_method == http::Method::POST => {
            Some(handle_telegram_webhook(state, headers, request_body).await)
        }
        _ => Some(super::build_unhandled_public_support_response(
            request_context,
        )),
    }
}

async fn handle_binding_status(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let module_enabled = telegram_push_module_enabled(state).await.unwrap_or(false);
    let bot_username = read_telegram_bot_username(state).await.unwrap_or(None);
    let binding = match state.find_user_telegram_binding(&auth.user.id).await {
        Ok(binding) => binding,
        Err(err) => return telegram_user_error(err),
    };
    let now = current_unix_secs();
    let pending = binding
        .as_ref()
        .filter(|binding| binding.link_code_is_active(now));
    mark_sensitive_response_no_store(
        Json(json!({
            "module_enabled": module_enabled,
            "bot_username": bot_username,
            "bound": binding
                .as_ref()
                .is_some_and(|binding| binding.is_verified()),
            "chat_title": binding
                .as_ref()
                .and_then(|binding| binding.chat_title.clone()),
            "username": binding
                .as_ref()
                .and_then(|binding| binding.username.clone()),
            "verified_at": binding
                .as_ref()
                .and_then(|binding| binding.verified_at_unix_secs)
                .and_then(unix_secs_to_rfc3339),
            "last_notified_at": binding
                .as_ref()
                .and_then(|binding| binding.last_notified_at_unix_secs)
                .and_then(unix_secs_to_rfc3339),
            "notification_count": binding
                .as_ref()
                .map(|binding| binding.notification_count)
                .unwrap_or(0),
            "pending_code": pending.map(|binding| binding.link_code.clone()),
            "pending_expires_at": pending
                .and_then(|binding| unix_secs_to_rfc3339(binding.link_code_expires_at_unix_secs)),
        }))
        .into_response(),
    )
}

async fn handle_binding_create(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !telegram_push_module_enabled(state).await.unwrap_or(false) {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "管理员尚未启用 Telegram 通知通道",
            false,
        );
    }
    let Some(bot_token) = read_telegram_bot_token(state).await.unwrap_or(None) else {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "管理员尚未配置 Telegram Bot Token",
            false,
        );
    };
    // Telegram must be able to call back into this instance, and that address is
    // not derivable from the request: the reverse proxy in front terminates TLS
    // and rewrites the host.
    match read_telegram_public_base_url(state).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "管理员尚未配置 Telegram 外部访问地址，暂时无法绑定",
                false,
            )
        }
        Err(err) => return telegram_user_error(err),
    }
    let bot_username = read_telegram_bot_username(state).await.unwrap_or(None);
    let now = current_unix_secs();
    let record = CreateUserTelegramBindingRecord {
        id: format!("tg-{}", Uuid::new_v4()),
        user_id: auth.user.id.clone(),
        link_code: generate_user_telegram_link_code(),
        link_code_expires_at_unix_secs: now + USER_TELEGRAM_LINK_CODE_TTL_SECS,
    };
    let stored = match state.create_user_telegram_binding(record).await {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "Telegram 绑定存储不可用",
                false,
            )
        }
        Err(err) => return telegram_user_error(err),
    };
    // Without a registered webhook the `/start` message never arrives, so the code
    // is useless. Registration is idempotent, and a failure here is worth
    // reporting rather than hiding: the user would otherwise wait forever.
    let webhook_ready = match register_telegram_webhook(state, &bot_token).await {
        Ok(()) => true,
        Err(err) => {
            warn!(
                error = %crate::error::redact_error_debug(&err),
                "failed to register telegram webhook"
            );
            false
        }
    };
    let deep_link = bot_username.as_deref().map(|username| {
        format!("https://t.me/{username}?start={}", stored.link_code)
    });
    mark_sensitive_response_no_store(
        Json(json!({
            "code": stored.link_code,
            "expires_at": unix_secs_to_rfc3339(stored.link_code_expires_at_unix_secs),
            "bot_username": bot_username,
            "deep_link": deep_link,
            "webhook_ready": webhook_ready,
        }))
        .into_response(),
    )
}

async fn handle_binding_delete(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.delete_user_telegram_binding(&auth.user.id).await {
        Ok(true) => Json(json!({ "message": "已解除 Telegram 绑定" })).into_response(),
        Ok(false) => {
            build_auth_error_response(http::StatusCode::NOT_FOUND, "尚未绑定 Telegram", false)
        }
        Err(err) => telegram_user_error(err),
    }
}

/// Send a per-user notification over Telegram.
///
/// Mirrors `send_user_important_notification_email`: the item's own templates and
/// per-item toggle decide whether anything is sent, and the destination comes
/// from the binding the user completed rather than from a caller-supplied value.
pub(crate) async fn send_user_telegram_notification(
    state: &AppState,
    item_key: &str,
    user_id: &str,
    notification: ImportantNotification,
    variables: &[(&str, String)],
) -> Result<ImportantNotificationDeliveryReport, GatewayError> {
    let Some(notification) =
        resolve_user_telegram_notification(state, item_key, notification, variables).await?
    else {
        return Ok(single_report(
            "user_telegram",
            false,
            "通知项未启用用户 Telegram 通知",
        ));
    };
    let Some(binding) = state.find_user_telegram_binding(user_id).await? else {
        return Ok(single_report("user_telegram", false, "用户未绑定 Telegram"));
    };
    let Some(chat_id) = binding
        .chat_id
        .as_deref()
        .filter(|_| binding.is_verified())
        .map(str::to_string)
    else {
        return Ok(single_report("user_telegram", false, "用户 Telegram 绑定未完成"));
    };
    let Some(bot_token) = read_telegram_bot_token(state).await? else {
        return Ok(single_report(
            "user_telegram",
            false,
            "未配置 Telegram Bot Token",
        ));
    };
    let config = read_telegram_push_config(state).await?;
    let text = render_telegram_notification_text(
        config.template.as_deref(),
        &notification.title,
        &notification.markdown_body,
    )?;
    match send_telegram_text(&bot_token, &chat_id, &text).await {
        Ok(()) => {
            if let Err(err) = state
                .record_user_telegram_notification(user_id, current_unix_secs())
                .await
            {
                warn!(
                    error = %crate::error::redact_error_debug(&err),
                    "failed to record user telegram notification"
                );
            }
            Ok(single_report(
                "user_telegram",
                true,
                "用户 Telegram 通知已发送",
            ))
        }
        Err(err) => {
            warn!(
                error = %crate::error::redact_error_debug(&err),
                user_id = %user_id,
                "failed to send user telegram notification"
            );
            Ok(single_report(
                "user_telegram",
                false,
                format!("用户 Telegram 通知发送失败: {err:?}"),
            ))
        }
    }
}

/// Telegram posts every inbound update here.
///
/// The only authentication is the shared secret registered through `setWebhook`
/// and echoed back in `X-Telegram-Bot-Api-Secret-Token`; the route carries no
/// user credential because Telegram is not a user. Every rejection answers 200
/// except a secret mismatch, because a 2xx is what stops Telegram from
/// redelivering a message that will never succeed.
async fn handle_telegram_webhook(
    state: &AppState,
    headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Response<Body> {
    let Some(expected) = read_telegram_webhook_secret(state).await.unwrap_or(None) else {
        // No secret means no webhook was ever registered, so this cannot be a
        // delivery Telegram made on our behalf.
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "Telegram Webhook 尚未注册",
            false,
        );
    };
    if !telegram_webhook_secret_matches(
        header_value_str(headers, TELEGRAM_WEBHOOK_SECRET_HEADER).as_deref(),
        &expected,
    ) {
        return build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "Telegram Webhook 校验失败",
            false,
        );
    }
    let Some(bot_token) = read_telegram_bot_token(state).await.unwrap_or(None) else {
        return webhook_ack();
    };
    let Some(body) = request_body.filter(|body| !body.is_empty()) else {
        return webhook_ack();
    };
    if body.len() > MAX_TELEGRAM_WEBHOOK_BODY_BYTES {
        return webhook_ack();
    }
    let Ok(update) = serde_json::from_slice::<TelegramWebhookUpdate>(body) else {
        return webhook_ack();
    };
    let Some(message) = update.message else {
        return webhook_ack();
    };
    let Some(chat) = message.chat else {
        return webhook_ack();
    };
    let Some(chat_id) = chat.id.map(|id| id.to_string()) else {
        return webhook_ack();
    };
    if validate_user_telegram_chat_id(&chat_id).is_err() {
        return webhook_ack();
    }
    let username = message
        .from
        .as_ref()
        .and_then(|from| from.username.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let chat_title = chat
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let chat_type = chat.chat_type.as_deref().map(ToOwned::to_owned);

    let Some(code) = message.text.as_deref().and_then(parse_telegram_link_code) else {
        // Chatter, not a binding attempt. Answer with instructions so a user who
        // opened the bot by hand knows what to do next.
        let _ = send_telegram_text(&bot_token, &chat_id, TELEGRAM_BIND_INSTRUCTIONS).await;
        return webhook_ack();
    };

    let now = current_unix_secs();
    match state
        .find_user_telegram_binding_by_active_link_code(code, now)
        .await
    {
        // The lookup already filters on "unverified and unexpired", so reaching
        // this point is what authorizes writing a chat id onto the row.
        Ok(Some(_)) => {}
        Ok(None) => {
            // An unknown, expired or already-consumed code is indistinguishable
            // from here, so the reply cannot say which one it was.
            let _ = send_telegram_text(&bot_token, &chat_id, TELEGRAM_BIND_UNKNOWN_CODE_MESSAGE)
                .await;
            return webhook_ack();
        }
        Err(err) => {
            warn!(
                error = %crate::error::redact_error_debug(&err),
                "failed to look up telegram link code"
            );
            return webhook_ack();
        }
    }

    let record = VerifyUserTelegramBindingRecord {
        link_code: code.to_string(),
        // Rotating the code on success is what makes it single-use: a replay finds
        // no row at all instead of re-binding the chat.
        rotated_link_code: generate_user_telegram_link_code(),
        rotated_link_code_expires_at_unix_secs: now + USER_TELEGRAM_LINK_CODE_TTL_SECS,
        chat_id: chat_id.clone(),
        chat_type,
        chat_title,
        username,
        verified_at_unix_secs: now,
    };
    match state.verify_user_telegram_binding(record).await {
        Ok(Some(_)) => {
            let _ = send_telegram_text(&bot_token, &chat_id, TELEGRAM_BIND_SUCCESS_MESSAGE).await;
        }
        Ok(None) => {
            let _ = send_telegram_text(&bot_token, &chat_id, TELEGRAM_BIND_CONSUMED_CODE_MESSAGE)
                .await;
        }
        Err(err) => {
            warn!(
                error = %crate::error::redact_error_debug(&err),
                "failed to verify telegram binding"
            );
            // The most likely cause is the unique constraint on a chat that is
            // already bound to a different account.
            let _ = send_telegram_text(&bot_token, &chat_id, TELEGRAM_BIND_CONFLICT_MESSAGE).await;
        }
    }
    webhook_ack()
}

/// Compare the presented secret in constant time. The secret is not a password
/// and this route is public, but a timing-safe compare costs nothing here.
fn telegram_webhook_secret_matches(presented: Option<&str>, expected: &str) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    if presented.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in presented.bytes().zip(expected.bytes()) {
        difference |= left ^ right;
    }
    difference == 0
}

/// Accepts `/start <code>`, `/start@<bot> <code>` and `/bind <code>`. Telegram
/// appends the bot handle in group chats, and a bare `/start` is a user opening
/// the bot by hand rather than completing a binding.
fn parse_telegram_link_code(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let rest = ["/start", "/bind"]
        .into_iter()
        .find_map(|command| trimmed.strip_prefix(command))
        .filter(|rest| {
            rest.is_empty() || rest.starts_with('@') || rest.starts_with(char::is_whitespace)
        })?;
    let rest = match rest.split_once(char::is_whitespace) {
        Some((first, rest)) if first.starts_with('@') => rest,
        Some((_, rest)) => rest,
        None => "",
    };
    let code = rest.trim();
    (!code.is_empty()).then_some(code)
}

/// Telegram has already delivered the update by the time this answers, so the
/// body is deliberately empty and always a success.
fn webhook_ack() -> Response<Body> {
    Json(json!({ "ok": true })).into_response()
}

fn telegram_user_error(error: GatewayError) -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("telegram binding failed: {error:?}"),
        false,
    )
}

#[derive(Deserialize)]
struct TelegramWebhookUpdate {
    message: Option<TelegramWebhookMessage>,
}

#[derive(Deserialize)]
struct TelegramWebhookMessage {
    text: Option<String>,
    chat: Option<TelegramWebhookChat>,
    from: Option<TelegramWebhookUser>,
}

#[derive(Deserialize)]
struct TelegramWebhookChat {
    /// Numeric id; negative for groups and channels.
    id: Option<i64>,
    #[serde(rename = "type")]
    chat_type: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct TelegramWebhookUser {
    username: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{parse_telegram_link_code, telegram_webhook_secret_matches};

    #[test]
    fn link_code_is_read_from_start_bind_and_group_forms() {
        assert_eq!(
            parse_telegram_link_code("/start ABCDEFGHJK"),
            Some("ABCDEFGHJK")
        );
        assert_eq!(
            parse_telegram_link_code("/start@moonlight_bot ABCDEFGHJK"),
            Some("ABCDEFGHJK")
        );
        assert_eq!(
            parse_telegram_link_code("/bind ABCDEFGHJK"),
            Some("ABCDEFGHJK")
        );
        assert_eq!(
            parse_telegram_link_code("  /start ABCDEFGHJK  "),
            Some("ABCDEFGHJK")
        );
    }

    #[test]
    fn link_code_is_absent_from_chatter_and_bare_commands() {
        for text in [
            "",
            "hello",
            "/start",
            "/start@moonlight_bot",
            "/help",
            "/startxyz ABCDEFGHJK",
            "/subscribe",
        ] {
            assert_eq!(parse_telegram_link_code(text), None, "{text}");
        }
    }

    #[test]
    fn webhook_secret_compare_is_length_aware() {
        assert!(telegram_webhook_secret_matches(Some("abc123"), "abc123"));
        assert!(!telegram_webhook_secret_matches(Some("abc124"), "abc123"));
        assert!(!telegram_webhook_secret_matches(Some("abc12"), "abc123"));
        assert!(!telegram_webhook_secret_matches(Some(""), "abc123"));
        assert!(!telegram_webhook_secret_matches(None, "abc123"));
    }
}
