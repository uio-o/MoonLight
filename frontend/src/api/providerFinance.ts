import client from './client'
import type { OverviewRange } from './overview'

export type ProviderExpenseKind = 'recharge' | 'subscription' | 'other'
export interface ProviderExpense {
  id: string
  provider_id: string
  provider_name: string
  kind: ProviderExpenseKind
  amount: string
  currency: string
  /** Rate that converted `amount` into `amount_base`, quoted as units of
   * `base_currency` per one unit of `currency`.  Null when the row was recorded
   * before a display currency was configured. */
  fx_rate_used: string | null
  amount_base: string | null
  base_currency: string | null
  paid_at: string
  period_start: string | null
  period_end: string | null
  note: string | null
  external_reference: string | null
  status: 'recorded' | 'void'
  created_at: string
  created_by: string | null
  voided_at: string | null
}
export interface ProviderExpenseInput {
  client_request_id: string
  provider_id: string
  kind: ProviderExpenseKind
  amount: string
  currency: string
  paid_at: string
  period_start?: string | null
  period_end?: string | null
  note?: string | null
  external_reference?: string | null
}
export interface ProviderExpenseTotals {
  currency: string
  amount: string
  entry_count: number
  recharge_amount: string
  subscription_amount: string
  other_amount: string
}
export interface ProviderExpenses {
  items: ProviderExpense[]
  total: number
  limit: number
  offset: number
  totals: ProviderExpenseTotals[]
  /** Rollups over the converted amount, grouped by the display currency each
   * row was recorded under.  Empty while no row carries a snapshot. */
  base_totals: ProviderExpenseTotals[]
  providers: { provider_id: string; provider_name: string; currency: string; amount: string; entry_count: number }[]
}
/** Currencies offered when recording an expense or choosing a display currency. */
export const SUPPORTED_CURRENCIES = [
  'USD', 'CNY', 'EUR', 'JPY', 'GBP', 'HKD', 'SGD', 'KRW', 'INR', 'TWD', 'AUD', 'CAD', 'RUB', 'MYR', 'THB',
] as const
export type SupportedCurrency = typeof SUPPORTED_CURRENCIES[number]
export type FxRateSource = 'live' | 'manual'
export interface FxRateTable {
  base: string
  display_currency: string
  source: FxRateSource
  fetched_at_unix_secs: number
  stale: boolean
  /** Fixed list the gateway offers; never narrowed by what the feed returned. */
  supported: readonly string[]
  rates: Record<string, string>
}
export interface ProviderAccountSubscription {
  group_name: string | null
  status: string | null
  daily_used_usd: number | null
  daily_limit_usd: number | null
  weekly_used_usd: number | null
  weekly_limit_usd: number | null
  monthly_used_usd: number | null
  monthly_limit_usd: number | null
  expires_at: string | null
}
export interface ProviderAccount {
  provider_id: string
  provider_name: string
  is_active: boolean
  billing_type: string | null
  quota: { limit: number | string | null; used: number | string | null; remaining: number | string | null; currency: string; period_start: string | null; expires_at: string | null } | null
  balance: {
    status: string
    observed_at: string | null
    currency: string | null
    available: number | string | null
    used: number | string | null
    granted: number | string | null
    plan_name: string | null
    subscriptions: ProviderAccountSubscription[]
  } | null
}
export interface ProviderAccounts { observed_at: string; items: ProviderAccount[] }
export type ProviderExpensesQuery = OverviewRange & { limit?: number; offset?: number }
const base = '/api/admin/billing'
export const providerFinanceApi = {
  async accounts(signal?: AbortSignal): Promise<ProviderAccounts> {
    return (await client.get<ProviderAccounts>(`${base}/provider-accounts`, { signal })).data
  },
  async expenses(params: ProviderExpensesQuery, signal?: AbortSignal): Promise<ProviderExpenses> {
    return (await client.get<ProviderExpenses>(`${base}/provider-expenses`, { params, signal })).data
  },
  async record(input: ProviderExpenseInput): Promise<{ item: ProviderExpense }> {
    return (await client.post<{ item: ProviderExpense }>(`${base}/provider-expenses`, input)).data
  },
  async void(id: string): Promise<{ item: ProviderExpense }> {
    return (await client.post<{ item: ProviderExpense }>(`${base}/provider-expenses/${encodeURIComponent(id)}/void`, {})).data
  },
  async exportExpenses(range: OverviewRange, signal?: AbortSignal): Promise<Blob> {
    return (await client.get<Blob>(`${base}/provider-expenses`, { params: { ...range, format: 'csv' }, responseType: 'blob', signal })).data
  },
  async refreshFxRates(signal?: AbortSignal): Promise<{ item: FxRateTable }> {
    return (await client.post<{ item: FxRateTable }>(`${base}/fx-rates/refresh`, {}, { signal })).data
  },
}
