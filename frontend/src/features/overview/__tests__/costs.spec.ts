import { createApp, nextTick, type App } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import CostAnalysis from '@/views/admin/CostAnalysis.vue'
import { setI18nLocale } from '@/i18n'

const overview = vi.hoisted(() => ({ costs: vi.fn(), breakdown: vi.fn(), exportCsv: vi.fn() }))
const finance = vi.hoisted(() => ({ accounts: vi.fn(), expenses: vi.fn(), record: vi.fn(), void: vi.fn(), exportExpenses: vi.fn() }))
const fx = vi.hoisted(() => ({
  effectiveRate: vi.fn((table: { rates: Record<string, string>; display_currency: string }, from: string) => {
    const fromRate = Number(table.rates[from])
    const toRate = Number(table.rates[table.display_currency])
    if (!Number.isFinite(fromRate) || !Number.isFinite(toRate) || fromRate <= 0 || toRate <= 0) return null
    return toRate / fromRate
  }),
}))
const rates = vi.hoisted(() => ({ set: null as ((value: unknown) => void) | null }))
vi.mock('@/api/overview', () => ({ overviewApi: overview }))
vi.mock('@/api/providerFinance', () => ({ providerFinanceApi: finance, SUPPORTED_CURRENCIES: ['USD', 'CNY', 'EUR', 'JPY', 'GBP', 'HKD', 'SGD', 'KRW', 'INR', 'TWD', 'AUD', 'CAD', 'RUB', 'MYR', 'THB'] }))
vi.mock('@/api/fx', () => ({ fxApi: { table: vi.fn(), refresh: vi.fn() }, effectiveRate: fx.effectiveRate }))
vi.mock('@/composables/useFxRates', async () => {
  const { ref } = await import('vue')
  const table = ref<unknown>(null)
  rates.set = (value: unknown) => { table.value = value }
  return {
    useFxRates: () => ({
      table,
      loading: ref(false),
      failed: ref(false),
      ensureLoaded: vi.fn(),
      refresh: vi.fn(),
      adopt: (next: unknown) => { table.value = next },
    }),
  }
})
vi.mock('@/components/charts/LineChart.vue', () => ({ default: { render: () => null } }))

const range = { from: '2026-09-01T00:00:00.000Z', to: '2026-09-02T00:00:00.000Z', timezone: 'UTC' }
const metric = { request_count: 1357, input_tokens: 5000, output_tokens: 678, total_tokens: 5678, billable_amount: { value: '999999', currency: 'USD', basis: 'billable', status: 'known' } }
const expense = { id: 'expense-1', provider_id: 'provider-1', provider_name: 'Provider One', kind: 'recharge', amount: '1.23', currency: 'USD', fx_rate_used: null, amount_base: null, base_currency: null, paid_at: range.from, period_start: null, period_end: null, note: 'Invoice one', external_reference: 'invoice-123', status: 'recorded', created_at: range.from, created_by: 'admin', voided_at: null }
const totals = [{ currency: 'USD', amount: '432.10', recharge_amount: '400', subscription_amount: '32.10', other_amount: '0', entry_count: 20 }, { currency: 'EUR', amount: '87.65', recharge_amount: '20', subscription_amount: '67.65', other_amount: '0', entry_count: 20 }]
const account = { provider_id: 'provider-1', provider_name: 'Provider One', is_active: true, billing_type: 'monthly_quota', quota: { limit: 100, used: 25, remaining: 75, currency: 'USD', period_start: range.from, expires_at: '2026-10-01T00:00:00Z' }, balance: { status: 'success', observed_at: range.to, currency: 'USD', available: 55, used: 45, granted: 100, plan_name: 'Monthly plan', subscriptions: [] } }
const mounted: { app: App; root: HTMLElement }[] = []
async function settle() { await Promise.resolve(); await Promise.resolve(); await nextTick(); await new Promise(resolve => setTimeout(resolve, 0)) }
async function mount(extra = '') {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/admin/cost-analysis', component: CostAnalysis }, { path: '/admin/usage', component: { render: () => null } }] })
  await router.push({ path: '/admin/cost-analysis', query: { ...range, ...(extra ? { provider_id: extra, model: 'legacy-model', amount_basis: 'billable', group_by: 'model' } : {}) } })
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(CostAnalysis).use(router)
  app.mount(root)
  mounted.push({ app, root })
  await settle()
  return { root, router }
}
function section(root: ParentNode, selector: string) {
  const item = root.querySelector(selector)
  if (!item) throw new Error(`Missing section ${selector}`)
  return item
}
function button(root: ParentNode, name: string) {
  const item = [...root.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === name)
  if (!item) throw new Error(`Missing button ${name}`)
  return item
}
function input(label: string) {
  const item = [...document.body.querySelectorAll('label')].find(item => item.textContent?.includes(label))?.querySelector<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>('input,select,textarea')
  if (!item) throw new Error(`Missing input ${label}`)
  return item
}
function fill(label: string, value: string) {
  const field = input(label)
  field.value = value
  field.dispatchEvent(new Event(field.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }))
}
async function submitExpense() {
  section(document.body, '#provider-expense-form').dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
  await settle()
}
beforeEach(() => {
  vi.resetAllMocks()
  setI18nLocale('zh-CN')
  overview.costs.mockResolvedValue({ data: { summary: metric, timeseries: [{ ...metric, bucket_start: range.from }] } })
  overview.breakdown.mockImplementation(async query => ({ data: { items: [{ ...metric, id: 'provider-1', label: 'Provider One' }], total: 40, limit: query.limit, offset: query.offset } }))
  finance.accounts.mockResolvedValue({ observed_at: range.to, items: [account] })
  finance.expenses.mockImplementation(async query => ({ items: [expense], total: 40, limit: query.limit, offset: query.offset, totals, base_totals: [], providers: [{ provider_id: 'provider-1', provider_name: 'Provider One', currency: 'USD', amount: '432.10', entry_count: 20 }] }))
  finance.record.mockResolvedValue({ item: expense })
  finance.void.mockResolvedValue({ item: { ...expense, status: 'void' } })
})
afterEach(() => { mounted.splice(0).forEach(({ app, root }) => { app.unmount(); root.remove() }); setI18nLocale('zh-CN') })

describe('provider expense analysis', () => {
  it('uses complete expense totals and keeps currencies separate from customer consumption', async () => {
    const { root } = await mount('ignored-provider')
    const summary = section(root, '[data-cost-summary]').textContent
    expect(summary).toContain('432.10 USD')
    expect(summary).toContain('87.65 EUR')
    expect(summary).not.toContain('1.23')
    expect(summary).not.toContain('999,999')
    expect(summary).not.toContain('519.75')
    expect(summary).toContain('1,357')
    expect(overview.costs.mock.lastCall?.[0]).not.toHaveProperty('provider_id')
    expect(overview.breakdown.mock.lastCall?.[0]).toMatchObject({ group_by: 'provider', sort: 'request_count' })
    expect(overview.breakdown.mock.lastCall?.[0]).not.toHaveProperty('model')
    expect(finance.expenses.mock.lastCall?.[0]).not.toHaveProperty('provider_id')
  })

  it.each(['costs', 'providers', 'accounts', 'expenses'] as const)('keeps other sections available when %s fails', async failed => {
    const call = { costs: overview.costs, providers: overview.breakdown, accounts: finance.accounts, expenses: finance.expenses }[failed]
    call.mockRejectedValue(new Error(`${failed} unavailable`))
    const { root } = await mount()
    expect(overview.costs).toHaveBeenCalledTimes(1)
    expect(overview.breakdown).toHaveBeenCalledTimes(1)
    expect(finance.accounts).toHaveBeenCalledTimes(1)
    expect(finance.expenses).toHaveBeenCalledTimes(1)
    expect(root.textContent).toContain(`${failed} unavailable`)
    if (failed !== 'providers') expect(section(root, '[data-provider-usage]').textContent).toContain('Provider One')
    if (failed !== 'accounts') expect(section(root, '[data-provider-accounts]').textContent).toContain('Monthly plan')
    if (failed !== 'expenses') expect(section(root, '[data-cost-summary]').textContent).toContain('432.10 USD')
    if (failed === 'expenses') expect(section(root, '[data-cost-summary]').textContent).not.toContain('999,999')
  })

  it('retains expense totals during pagination and clears them for another date scope', async () => {
    const { root, router } = await mount()
    finance.expenses.mockImplementation(() => new Promise(() => {}))
    section(root, '[data-provider-expenses]').querySelector<HTMLButtonElement>('[aria-label="下一页"]')?.click()
    await settle()
    expect(finance.expenses.mock.lastCall?.[0]).toMatchObject({ offset: 25 })
    expect(section(root, '[data-cost-summary]').textContent).toContain('432.10 USD')
    expect(overview.costs).toHaveBeenCalledTimes(1)
    expect(overview.breakdown).toHaveBeenCalledTimes(1)
    await router.replace({ query: { from: '2026-08-01T00:00:00Z', to: '2026-08-02T00:00:00Z', timezone: 'UTC' } })
    await settle()
    expect(finance.expenses.mock.lastCall?.[0]).toMatchObject({ offset: 0, from: '2026-08-01T00:00:00.000Z' })
    expect(section(root, '[data-cost-summary]').textContent).not.toContain('432.10 USD')
    expect(section(root, '[data-provider-expenses]').textContent).not.toContain('invoice-123')
  })

  it('reuses a retry id for unchanged payment data and refreshes expenses after a successful save', async () => {
    finance.record.mockRejectedValue(new Error('connection lost'))
    const { root } = await mount()
    button(root, '登记支出').click()
    await settle()
    fill('提供商', 'provider-1')
    fill('实付金额', '10.25')
    fill('付款时间', '2026-09-01T13:20')
    await submitExpense()
    expect(finance.record).toHaveBeenCalledTimes(1)
    const original = finance.record.mock.lastCall?.[0]
    expect(original).toMatchObject({ amount: '10.25', provider_id: 'provider-1', currency: 'USD', paid_at: '2026-09-01T13:20:00.000Z' })
    expect(original.client_request_id).toBeTruthy()
    expect(document.body.textContent).toContain('connection lost')
    await submitExpense()
    expect(finance.record.mock.lastCall?.[0].client_request_id).toBe(original.client_request_id)
    fill('实付金额', '11.25')
    finance.record.mockResolvedValue({ item: { ...expense, amount: '11.25' } })
    await submitExpense()
    expect(finance.record.mock.lastCall?.[0].client_request_id).not.toBe(original.client_request_id)
    expect(finance.expenses).toHaveBeenCalledTimes(2)
    expect(document.body.querySelector('#provider-expense-form')).toBeNull()
  })

  it('requires a confirmation before voiding and refreshes the ledger only after confirmation', async () => {
    const { root } = await mount()
    button(section(root, '[data-provider-expenses]'), '作废').click()
    await settle()
    expect(finance.void).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('确认作废这笔记录')
    button(document.body, '取消').click()
    await settle()
    expect(finance.void).not.toHaveBeenCalled()
    button(section(root, '[data-provider-expenses]'), '作废').click()
    await settle()
    button(document.body, '确认作废').click()
    await settle()
    expect(finance.void).toHaveBeenCalledWith('expense-1')
    expect(finance.expenses).toHaveBeenCalledTimes(2)
  })

  it('rolls the converted total up under the display currency', async () => {
    finance.expenses.mockImplementation(async query => ({
      items: [expense],
      total: 40,
      limit: query.limit,
      offset: query.offset,
      totals,
      base_totals: [{ currency: 'CNY', amount: '3352.10', recharge_amount: '3100', subscription_amount: '252.10', other_amount: '0', entry_count: 30 }],
      providers: [{ provider_id: 'provider-1', provider_name: 'Provider One', currency: 'USD', amount: '432.10', entry_count: 20 }],
    }))
    const { root } = await mount()

    const strip = section(root, '[data-fx-total]').textContent
    expect(strip).toContain('按全局币种合计')
    expect(strip).toContain('3,352.10')
    expect(strip).toContain('CNY')
    expect(strip).toContain('30 / 40')
    // The per-currency cards keep their own grouping.
    expect(section(root, '[data-cost-summary]').textContent).toContain('432.10 USD')
  })

  it('omits the converted rollup until some row carries a snapshot', async () => {
    const { root } = await mount()

    expect(root.querySelector('[data-fx-total]')).toBeNull()
  })

  it('shows the rate each row was converted at when it was recorded', async () => {
    finance.expenses.mockImplementation(async query => ({
      items: [{ ...expense, currency: 'CNY', amount: '48.28', fx_rate_used: '0.13881347', amount_base: '6.70', base_currency: 'USD' }],
      total: 40,
      limit: query.limit,
      offset: query.offset,
      totals: [{ currency: 'CNY', amount: '48.28', recharge_amount: '48.28', subscription_amount: '0', other_amount: '0', entry_count: 1 }],
      base_totals: [{ currency: 'USD', amount: '6.70', recharge_amount: '6.70', subscription_amount: '0', other_amount: '0', entry_count: 1 }],
      providers: [],
    }))
    const { root } = await mount()

    const ledger = section(root, '[data-provider-expenses]')
    expect(ledger.textContent).toContain('48.28')
    expect(ledger.textContent).toContain('≈')
    expect(ledger.querySelector('[data-fx-rate]')?.getAttribute('title'))
      .toContain('1 CNY = 0.13881347 USD')
  })

  it('previews the conversion into the display currency before saving', async () => {
    rates.set?.({
      base: 'USD',
      display_currency: 'CNY',
      source: 'live',
      fetched_at_unix_secs: 1_795_564_951,
      stale: false,
      supported: ['USD', 'CNY'],
      rates: { USD: '1', CNY: '6.678179' },
    })
    const { root } = await mount()
    button(root, '登记支出').click()
    await settle()

    expect(document.body.querySelector('[data-fx-preview]')).toBeNull()
    fill('币种', 'USD')
    fill('实付金额', '100')
    await settle()

    const preview = document.body.querySelector('[data-fx-preview]')?.textContent
    expect(preview).toContain('CNY')
    expect(preview).toContain('667.82')
    expect(fx.effectiveRate).toHaveBeenCalledWith(expect.objectContaining({ display_currency: 'CNY' }), 'USD')
  })

  it('warns instead of previewing when no rate covers the chosen currency', async () => {
    // CNY is simply absent, so the real rate helper has nothing to divide by.
    rates.set?.({
      base: 'USD',
      display_currency: 'USD',
      source: 'live',
      fetched_at_unix_secs: 1_795_564_951,
      stale: false,
      supported: ['USD', 'CNY'],
      rates: { USD: '1' },
    })
    const { root } = await mount()
    button(root, '登记支出').click()
    await settle()
    fill('币种', 'CNY')
    fill('实付金额', '100')
    await settle()

    expect(document.body.querySelector('[data-fx-preview]')).toBeNull()
    expect(document.body.textContent).toContain('汇率表缺少 CNY')
  })

  it('warns when no rate table is available at all', async () => {
    rates.set?.(null)
    const { root } = await mount()
    button(root, '登记支出').click()
    await settle()
    fill('实付金额', '100')
    await settle()

    expect(document.body.querySelector('[data-fx-preview]')).toBeNull()
    expect(document.body.textContent).toContain('尚未取得汇率')
  })
})
