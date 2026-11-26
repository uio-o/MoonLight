//! An administrator-maintained purchasing ledger. Entries are not inferred from request prices.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fixed-point scale shared by amounts and exchange rates: eight decimals.
pub const PROVIDER_EXPENSE_SCALE: u128 = 100_000_000;

pub fn provider_expense_amount_units(value: &str) -> Option<u128> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || whole.len() > 12
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 8
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let units = whole
        .parse::<u128>()
        .ok()?
        .checked_mul(PROVIDER_EXPENSE_SCALE)?
        .checked_add(if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u128>().ok()? * 10_u128.pow(8 - fraction.len() as u32)
        })?;
    (units > 0).then_some(units)
}

pub fn format_provider_expense_amount(units: u128) -> String {
    format!(
        "{}.{:08}",
        units / PROVIDER_EXPENSE_SCALE,
        units % PROVIDER_EXPENSE_SCALE
    )
}

/// Parse an exchange rate into fixed-point units at [`PROVIDER_EXPENSE_SCALE`].
///
/// A rate is quoted as units of the target currency per one USD, so it carries
/// no sign and no exponent notation.  The whole part is capped at eight digits,
/// which leaves room for rates like the Vietnamese dong's ~25 000 while keeping
/// the widest conversion (`units * rate_units`) inside `u128`.
pub fn provider_expense_rate_units(value: &str) -> Option<u128> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || whole.len() > 8
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 8
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let units = whole
        .parse::<u128>()
        .ok()?
        .checked_mul(PROVIDER_EXPENSE_SCALE)?
        .checked_add(if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u128>().ok()? * 10_u128.pow(8 - fraction.len() as u32)
        })?;
    (units > 0).then_some(units)
}

/// Convert an amount's fixed-point units between two currencies whose rates
/// are both quoted as units per one USD.
///
/// `units * to / from` is exact apart from the final division, which rounds
/// half-up.  Truncating would silently shave a fraction off every converted row
/// and make rollups disagree with the per-row figures an operator reads back.
/// The numerator is doubled before adding the divisor so the rounding cannot
/// bias toward zero.
pub fn convert_provider_expense_units(
    units: u128,
    from_rate_units: u128,
    to_rate_units: u128,
) -> Option<u128> {
    if from_rate_units == 0 {
        return None;
    }
    let scaled = units.checked_mul(to_rate_units)?;
    let numerator = scaled.checked_mul(2)?.checked_add(from_rate_units)?;
    let divisor = from_rate_units.checked_mul(2)?;
    Some(numerator / divisor)
}

/// The effective conversion rate between two quoted rates, as a fixed-point
/// amount at [`PROVIDER_EXPENSE_SCALE`].
///
/// This is the single number an operator multiplies by, so a JPY expense shown
/// in CNY reports ~0.0419 rather than the JPY-per-USD rate that produced it.
/// The converted amount itself is computed from both full-precision rates, so
/// this figure summarises the conversion instead of performing it.
pub fn provider_expense_effective_rate(from_rate_units: u128, to_rate_units: u128) -> Option<u128> {
    convert_provider_expense_units(PROVIDER_EXPENSE_SCALE, from_rate_units, to_rate_units)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderExpenseInput {
    pub client_request_id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub kind: String,
    pub amount: String,
    pub currency: String,
    /// Rate that converted `amount` into `amount_base`, quoted as units of
    /// `base_currency` per one USD.  `None` for rows recorded before the
    /// display-currency feature existed.
    pub fx_rate_used: Option<String>,
    /// `amount` converted at `fx_rate_used`.  Kept beside the original so a
    /// rollup never has to re-derive money that was actually paid.
    pub amount_base: Option<String>,
    /// Display currency in force when the row was recorded.
    pub base_currency: Option<String>,
    pub paid_at_unix_ms: u64,
    pub period_start_unix_ms: Option<u64>,
    pub period_end_unix_ms: Option<u64>,
    pub note: Option<String>,
    pub external_reference: Option<String>,
    pub created_by: Option<String>,
}

impl ProviderExpenseInput {
    /// Provider display names are snapshots, so renames do not break a retry.
    ///
    /// The three FX fields are deliberately excluded: they are derived from the
    /// rate table at write time, so a retry issued after a rate edit would
    /// otherwise look like a different request.  Identity is the money actually
    /// paid, and the stored snapshot stays the one that was first recorded.
    pub fn same_request_as(&self, other: &Self) -> bool {
        self.client_request_id == other.client_request_id
            && self.provider_id == other.provider_id
            && self.kind == other.kind
            && provider_expense_amount_units(&self.amount)
                == provider_expense_amount_units(&other.amount)
            && self.currency == other.currency
            && self.paid_at_unix_ms == other.paid_at_unix_ms
            && self.period_start_unix_ms == other.period_start_unix_ms
            && self.period_end_unix_ms == other.period_end_unix_ms
            && self.note == other.note
            && self.external_reference == other.external_reference
            && self.created_by == other.created_by
    }
    pub fn validate(&self) -> Result<(), String> {
        if uuid::Uuid::parse_str(&self.client_request_id).is_err() {
            return Err("client_request_id must be a UUID".into());
        }
        if self.provider_id.is_empty()
            || self.provider_id.len() > 512
            || self.provider_name.is_empty()
            || self.provider_name.len() > 512
        {
            return Err("invalid provider identity".into());
        }
        if !matches!(self.kind.as_str(), "recharge" | "subscription" | "other") {
            return Err("kind must be recharge, subscription or other".into());
        }
        if provider_expense_amount_units(&self.amount).is_none() {
            return Err(
                "amount must be positive with at most 12 integer and 8 decimal digits".into(),
            );
        }
        if self.currency.len() != 3 || !self.currency.bytes().all(|c| c.is_ascii_uppercase()) {
            return Err("currency must be a 3-letter uppercase code".into());
        }
        self.validate_fx_snapshot()?;
        if self.paid_at_unix_ms > 253_402_300_799_000
            || self
                .period_start_unix_ms
                .is_some_and(|v| v > 253_402_300_799_000)
            || self
                .period_end_unix_ms
                .is_some_and(|v| v > 253_402_300_799_000)
        {
            return Err("invalid timestamp".into());
        }
        match (self.period_start_unix_ms, self.period_end_unix_ms) {
            (None, None) => {}
            (Some(start), Some(end)) if start < end => {}
            _ => {
                return Err(
                    "coverage period must contain both start and end, with start before end".into(),
                )
            }
        }
        if self.note.as_ref().is_some_and(|v| {
            v.len() > 2000 || v.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
        }) || self
            .external_reference
            .as_ref()
            .is_some_and(|v| v.len() > 256 || v.chars().any(char::is_control))
        {
            return Err("invalid note or external_reference".into());
        }
        Ok(())
    }

    /// The FX snapshot is all-or-nothing: a row either carries a complete
    /// conversion or predates the feature and carries none.  A half-populated
    /// snapshot would make the converted rollup either drop the row or sum a
    /// rate the operator cannot see.
    fn validate_fx_snapshot(&self) -> Result<(), String> {
        match (&self.fx_rate_used, &self.amount_base, &self.base_currency) {
            (None, None, None) => Ok(()),
            (Some(rate), Some(amount_base), Some(base_currency)) => {
                if provider_expense_rate_units(rate).is_none() {
                    return Err("fx_rate_used must be a positive decimal rate".into());
                }
                if provider_expense_amount_units(amount_base).is_none() {
                    return Err("amount_base must be a positive decimal amount".into());
                }
                if base_currency.len() != 3
                    || !base_currency.bytes().all(|c| c.is_ascii_uppercase())
                {
                    return Err("base_currency must be a 3-letter uppercase code".into());
                }
                Ok(())
            }
            _ => Err("fx_rate_used, amount_base and base_currency must be set together".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderExpenseRecord {
    pub id: String,
    #[serde(flatten)]
    pub entry: ProviderExpenseInput,
    pub created_at_unix_ms: u64,
    pub voided_at_unix_ms: Option<u64>,
    pub voided_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderExpenseQuery {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub limit: u32,
    pub offset: u64,
}
impl ProviderExpenseQuery {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.from_unix_ms >= self.to_unix_ms
            || self.to_unix_ms > 253_402_300_799_000
            || self.to_unix_ms - self.from_unix_ms > 366 * 86_400_000
            || self.limit == 0
            || self.limit > 10_001
            || self.offset > i64::MAX as u64
        {
            return Err(crate::DataLayerError::InvalidInput(
                "invalid provider expense range or pagination".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderExpenseTotals {
    pub currency: String,
    pub amount: String,
    pub recharge_amount: String,
    pub subscription_amount: String,
    pub other_amount: String,
    pub entry_count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderExpenseProviderTotal {
    pub provider_id: String,
    pub provider_name: String,
    pub currency: String,
    pub amount: String,
    pub entry_count: u64,
}
#[derive(Debug, Clone, Default)]
pub struct ProviderExpensePage {
    pub items: Vec<ProviderExpenseRecord>,
    pub total: u64,
    pub totals: Vec<ProviderExpenseTotals>,
    /// Rollups over the converted amount, grouped by the display currency each
    /// row was recorded under.  Empty when every row predates the FX snapshot.
    pub base_totals: Vec<ProviderExpenseTotals>,
    pub providers: Vec<ProviderExpenseProviderTotal>,
}

/// Index of a kind's bucket inside the `[recharge, subscription, other]` triple.
fn expense_kind_index(kind: &str) -> usize {
    match kind {
        "recharge" => 0,
        "subscription" => 1,
        _ => 2,
    }
}

fn expense_totals(currencies: BTreeMap<String, ([u128; 3], u64)>) -> Vec<ProviderExpenseTotals> {
    currencies
        .into_iter()
        .map(|(currency, (amounts, entry_count))| ProviderExpenseTotals {
            currency,
            amount: format_provider_expense_amount(amounts.iter().sum()),
            recharge_amount: format_provider_expense_amount(amounts[0]),
            subscription_amount: format_provider_expense_amount(amounts[1]),
            other_amount: format_provider_expense_amount(amounts[2]),
            entry_count,
        })
        .collect()
}

pub fn provider_expense_memory_page(
    entries: impl Iterator<Item = ProviderExpenseRecord>,
    query: &ProviderExpenseQuery,
) -> Result<ProviderExpensePage, crate::DataLayerError> {
    query.validate()?;
    let mut entries = entries
        .filter(|r| {
            r.voided_at_unix_ms.is_none()
                && r.entry.paid_at_unix_ms >= query.from_unix_ms
                && r.entry.paid_at_unix_ms < query.to_unix_ms
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.entry
            .paid_at_unix_ms
            .cmp(&a.entry.paid_at_unix_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut currencies = BTreeMap::<String, ([u128; 3], u64)>::new();
    let mut base_currencies = BTreeMap::<String, ([u128; 3], u64)>::new();
    let mut providers = BTreeMap::<(String, String), (String, u128, u64)>::new();
    for record in &entries {
        let row = &record.entry;
        let units = provider_expense_amount_units(&row.amount).ok_or_else(|| {
            crate::DataLayerError::UnexpectedValue("invalid recorded expense amount".into())
        })?;
        let bucket = expense_kind_index(&row.kind);
        let (amounts, count) = currencies.entry(row.currency.clone()).or_default();
        amounts[bucket] += units;
        *count += 1;
        // A row without a snapshot simply contributes nothing to the converted
        // rollup; it still shows up in the per-currency totals above.
        if let (Some(base_currency), Some(amount_base)) = (&row.base_currency, &row.amount_base) {
            let base_units = provider_expense_amount_units(amount_base).ok_or_else(|| {
                crate::DataLayerError::UnexpectedValue("invalid converted expense amount".into())
            })?;
            let (amounts, count) = base_currencies.entry(base_currency.clone()).or_default();
            amounts[bucket] += base_units;
            *count += 1;
        }
        let (name, amount, count) = providers
            .entry((row.provider_id.clone(), row.currency.clone()))
            .or_insert_with(|| (row.provider_name.clone(), 0, 0));
        let _ = name;
        *amount += units;
        *count += 1;
    }
    Ok(ProviderExpensePage {
        total: entries.len() as u64,
        items: entries
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect(),
        totals: expense_totals(currencies),
        base_totals: expense_totals(base_currencies),
        providers: providers
            .into_iter()
            .map(
                |((provider_id, currency), (provider_name, amount, entry_count))| {
                    ProviderExpenseProviderTotal {
                        provider_id,
                        provider_name,
                        currency,
                        amount: format_provider_expense_amount(amount),
                        entry_count,
                    }
                },
            )
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_expense_amounts_are_exact_and_reject_ambiguous_inputs() {
        assert_eq!(
            format_provider_expense_amount(provider_expense_amount_units("0.12345678").unwrap()),
            "0.12345678"
        );
        for bad in [
            "0",
            "-1",
            "+1",
            "1e2",
            "NaN",
            "1.000000001",
            "1000000000000",
            " 1",
            ".1",
        ] {
            assert!(provider_expense_amount_units(bad).is_none(), "{bad}");
        }
    }
    #[test]
    fn provider_expense_report_has_currency_separation_void_exclusion_and_full_page_totals() {
        let entry =
            |id: &str, currency: &str, amount: &str, kind: &str, paid: u64, voided: bool| {
                ProviderExpenseRecord {
                    id: id.into(),
                    entry: ProviderExpenseInput {
                        client_request_id: uuid::Uuid::new_v4().to_string(),
                        provider_id: "p".into(),
                        provider_name: "Supplier".into(),
                        kind: kind.into(),
                        amount: amount.into(),
                        currency: currency.into(),
                        fx_rate_used: None,
                        amount_base: None,
                        base_currency: None,
                        paid_at_unix_ms: paid,
                        period_start_unix_ms: None,
                        period_end_unix_ms: None,
                        note: None,
                        external_reference: None,
                        created_by: None,
                    },
                    created_at_unix_ms: paid,
                    voided_at_unix_ms: voided.then_some(paid + 1),
                    voided_by: None,
                }
            };
        let rows = vec![
            entry("1", "USD", "0.1", "recharge", 10, false),
            entry("2", "USD", "0.2", "subscription", 20, false),
            entry("3", "CNY", "5", "other", 20, false),
            entry("4", "USD", "99", "recharge", 20, true),
            entry("5", "USD", "99", "recharge", 30, false),
        ];
        let page = provider_expense_memory_page(
            rows.into_iter(),
            &ProviderExpenseQuery {
                from_unix_ms: 10,
                to_unix_ms: 30,
                limit: 1,
                offset: 1,
            },
        )
        .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.totals[0].currency, "CNY");
        assert_eq!(page.totals[0].amount, "5.00000000");
        assert_eq!(page.totals[1].amount, "0.30000000");
        assert_eq!(page.totals[1].recharge_amount, "0.10000000");
        assert_eq!(page.totals[1].subscription_amount, "0.20000000");
        assert!(
            page.base_totals.is_empty(),
            "rows without a snapshot must not appear in the converted rollup"
        );
    }
    #[test]
    fn provider_expense_rates_parse_and_convert_without_drifting() {
        assert_eq!(provider_expense_rate_units("7.2"), Some(720_000_000));
        assert_eq!(provider_expense_rate_units("1"), Some(100_000_000));
        assert_eq!(
            provider_expense_rate_units("25765.93616"),
            Some(2_576_593_616_000)
        );
        for bad in ["0", "-1", "1e2", " 7.2", ".2", "123456789", "7.123456789"] {
            assert!(provider_expense_rate_units(bad).is_none(), "{bad}");
        }
        let usd = provider_expense_rate_units("1").expect("usd rate");
        // 100 USD at 6.678179 CNY per USD is exactly 667.8179 CNY.
        let units = provider_expense_amount_units("100").expect("valid amount");
        let cny = provider_expense_rate_units("6.678179").expect("valid rate");
        let converted = convert_provider_expense_units(units, usd, cny).expect("within u128");
        assert_eq!(format_provider_expense_amount(converted), "667.81790000");
        // The same pair read the other way is a division that does not resolve
        // cleanly: 100 / 6.678179 = 14.9741419000..., rounded half-up.
        let back = convert_provider_expense_units(units, cny, usd).expect("within u128");
        assert_eq!(format_provider_expense_amount(back), "14.97414190");
        // A rate above 1 with a large amount stays exact instead of overflowing.
        let big = provider_expense_amount_units("999999999999").expect("max amount");
        let vnd = provider_expense_rate_units("25765.93616").expect("vnd rate");
        assert!(convert_provider_expense_units(big, usd, vnd).is_some());
        assert!(convert_provider_expense_units(big, 0, vnd).is_none());
        // 720 CNY at 7.2 per USD lands exactly on 100 USD, so the half-up rule
        // sees a tie of zero and must not nudge the result either way.
        let cny72 = provider_expense_rate_units("7.2").expect("cny rate");
        let round_trip = provider_expense_amount_units("720").expect("valid amount");
        assert_eq!(
            format_provider_expense_amount(
                convert_provider_expense_units(round_trip, cny72, usd).expect("within u128")
            ),
            "100.00000000"
        );
    }
    #[test]
    fn provider_expense_base_totals_group_by_snapshot_currency() {
        let entry =
            |id: &str, currency: &str, amount: &str, base: Option<&str>| ProviderExpenseRecord {
                id: id.into(),
                entry: ProviderExpenseInput {
                    client_request_id: uuid::Uuid::new_v4().to_string(),
                    provider_id: "p".into(),
                    provider_name: "Supplier".into(),
                    kind: "recharge".into(),
                    amount: amount.into(),
                    currency: currency.into(),
                    fx_rate_used: base.map(|_| "7.20000000".to_string()),
                    amount_base: base.map(|value| value.to_string()),
                    base_currency: base.map(|_| "USD".to_string()),
                    paid_at_unix_ms: 10,
                    period_start_unix_ms: None,
                    period_end_unix_ms: None,
                    note: None,
                    external_reference: None,
                    created_by: None,
                },
                created_at_unix_ms: 10,
                voided_at_unix_ms: None,
                voided_by: None,
            };
        let rows = vec![
            entry("1", "CNY", "72", Some("10.00000000")),
            entry("2", "CNY", "36", Some("5.00000000")),
            entry("3", "CNY", "50", None),
            entry("4", "USD", "3", Some("3.00000000")),
        ];
        let page = provider_expense_memory_page(
            rows.into_iter(),
            &ProviderExpenseQuery {
                from_unix_ms: 0,
                to_unix_ms: 100,
                limit: 10,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(page.totals[0].currency, "CNY");
        assert_eq!(page.totals[0].amount, "158.00000000");
        assert_eq!(page.totals[1].currency, "USD");
        assert_eq!(page.totals[1].amount, "3.00000000");
        assert_eq!(page.base_totals.len(), 1);
        assert_eq!(page.base_totals[0].currency, "USD");
        assert_eq!(page.base_totals[0].amount, "18.00000000");
        assert_eq!(page.base_totals[0].recharge_amount, "18.00000000");
        assert_eq!(page.base_totals[0].entry_count, 3);
    }
    #[test]
    fn provider_expense_fx_snapshot_is_all_or_nothing() {
        let base = ProviderExpenseInput {
            client_request_id: uuid::Uuid::new_v4().to_string(),
            provider_id: "p".into(),
            provider_name: "Supplier".into(),
            kind: "recharge".into(),
            amount: "10".into(),
            currency: "CNY".into(),
            fx_rate_used: None,
            amount_base: None,
            base_currency: None,
            paid_at_unix_ms: 10,
            period_start_unix_ms: None,
            period_end_unix_ms: None,
            note: None,
            external_reference: None,
            created_by: None,
        };
        assert!(base.validate().is_ok());
        let mut complete = base.clone();
        complete.fx_rate_used = Some("7.20000000".into());
        complete.amount_base = Some("1.38888889".into());
        complete.base_currency = Some("USD".into());
        assert!(complete.validate().is_ok());
        let mutations: [fn(&mut ProviderExpenseInput); 6] = [
            |v| v.fx_rate_used = None,
            |v| v.amount_base = None,
            |v| v.base_currency = None,
            |v| v.base_currency = Some("US".into()),
            |v| v.fx_rate_used = Some("0".into()),
            |v| v.amount_base = Some("0".into()),
        ];
        for mutate in mutations {
            let mut broken = complete.clone();
            mutate(&mut broken);
            assert!(broken.validate().is_err());
        }
    }
    #[test]
    fn provider_expense_same_request_ignores_the_fx_snapshot() {
        let base = ProviderExpenseInput {
            client_request_id: uuid::Uuid::new_v4().to_string(),
            provider_id: "p".into(),
            provider_name: "Supplier".into(),
            kind: "recharge".into(),
            amount: "72".into(),
            currency: "CNY".into(),
            fx_rate_used: Some("7.20000000".into()),
            amount_base: Some("10.00000000".into()),
            base_currency: Some("USD".into()),
            paid_at_unix_ms: 10,
            period_start_unix_ms: None,
            period_end_unix_ms: None,
            note: None,
            external_reference: None,
            created_by: None,
        };
        let mut retried_after_rate_edit = base.clone();
        retried_after_rate_edit.fx_rate_used = Some("7.35000000".into());
        retried_after_rate_edit.amount_base = Some("9.79591837".into());
        assert!(base.same_request_as(&retried_after_rate_edit));
        let mut different_money = base.clone();
        different_money.amount = "73".into();
        assert!(!base.same_request_as(&different_money));
    }
}
