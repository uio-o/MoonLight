use super::{
    json_object, ProviderOpsActionSpec, ProviderOpsArchitectureSpec, ProviderOpsAuthSpec,
    ProviderOpsBalanceMode, ProviderOpsCheckinMode, ProviderOpsVerifyMode,
};
use serde_json::{json, Map, Value};

pub(super) fn spec() -> ProviderOpsArchitectureSpec {
    let credentials_schema = json!({
        "type": "object",
        "properties": {
            "api_key": {
                "type": "string",
                "title": "API Key",
                "description": "中转站的 API 密钥，仅用于鉴权；查到的余额是账户级数据，与具体用哪把密钥无关",
                "x-sensitive": true,
                "x-input-type": "password"
            },
            "base_url": {
                "type": "string",
                "title": "站点地址",
                "description": "API 基础地址，需精确到提供余额接口的域名，例如 https://api.example.com"
            }
        },
        "required": ["api_key", "base_url"],
        "x-auth-method": "bearer",
        "x-auth-type": "api_key",
        "x-currency": "CNY",
        "x-field-groups": [
            { "fields": ["base_url"] },
            { "fields": ["api_key"] }
        ],
        "x-validation": [
            {
                "type": "required",
                "fields": ["api_key"],
                "message": "请填写 API Key"
            },
            {
                "type": "required",
                "fields": ["base_url"],
                "message": "请填写站点地址"
            }
        ]
    });

    ProviderOpsArchitectureSpec {
        architecture_id: "openai_billing",
        display_name: "OpenAI 计费（账户余额）",
        description: "OpenAI 兼容的 /dashboard/billing 接口：查询账户总额度与已用金额，余额为账户级，同一账户下所有密钥共用",
        hidden: false,
        credentials_schema: credentials_schema.clone(),
        verify_endpoint: "/v1/dashboard/billing/subscription",
        verify_mode: ProviderOpsVerifyMode::DirectGet,
        balance_mode: ProviderOpsBalanceMode::OpenAiBillingDualRequest,
        checkin_mode: ProviderOpsCheckinMode::None,
        query_balance_cookie_auth_errors: false,
        supported_auth_types: vec![ProviderOpsAuthSpec {
            auth_type: "api_key",
            display_name: "OpenAI Billing Key",
            credentials_schema,
        }],
        supported_actions: vec![ProviderOpsActionSpec {
            action_type: "query_balance",
            display_name: "查询账户余额",
            description: "分别请求总额度与已用金额接口，计算出账户可用余额",
            config_schema: json!({
                "type": "object",
                "properties": {
                    "subscription_endpoint": {
                        "type": "string",
                        "title": "总额度接口路径",
                        "description": "返回账户总额度的接口路径，读取 hard_limit_usd 字段",
                        "default": "/v1/dashboard/billing/subscription"
                    },
                    "usage_endpoint": {
                        "type": "string",
                        "title": "已用金额接口路径",
                        "description": "返回账户已用金额的接口路径，读取 total_usage 字段",
                        "default": "/v1/dashboard/billing/usage"
                    },
                    "usage_divisor": {
                        "type": "number",
                        "title": "已用金额除数",
                        "description": "将已用金额换算成元的除数；接口以「分」为单位时填 100",
                        "default": 100
                    },
                    "method": {
                        "type": "string",
                        "title": "请求方法",
                        "enum": ["GET", "POST"],
                        "default": "GET"
                    },
                    "currency": {
                        "type": "string",
                        "title": "货币单位",
                        "default": "CNY"
                    }
                },
                "required": []
            }),
        }],
        default_connector: Some("api_key"),
    }
}

pub(super) fn default_action_config(action_type: &str) -> Option<Map<String, Value>> {
    match action_type {
        "query_balance" => Some(json_object(json!({
            "subscription_endpoint": "/v1/dashboard/billing/subscription",
            "usage_endpoint": "/v1/dashboard/billing/usage",
            "usage_divisor": 100,
            "method": "GET",
            "currency": "CNY"
        }))),
        _ => None,
    }
}
