use super::super::super::verify::{
    admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError,
};
use super::super::responses::{
    admin_provider_ops_action_error, admin_provider_ops_action_response,
};
use super::super::support::{
    admin_provider_ops_json_object_map, admin_provider_ops_request_method,
    admin_provider_ops_request_url,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::parse_openai_billing_balance_payload;
use aether_contracts::ProxySnapshot;
use serde_json::{json, Map, Value};

const DEFAULT_SUBSCRIPTION_ENDPOINT: &str = "/v1/dashboard/billing/subscription";
const DEFAULT_USAGE_ENDPOINT: &str = "/v1/dashboard/billing/usage";

/// OpenAI 兼容 `/dashboard/billing` 的账户余额查询。
///
/// 余额不在单个接口里：总额度来自 subscription 接口，已用金额来自 usage 接口，
/// 两者都拿到之后才能算出 `hard_limit_usd - total_usage / usage_divisor`。
/// 任一侧缺失或失败都直接报错，避免拿半个数据算出误导性的余额。
pub(super) async fn admin_provider_ops_openai_billing_balance_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    base_url: &str,
    action_config: &Map<String, Value>,
    headers: &reqwest::header::HeaderMap,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> serde_json::Value {
    let start = std::time::Instant::now();

    let subscription_url = match resolve_endpoint_url(
        base_url,
        action_config,
        "subscription_endpoint",
        DEFAULT_SUBSCRIPTION_ENDPOINT,
    ) {
        Ok(url) => url,
        Err(message) => {
            return admin_provider_ops_action_error(
                "not_configured",
                "query_balance",
                message,
                None,
            )
        }
    };
    let usage_url = match resolve_endpoint_url(
        base_url,
        action_config,
        "usage_endpoint",
        DEFAULT_USAGE_ENDPOINT,
    ) {
        Ok(url) => url,
        Err(message) => {
            return admin_provider_ops_action_error(
                "not_configured",
                "query_balance",
                message,
                None,
            )
        }
    };

    let method = admin_provider_ops_request_method(action_config, "GET");
    let subscription_request_id =
        format!("provider-ops-action:openai_billing:subscription:{provider_id}");
    let usage_request_id = format!("provider-ops-action:openai_billing:usage:{provider_id}");

    let (subscription_result, usage_result) = tokio::join!(
        admin_provider_ops_execute_json_request(
            state,
            &subscription_request_id,
            method.clone(),
            &subscription_url,
            headers,
            None,
            proxy_snapshot,
        ),
        admin_provider_ops_execute_json_request(
            state,
            &usage_request_id,
            method,
            &usage_url,
            headers,
            None,
            proxy_snapshot,
        )
    );
    let response_time_ms = Some(start.elapsed().as_millis() as u64);

    let (subscription_status, subscription_json) = match subscription_result {
        Ok(result) => result,
        Err(err) => {
            let message = match err {
                AdminProviderOpsExecuteJsonError::InvalidJson(message)
                | AdminProviderOpsExecuteJsonError::Transport(message) => message,
            };
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                network_error_message(&message),
                None,
            );
        }
    };
    if subscription_status != http::StatusCode::OK {
        return status_error(subscription_status, response_time_ms);
    }

    let (usage_status, usage_json) = match usage_result {
        Ok(result) => result,
        Err(err) => {
            let message = match err {
                AdminProviderOpsExecuteJsonError::InvalidJson(message)
                | AdminProviderOpsExecuteJsonError::Transport(message) => message,
            };
            return admin_provider_ops_action_error(
                "network_error",
                "query_balance",
                network_error_message(&message),
                None,
            );
        }
    };
    if usage_status != http::StatusCode::OK {
        return status_error(usage_status, response_time_ms);
    }

    let data = match parse_openai_billing_balance_payload(
        action_config,
        &subscription_json,
        &usage_json,
    ) {
        Ok(data) => data,
        Err(message) => {
            return admin_provider_ops_action_error(
                "parse_error",
                "query_balance",
                message,
                response_time_ms,
            )
        }
    };

    admin_provider_ops_action_response(
        "success",
        "query_balance",
        data,
        None,
        response_time_ms,
        86400,
    )
}

fn resolve_endpoint_url(
    base_url: &str,
    action_config: &Map<String, Value>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    let endpoint = action_config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string();
    let scoped_config = admin_provider_ops_json_object_map(json!({ "endpoint": endpoint }));
    admin_provider_ops_request_url(base_url, &scoped_config, default)
}

fn status_error(status: http::StatusCode, response_time_ms: Option<u64>) -> serde_json::Value {
    match status {
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN => {
            admin_provider_ops_action_error(
                "auth_failed",
                "query_balance",
                "认证失败，请检查 API Key 与站点地址",
                response_time_ms,
            )
        }
        http::StatusCode::NOT_FOUND => admin_provider_ops_action_error(
            "not_supported",
            "query_balance",
            "余额接口不存在，请检查接口路径配置",
            response_time_ms,
        ),
        http::StatusCode::TOO_MANY_REQUESTS => admin_provider_ops_action_error(
            "rate_limited",
            "query_balance",
            "请求频率限制",
            response_time_ms,
        ),
        _ => admin_provider_ops_action_error(
            "unknown_error",
            "query_balance",
            format!(
                "HTTP {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            ),
            response_time_ms,
        ),
    }
}

fn network_error_message(error: &str) -> String {
    let normalized = error.trim();
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("timeout") || normalized.contains("超时") {
        return "请求超时".to_string();
    }
    "网络错误".to_string()
}
