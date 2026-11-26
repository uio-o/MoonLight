//! Display currency and the exchange-rate table that converts recorded
//! provider expenses into it.
//!
//! USD stays the accounting currency everywhere else in the system.  This
//! module only decides how a recorded expense is *labelled* and *rolled up*:
//! the amount as paid is never rewritten, and every converted value carries the
//! rate that produced it so a later rate edit cannot move history.

use crate::clock::current_unix_secs;
use crate::handlers::shared::system_config_string;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::billing::{
    convert_provider_expense_units, format_provider_expense_amount, provider_expense_amount_units,
    provider_expense_effective_rate, provider_expense_rate_units,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// Display currency for money an operator reads back.  USD keeps the current
/// behaviour, so an untouched install shows exactly what it showed before.
pub(crate) const DISPLAY_CURRENCY_KEY: &str = "ui.display_currency";
/// Rate table keyed by ISO code, quoted as units of that currency per one USD.
pub(crate) const FX_RATES_KEY: &str = "ui.fx_rates";

/// Free, keyless feed that republishes daily.  The endpoint is pinned rather
/// than configured so a mistyped override cannot silently break refreshes, and
/// because manual rates remain available as the fallback.
const FX_RATES_ENDPOINT: &str = "https://open.er-api.com/v6/latest/USD";
const FX_RATES_TIMEOUT: Duration = Duration::from_secs(10);
/// The feed returns ~170 currencies, which is well under this ceiling; the cap
/// exists so a hostile or broken endpoint cannot exhaust memory.
const FX_RATES_MAX_BODY_BYTES: usize = 1024 * 1024;
/// Anything older than the feed's own republish cadence is shown as stale.
const FX_RATES_MAX_AGE_SECS: u64 = 24 * 60 * 60;
/// Currencies offered in the picker.  The feed carries far more, but a list an
/// operator can scan beats a 170-entry dropdown.
pub(crate) const SUPPORTED_DISPLAY_CURRENCIES: [&str; 15] = [
    "USD", "CNY", "EUR", "JPY", "GBP", "HKD", "SGD", "KRW", "INR", "TWD", "AUD", "CAD", "RUB",
    "MYR", "THB",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FxRateSource {
    Live,
    Manual,
}

impl FxRateSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Manual => "manual",
        }
    }
    fn parse(value: &str) -> Self {
        if value == "live" {
            Self::Live
        } else {
            Self::Manual
        }
    }
}

/// A resolved rate table.  `rates` keeps the original text so an operator's
/// hand-typed value round-trips unchanged instead of being reformatted.
#[derive(Clone, Debug)]
pub(crate) struct FxRateTable {
    pub(crate) display_currency: String,
    pub(crate) base: String,
    pub(crate) rates: BTreeMap<String, String>,
    pub(crate) fetched_at_unix_secs: u64,
    pub(crate) source: FxRateSource,
}

impl FxRateTable {
    fn empty(display_currency: String) -> Self {
        let mut rates = BTreeMap::new();
        rates.insert("USD".to_string(), "1.00000000".to_string());
        Self {
            display_currency,
            base: "USD".to_string(),
            rates,
            fetched_at_unix_secs: 0,
            source: FxRateSource::Manual,
        }
    }

    /// Rate for `currency`, or `None` when the table does not cover it.  USD is
    /// always present because [`FxRateTable::empty`] seeds it.
    fn rate(&self, currency: &str) -> Option<&str> {
        self.rates.get(currency).map(String::as_str)
    }

    pub(crate) fn is_stale(&self, now_unix_secs: u64) -> bool {
        self.fetched_at_unix_secs == 0
            || now_unix_secs.saturating_sub(self.fetched_at_unix_secs) > FX_RATES_MAX_AGE_SECS
    }
}

/// A converted amount plus the rate that produced it, ready to persist.
#[derive(Clone, Debug)]
pub(crate) struct FxConversion {
    pub(crate) fx_rate_used: String,
    pub(crate) amount_base: String,
    pub(crate) base_currency: String,
}

/// The display currency, defaulting to USD when unset or unrecognised.  An
/// unrecognised value is ignored rather than rejected so a stale config left by
/// an older build cannot break the cost page.
pub(crate) async fn read_display_currency(state: &AppState) -> Result<String, GatewayError> {
    let stored = state
        .read_system_config_json_value(DISPLAY_CURRENCY_KEY)
        .await?
        .as_ref()
        .and_then(|value| system_config_string(Some(value)));
    Ok(stored
        .filter(|value| SUPPORTED_DISPLAY_CURRENCIES.contains(&value.as_str()))
        .unwrap_or_else(|| "USD".to_string()))
}

pub(crate) async fn read_fx_rate_table(state: &AppState) -> Result<FxRateTable, GatewayError> {
    let display_currency = read_display_currency(state).await?;
    let stored = state.read_system_config_json_value(FX_RATES_KEY).await?;
    Ok(parse_fx_rate_table(stored.as_ref(), display_currency))
}

/// Parse the stored rate table, discarding anything that does not round-trip
/// through the fixed-point helpers.  A partially valid table is still usable:
/// the currencies it does cover convert, the rest simply report no rate.
fn parse_fx_rate_table(stored: Option<&Value>, display_currency: String) -> FxRateTable {
    let mut table = FxRateTable::empty(display_currency);
    let Some(stored) = stored else {
        return table;
    };
    let Some(rates) = stored.get("rates").and_then(Value::as_object) else {
        return table;
    };
    for (code, value) in rates {
        let Some(text) = value.as_str() else { continue };
        let code = code.trim().to_ascii_uppercase();
        if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            continue;
        }
        // USD is the quote base, so its rate is fixed rather than stored.  A
        // hand-edited "USD" entry is ignored instead of silently breaking every
        // conversion that passes through it.
        if code == "USD" {
            continue;
        }
        if provider_expense_rate_units(text).is_none() {
            continue;
        }
        table.rates.insert(code, text.to_string());
    }
    if let Some(base) = stored.get("base").and_then(Value::as_str) {
        let base = base.trim().to_ascii_uppercase();
        if base.len() == 3 && base.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            table.base = base;
        }
    }
    table.fetched_at_unix_secs = stored
        .get("fetched_at_unix_secs")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    table.source = stored
        .get("source")
        .and_then(Value::as_str)
        .map(FxRateSource::parse)
        .unwrap_or(FxRateSource::Manual);
    table
}

/// Fetch the live table and persist it.  The previous table is left untouched
/// when the feed is unreachable or returns something unusable, so a failed
/// refresh degrades to "still using the last known rates".
pub(crate) async fn refresh_fx_rates(state: &AppState) -> Result<FxRateTable, GatewayError> {
    let display_currency = read_display_currency(state).await?;
    let (fetched_at, mut rates) = fetch_live_rates().await?;
    // USD is the quote base, so its rate is pinned rather than taken from the
    // feed: a wrong "USD" entry would corrupt every conversion through it.
    rates.insert("USD".to_string(), "1.00000000".to_string());
    let table = FxRateTable {
        display_currency,
        base: "USD".to_string(),
        rates,
        fetched_at_unix_secs: fetched_at,
        source: FxRateSource::Live,
    };
    state
        .upsert_system_config_json_value(FX_RATES_KEY, &fx_rate_table_json(&table), None)
        .await?;
    Ok(table)
}

async fn fetch_live_rates() -> Result<(u64, BTreeMap<String, String>), GatewayError> {
    let client = reqwest::Client::builder()
        .timeout(FX_RATES_TIMEOUT)
        .build()
        .map_err(|_| GatewayError::Internal("无法创建汇率请求客户端".to_string()))?;
    let response = client
        .get(FX_RATES_ENDPOINT)
        .send()
        .await
        .map_err(|_| GatewayError::Internal("汇率接口请求失败".to_string()))?;
    if !response.status().is_success() {
        return Err(GatewayError::Internal(format!(
            "汇率接口返回 HTTP {}",
            response.status().as_u16()
        )));
    }
    let body = aether_http::read_response_bytes_with_limit(response, FX_RATES_MAX_BODY_BYTES)
        .await
        .map_err(|err| GatewayError::Internal(format!("汇率响应读取失败: {err}")))?;
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|_| GatewayError::Internal("汇率响应解析失败".to_string()))?;
    if payload.get("result").and_then(Value::as_str) != Some("success") {
        return Err(GatewayError::Internal(
            "汇率接口未返回成功结果".to_string(),
        ));
    }
    let fetched_at = payload
        .get("time_last_update_unix")
        .and_then(Value::as_u64)
        .unwrap_or_else(current_unix_secs);
    let Some(rates) = payload.get("rates").and_then(Value::as_object) else {
        return Err(GatewayError::Internal("汇率响应缺少 rates 字段".to_string()));
    };
    let mut table = BTreeMap::new();
    for (code, value) in rates {
        let Some(number) = value.as_f64() else { continue };
        let code = code.trim().to_ascii_uppercase();
        if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            continue;
        }
        // Only the supported list is kept, so the picker and the table cannot
        // drift apart and an unused currency cannot widen the stored blob.
        if !SUPPORTED_DISPLAY_CURRENCIES.contains(&code.as_str()) {
            continue;
        }
        if !number.is_finite() || number <= 0.0 {
            continue;
        }
        table.insert(code, format_rate(number));
    }
    if table.is_empty() {
        return Err(GatewayError::Internal("汇率响应未包含可用币种".to_string()));
    }
    Ok((fetched_at, table))
}

/// Render a rate with eight decimals so it round-trips through
/// [`provider_expense_rate_units`] without losing precision.
fn format_rate(rate: f64) -> String {
    format!("{rate:.8}")
}

pub(crate) fn fx_rate_table_json(table: &FxRateTable) -> Value {
    let now = current_unix_secs();
    json!({
        "base": table.base,
        "display_currency": table.display_currency,
        "source": table.source.as_str(),
        "fetched_at_unix_secs": table.fetched_at_unix_secs,
        "stale": table.is_stale(now),
        "supported": SUPPORTED_DISPLAY_CURRENCIES,
        "rates": table.rates,
    })
}

/// Convert `amount` from `from` into the display currency.  `None` means the
/// table does not cover the pair, which callers surface as "no rate" rather
/// than guessing at 1:1.
///
/// `fx_rate_used` is the effective rate (`to / from`) rounded to the same eight
/// decimals the stored rates use, so an operator can multiply by it and land on
/// `amount_base`.  The amount itself is computed from both full-precision
/// rates, which keeps it exact even when the rounded summary is not.
pub(crate) fn convert_with_table(
    table: &FxRateTable,
    amount: &str,
    from: &str,
) -> Option<FxConversion> {
    let units = provider_expense_amount_units(amount)?;
    let from_rate = provider_expense_rate_units(table.rate(from)?)?;
    let to_rate = provider_expense_rate_units(table.rate(&table.display_currency)?)?;
    let base_units = convert_provider_expense_units(units, from_rate, to_rate)?;
    let effective = provider_expense_effective_rate(from_rate, to_rate)?;
    Some(FxConversion {
        fx_rate_used: format_provider_expense_amount(effective),
        amount_base: format_provider_expense_amount(base_units),
        base_currency: table.display_currency.clone(),
    })
}

pub(crate) async fn convert_to_display_currency(
    state: &AppState,
    amount: &str,
    from: &str,
) -> Result<Option<FxConversion>, GatewayError> {
    let table = read_fx_rate_table(state).await?;
    Ok(convert_with_table(&table, amount, from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table_with(rates: Value, display_currency: &str) -> FxRateTable {
        let stored = json!({
            "base": "USD",
            "source": "live",
            "fetched_at_unix_secs": 1_795_564_951_u64,
            "rates": rates,
        });
        parse_fx_rate_table(Some(&stored), display_currency.to_string())
    }

    #[test]
    fn parse_keeps_valid_rates_and_ignores_everything_else() {
        let table = table_with(
            json!({
                "CNY": "6.678179",
                "JPY": "159.238818",
                "bad-code": "1",
                "TOOLONGCODE": "1",
                "EUR": "0",
                "GBP": "not-a-number",
                "HKD": 7.84,
                "USD": "99",
            }),
            "CNY",
        );
        assert_eq!(table.display_currency, "CNY");
        assert_eq!(table.base, "USD");
        assert_eq!(table.source, FxRateSource::Live);
        assert_eq!(table.fetched_at_unix_secs, 1_795_564_951);
        // USD is pinned to the quote base even though the stored value said 99.
        assert_eq!(table.rate("USD"), Some("1.00000000"));
        assert_eq!(table.rate("CNY"), Some("6.678179"));
        assert_eq!(table.rate("JPY"), Some("159.238818"));
        for missing in ["EUR", "GBP", "HKD", "RUB"] {
            assert_eq!(table.rate(missing), None, "{missing} should be dropped");
        }
    }

    #[test]
    fn parse_falls_back_to_a_usd_only_table_when_stored_value_is_unusable() {
        for stored in [
            None,
            Some(json!({"rates": "not-an-object"})),
            Some(json!({"base": "USD"})),
            Some(json!({"rates": {"CNY": "abc"}})),
        ] {
            let table = parse_fx_rate_table(stored.as_ref(), "USD".to_string());
            assert_eq!(table.rates.len(), 1);
            assert_eq!(table.rate("USD"), Some("1.00000000"));
            assert!(table.is_stale(1_795_564_951), "no fetch time means stale");
        }
    }

    #[test]
    fn staleness_uses_the_feed_cadence() {
        let table = table_with(json!({"CNY": "7.2"}), "USD");
        let fetched = table.fetched_at_unix_secs;
        assert!(!table.is_stale(fetched + FX_RATES_MAX_AGE_SECS));
        assert!(table.is_stale(fetched + FX_RATES_MAX_AGE_SECS + 1));
    }

    #[test]
    fn conversion_uses_the_ratio_of_two_rates() {
        let table = table_with(json!({"CNY": "6.678179", "JPY": "159.238818"}), "CNY");
        // 100 USD -> CNY multiplies by the CNY rate, and the stored rate is the
        // effective one, so multiplying by it reproduces the amount.
        let to_cny = convert_with_table(&table, "100", "USD").expect("usd to cny");
        assert_eq!(to_cny.amount_base, "667.81790000");
        assert_eq!(to_cny.fx_rate_used, "6.67817900");
        assert_eq!(to_cny.base_currency, "CNY");
        // Same currency in and out is the identity at rate 1.
        let identity = convert_with_table(&table, "100", "CNY").expect("cny to cny");
        assert_eq!(identity.amount_base, "100.00000000");
        assert_eq!(identity.fx_rate_used, "1.00000000");
        // 100 JPY -> CNY divides by the JPY rate: the effective rate is the
        // ratio, not the JPY-per-USD rate the table stores.
        let from_jpy = convert_with_table(&table, "100", "JPY").expect("jpy to cny");
        assert_eq!(from_jpy.fx_rate_used, "0.04193813");
        assert_eq!(from_jpy.amount_base, "4.19381347");
        // A currency the table does not cover converts to nothing at all.
        assert!(convert_with_table(&table, "100", "GBP").is_none());
        // A display currency outside the table cannot be a target either.
        let usd_table = table_with(json!({"CNY": "7.2"}), "EUR");
        assert!(convert_with_table(&usd_table, "100", "USD").is_none());
        // Malformed amounts are rejected rather than treated as zero.
        assert!(convert_with_table(&table, "0", "USD").is_none());
        assert!(convert_with_table(&table, "abc", "USD").is_none());
    }

    #[test]
    fn supported_currencies_are_unique_and_include_the_quote_base() {
        assert_eq!(SUPPORTED_DISPLAY_CURRENCIES.len(), 15);
        assert!(SUPPORTED_DISPLAY_CURRENCIES.contains(&"USD"));
        assert!(SUPPORTED_DISPLAY_CURRENCIES.contains(&"CNY"));
        for (index, code) in SUPPORTED_DISPLAY_CURRENCIES.iter().enumerate() {
            assert_eq!(code.len(), 3, "{code} must be three letters");
            assert!(
                !SUPPORTED_DISPLAY_CURRENCIES[..index].contains(code),
                "{code} is listed twice"
            );
        }
    }
}
