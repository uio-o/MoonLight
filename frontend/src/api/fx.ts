import client from './client'
import { adminApi } from './admin'
import { SUPPORTED_CURRENCIES, type FxRateTable, type SupportedCurrency } from './providerFinance'

const base = '/api/admin/billing'
const RATES_KEY = 'ui.fx_rates'
const DISPLAY_CURRENCY_KEY = 'ui.display_currency'

export const fxApi = {
  async table(signal?: AbortSignal): Promise<FxRateTable> {
    const response = await client.get<{ item: FxRateTable }>(`${base}/fx-rates`, { signal })
    return response.data.item
  },
  async refresh(signal?: AbortSignal): Promise<FxRateTable> {
    const response = await client.post<{ item: FxRateTable }>(`${base}/fx-rates/refresh`, {}, { signal })
    return response.data.item
  },
  /** Persist a hand-edited table.
   *
   *  The gateway re-reads and validates what lands here, so an entry it cannot
   *  parse simply reports no rate instead of breaking every conversion.  The
   *  fetch timestamp is carried over so a hand edit does not silently mark the
   *  rest of the table as fresh; a table that was never fetched is stamped now,
   *  which keeps it from reading as permanently stale.
   */
  async saveManualRates(rates: Record<string, string>, fetchedAtUnixSecs: number): Promise<void> {
    const fetchedAt = fetchedAtUnixSecs || Math.floor(Date.now() / 1000)
    await adminApi.updateSystemConfig(
      RATES_KEY,
      { base: 'USD', source: 'manual', fetched_at_unix_secs: fetchedAt, rates },
      '全局显示币种汇率表，按 1 USD 可兑换的目标币种数量计价'
    )
  },
  async saveDisplayCurrency(currency: SupportedCurrency): Promise<void> {
    await adminApi.updateSystemConfig(
      DISPLAY_CURRENCY_KEY,
      currency,
      '全局显示币种，用于成本分析等金额展示'
    )
  },
}

/** Rate that converts one unit of `from` into the display currency.
 *
 *  Rates are quoted as units of each currency per one USD, so the effective
 *  rate is the ratio of the two — the same number the gateway stores as
 *  `fx_rate_used` on a recorded expense.
 */
export function effectiveRate(table: FxRateTable, from: string): number | null {
  const fromRate = Number(table.rates[from])
  const toRate = Number(table.rates[table.display_currency])
  if (!Number.isFinite(fromRate) || !Number.isFinite(toRate) || fromRate <= 0 || toRate <= 0) {
    return null
  }
  return toRate / fromRate
}

export function isSupportedCurrency(value: unknown): value is SupportedCurrency {
  return typeof value === 'string' && (SUPPORTED_CURRENCIES as readonly string[]).includes(value)
}
