use super::request::{admin_provider_ops_execute_json_request, AdminProviderOpsExecuteJsonError};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{
    canonicalize_provider_ops_base_url, resolve_provider_ops_same_origin_url,
    ProviderOpsCanonicalDestination,
};
use aether_admin::provider::ops::{
    admin_provider_ops_openai_billing_balance_values, admin_provider_ops_verify_failure,
    admin_provider_ops_verify_success, admin_provider_ops_verify_user_payload_with_usage,
    build_headers, resolve_action_config, ProviderOpsArchitectureSpec,
};
use aether_contracts::ProxySnapshot;
use serde_json::{json, Map, Value};

const DEFAULT_SUBSCRIPTION_ENDPOINT: &str = "/v1/dashboard/billing/subscription";
const DEFAULT_USAGE_ENDPOINT: &str = "/v1/dashboard/billing/usage";

/// 计费接口不返回任何账户标识，验证结果里用一个固定标签代替 username/display_name。
const ACCOUNT_LABEL: &str = "OpenAI 计费账户";

/// OpenAI 兼容 `/dashboard/billing` 的验证。
///
/// 与其它架构不同，这里**必须发两个请求**：总额度在 subscription 接口，已用金额在
/// usage 接口，只请求其中一个既拿不到余额、也无法确认密钥对两个接口都有效。
///
/// 之所以不走通用的 `DirectGet` 单请求路径：subscription 的响应里没有
/// `username` / `quota` 字段，通用解析器会判为「验证响应缺少：用户信息、余额」；
/// 而即便只按 `hard_limit_usd` 补一个值，显示的也是总额度而非可用余额，
/// 会和「查询余额」动作的结果互相矛盾。
pub(super) async fn admin_provider_ops_local_openai_billing_verify_response(
    state: &AdminAppState<'_>,
    base_url: &str,
    architecture: &ProviderOpsArchitectureSpec,
    config: &Map<String, Value>,
    credentials: &Map<String, Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Value {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return admin_provider_ops_verify_failure("请提供 API 地址");
    }

    let action_config =
        resolve_action_config(architecture.architecture_id, config, "query_balance", None)
            .unwrap_or_default();

    let destination = match canonicalize_provider_ops_base_url(base_url) {
        Ok(destination) => destination,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };
    let subscription_url = match resolve_endpoint_url(
        &destination,
        &action_config,
        "subscription_endpoint",
        DEFAULT_SUBSCRIPTION_ENDPOINT,
    ) {
        Ok(url) => url,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };
    let usage_url = match resolve_endpoint_url(
        &destination,
        &action_config,
        "usage_endpoint",
        DEFAULT_USAGE_ENDPOINT,
    ) {
        Ok(url) => url,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };

    let headers = match build_headers(architecture.architecture_id, config, credentials) {
        Ok(headers) => headers,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };

    let (subscription_result, usage_result) = tokio::join!(
        admin_provider_ops_execute_json_request(
            state,
            "provider-ops-verify:openai_billing:subscription",
            reqwest::Method::GET,
            &subscription_url,
            &headers,
            None,
            proxy_snapshot,
        ),
        admin_provider_ops_execute_json_request(
            state,
            "provider-ops-verify:openai_billing:usage",
            reqwest::Method::GET,
            &usage_url,
            &headers,
            None,
            proxy_snapshot,
        ),
    );

    let subscription_json = match subscription_result {
        Ok((status, response_json)) => {
            if let Some(failure) = status_failure(status) {
                return failure;
            }
            response_json
        }
        Err(error) => return admin_provider_ops_verify_failure(transport_message(&error)),
    };
    let usage_json = match usage_result {
        Ok((status, response_json)) => {
            if let Some(failure) = status_failure(status) {
                return failure;
            }
            response_json
        }
        Err(error) => return admin_provider_ops_verify_failure(transport_message(&error)),
    };

    let (total_granted, total_used, available, currency) =
        match admin_provider_ops_openai_billing_balance_values(
            &action_config,
            &subscription_json,
            &usage_json,
        ) {
            Ok(values) => values,
            Err(message) => return admin_provider_ops_verify_failure(message),
        };

    let mut extra = Map::new();
    extra.insert("hard_limit_usd".to_string(), json!(total_granted));
    extra.insert("total_used".to_string(), json!(total_used));
    extra.insert("currency".to_string(), Value::String(currency));
    // 标记这是账户级余额，前端与运维都能据此与按密钥计费的渠道区分开。
    extra.insert("account_level".to_string(), Value::Bool(true));

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload_with_usage(
            Some(ACCOUNT_LABEL.to_string()),
            Some(ACCOUNT_LABEL.to_string()),
            None,
            Some(available),
            Some(total_used),
            None,
            Some(extra),
        ),
        None,
    )
}

fn resolve_endpoint_url(
    destination: &ProviderOpsCanonicalDestination,
    action_config: &Map<String, Value>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    let endpoint = action_config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default);
    resolve_provider_ops_same_origin_url(destination, endpoint).map_err(str::to_string)
}

fn status_failure(status: http::StatusCode) -> Option<Value> {
    match status {
        http::StatusCode::UNAUTHORIZED => Some(admin_provider_ops_verify_failure(
            "认证失败：API Key 无效",
        )),
        http::StatusCode::FORBIDDEN => Some(admin_provider_ops_verify_failure(
            "认证失败：API Key 无权查询计费信息",
        )),
        http::StatusCode::NOT_FOUND => Some(admin_provider_ops_verify_failure(
            "计费接口不存在，请检查接口路径配置",
        )),
        http::StatusCode::TOO_MANY_REQUESTS => {
            Some(admin_provider_ops_verify_failure("请求频率限制"))
        }
        status if status.is_success() => None,
        status => Some(admin_provider_ops_verify_failure(format!(
            "验证失败：HTTP {}",
            status.as_u16()
        ))),
    }
}

fn transport_message(error: &AdminProviderOpsExecuteJsonError) -> String {
    let raw = match error {
        AdminProviderOpsExecuteJsonError::InvalidJson(message)
        | AdminProviderOpsExecuteJsonError::Transport(message) => message.as_str(),
    };
    let normalized = raw.trim();
    if normalized.to_ascii_lowercase().contains("timeout") || normalized.contains("超时") {
        "请求超时".to_string()
    } else {
        "网络错误，无法连接计费接口".to_string()
    }
}
