import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, postMock, putMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  postMock: vi.fn(),
  putMock: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: { get: getMock, post: postMock, put: putMock },
}))

import { effectiveRate, fxApi, isSupportedCurrency } from '@/api/fx'
import type { FxRateTable } from '@/api/providerFinance'

const table: FxRateTable = {
  base: 'USD',
  display_currency: 'CNY',
  source: 'live',
  fetched_at_unix_secs: 1_795_564_951,
  stale: false,
  supported: ['USD', 'CNY', 'JPY'],
  rates: { USD: '1', CNY: '6.678179', JPY: '159.238818' },
}

describe('exchange rate helpers', () => {
  it('quotes the ratio between two per-USD rates', () => {
    // Same pair the gateway records as fx_rate_used on a saved expense.
    expect(effectiveRate(table, 'USD')).toBeCloseTo(6.678179, 8)
    expect(effectiveRate(table, 'CNY')).toBe(1)
    expect(effectiveRate(table, 'JPY')).toBeCloseTo(6.678179 / 159.238818, 12)
  })

  it('reports no rate for a currency the table does not cover', () => {
    expect(effectiveRate(table, 'GBP')).toBeNull()
    expect(effectiveRate({ ...table, rates: { USD: '1' } }, 'USD')).toBeNull()
  })

  it('rejects rates that cannot be used as numbers', () => {
    for (const rates of [
      { USD: '1', CNY: 'abc' },
      { USD: '1', CNY: '0' },
      { USD: '1', CNY: '-1' },
    ]) {
      expect(effectiveRate({ ...table, rates }, 'USD'), JSON.stringify(rates)).toBeNull()
    }
  })

  it('recognises only the currencies the picker offers', () => {
    expect(isSupportedCurrency('USD')).toBe(true)
    expect(isSupportedCurrency('THB')).toBe(true)
    expect(isSupportedCurrency('usd')).toBe(false)
    expect(isSupportedCurrency('USDT')).toBe(false)
    expect(isSupportedCurrency(42)).toBe(false)
    expect(isSupportedCurrency(undefined)).toBe(false)
  })
})

describe('exchange rate endpoints', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    putMock.mockReset()
  })

  it('unwraps the table from the read response', async () => {
    getMock.mockResolvedValue({ data: { item: table } })

    expect(await fxApi.table()).toBe(table)
    expect(getMock.mock.lastCall?.[0]).toBe('/api/admin/billing/fx-rates')
  })

  it('unwraps the table from the refresh response', async () => {
    postMock.mockResolvedValue({ data: { item: { ...table, source: 'manual' } } })

    const refreshed = await fxApi.refresh()

    expect(refreshed.source).toBe('manual')
    expect(postMock.mock.lastCall?.[0]).toBe('/api/admin/billing/fx-rates/refresh')
  })

  it('writes hand-edited rates under the gateway-owned config key', async () => {
    putMock.mockResolvedValue({ data: { key: 'ui.fx_rates' } })

    await fxApi.saveManualRates({ CNY: '7.2', GBP: '' }, 1_795_564_951)

    const [path, body] = putMock.mock.lastCall as [string, Record<string, unknown>]
    expect(path).toBe('/api/admin/system/configs/ui.fx_rates')
    expect(body.value).toMatchObject({
      base: 'USD',
      source: 'manual',
      fetched_at_unix_secs: 1_795_564_951,
      rates: { CNY: '7.2', GBP: '' },
    })
  })

  it('stamps a never-fetched table as current so it does not read as stale', async () => {
    putMock.mockResolvedValue({ data: { key: 'ui.fx_rates' } })

    await fxApi.saveManualRates({ CNY: '7.2' }, 0)

    const [, body] = putMock.mock.lastCall as [string, { value: { fetched_at_unix_secs: number } }]
    expect(body.value.fetched_at_unix_secs).toBeGreaterThan(1_700_000_000)
  })
})
