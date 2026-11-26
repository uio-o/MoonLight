import { beforeEach, describe, expect, it, vi } from 'vitest'

const { tableMock, refreshMock } = vi.hoisted(() => ({
  tableMock: vi.fn(),
  refreshMock: vi.fn(),
}))

vi.mock('@/api/fx', () => ({
  fxApi: { table: tableMock, refresh: refreshMock },
}))

import type { FxRateTable } from '@/api/providerFinance'

function makeTable(overrides: Partial<FxRateTable> = {}): FxRateTable {
  return {
    base: 'USD',
    display_currency: 'USD',
    source: 'live',
    fetched_at_unix_secs: 1_795_564_951,
    stale: false,
    supported: ['USD', 'CNY'],
    rates: { USD: '1', CNY: '6.678179' },
    ...overrides,
  }
}

/** The table lives at module scope so every surface shares one, which means each
 *  case needs a fresh module registry rather than a fresh component. */
async function useRates() {
  vi.resetModules()
  return (await import('@/composables/useFxRates')).useFxRates()
}

describe('useFxRates', () => {
  beforeEach(() => {
    tableMock.mockReset()
    refreshMock.mockReset()
  })

  it('reads the stored table without touching the feed', async () => {
    tableMock.mockResolvedValue(makeTable())
    const fx = await useRates()

    expect(await fx.ensureLoaded()).toMatchObject({ display_currency: 'USD' })
    expect(refreshMock).not.toHaveBeenCalled()
    expect(fx.table.value).toMatchObject({ display_currency: 'USD' })
  })

  it('refreshes a stored table that is already stale', async () => {
    tableMock.mockResolvedValue(makeTable({ stale: true }))
    refreshMock.mockResolvedValue(makeTable({ stale: false }))
    const fx = await useRates()

    const loaded = await fx.ensureLoaded()

    expect(tableMock).toHaveBeenCalledTimes(1)
    expect(refreshMock).toHaveBeenCalledTimes(1)
    expect(loaded?.stale).toBe(false)
  })

  it('keeps the stored table when the feed is unreachable', async () => {
    tableMock.mockResolvedValue(makeTable({ stale: true, display_currency: 'CNY' }))
    refreshMock.mockRejectedValue(new Error('gateway down'))
    const fx = await useRates()

    const loaded = await fx.ensureLoaded()

    expect(loaded).toMatchObject({ display_currency: 'CNY', stale: true })
    expect(fx.failed.value).toBe(true)
  })

  it('does not hit the feed when the read itself fails', async () => {
    tableMock.mockRejectedValue(new Error('network down'))
    const fx = await useRates()

    expect(await fx.ensureLoaded()).toBeNull()
    expect(refreshMock).not.toHaveBeenCalled()
  })

  it('reuses a loaded fresh table across callers', async () => {
    tableMock.mockResolvedValue(makeTable())
    const fx = await useRates()

    await fx.ensureLoaded()
    await fx.ensureLoaded()

    expect(tableMock).toHaveBeenCalledTimes(1)
  })

  it('shares one in-flight request instead of starting a second', async () => {
    let release: (value: FxRateTable) => void = () => {}
    tableMock.mockReturnValue(new Promise<FxRateTable>(resolve => { release = resolve }))
    const fx = await useRates()

    const first = fx.ensureLoaded()
    const second = fx.ensureLoaded()
    release(makeTable())
    await Promise.all([first, second])

    expect(tableMock).toHaveBeenCalledTimes(1)
  })

  it('adopts a table the settings page just saved', async () => {
    const fx = await useRates()

    fx.adopt(makeTable({ display_currency: 'CNY' }))

    expect(fx.table.value).toMatchObject({ display_currency: 'CNY' })
    expect(fx.failed.value).toBe(false)
  })
})
