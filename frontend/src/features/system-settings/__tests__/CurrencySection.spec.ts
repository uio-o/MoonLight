import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'

import CurrencySection from '../CurrencySection.vue'

const { saveRates, saveCurrency } = vi.hoisted(() => ({
  saveRates: vi.fn(),
  saveCurrency: vi.fn(),
}))

vi.mock('@/api/fx', () => ({
  fxApi: {
    saveManualRates: saveRates,
    saveDisplayCurrency: saveCurrency,
    table: vi.fn(),
    refresh: vi.fn(),
  },
  effectiveRate: vi.fn(),
}))

vi.mock('@/composables/useFxRates', () => ({
  useFxRates: () => ({
    table: ref(null),
    loading: ref(false),
    failed: ref(false),
    ensureLoaded: vi.fn(async () => null),
    refresh: vi.fn(),
    adopt: vi.fn(),
  }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn() }),
}))

vi.mock('@/components/layout', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    CardSection: defineComponent({
      name: 'CardSectionStub',
      props: { title: String, description: String },
      setup(props, { slots }) {
        return () => h('section', [h('h2', props.title), slots.actions?.(), slots.default?.()])
      },
    }),
  }
})

// Attribute-forwarding stubs: the section reaches its fields by id.
vi.mock('@/components/ui/button.vue', () => ({
  default: defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('button', attrs, slots.default?.())
    },
  }),
}))

// Mirrors the real input: a native `input` event becomes `update:modelValue`.
vi.mock('@/components/ui/input.vue', () => ({
  default: defineComponent({
    name: 'InputStub',
    inheritAttrs: false,
    emits: ['update:modelValue'],
    setup(_, { attrs, emit }) {
      return () =>
        h('input', {
          ...attrs,
          onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
        })
    },
  }),
}))

vi.mock('@/components/ui/label.vue', () => ({
  default: defineComponent({
    name: 'LabelStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('label', attrs, slots.default?.())
    },
  }),
}))

// The select family is only a container here; the section reads its own state.
vi.mock('@/components/ui/select.vue', () => ({
  default: defineComponent({
    name: 'SelectStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('div', attrs, slots.default?.())
    },
  }),
}))

vi.mock('@/components/ui/select-trigger.vue', () => ({
  default: defineComponent({
    name: 'SelectTriggerStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('div', attrs, slots.default?.())
    },
  }),
}))

vi.mock('@/components/ui/select-value.vue', () => ({
  default: defineComponent({
    name: 'SelectValueStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('div', attrs, slots.default?.())
    },
  }),
}))

vi.mock('@/components/ui/select-content.vue', () => ({
  default: defineComponent({
    name: 'SelectContentStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('div', attrs, slots.default?.())
    },
  }),
}))

vi.mock('@/components/ui/select-item.vue', () => ({
  default: defineComponent({
    name: 'SelectItemStub',
    inheritAttrs: false,
    setup(_, { slots, attrs }) {
      return () => h('div', attrs, slots.default?.())
    },
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function unmountAll() {
  mountedApps.splice(0).forEach(({ app, root }) => {
    app.unmount()
    root.remove()
  })
}

function mountSection() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(CurrencySection)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

function saveButton(root: ParentNode) {
  const button = [...root.querySelectorAll<HTMLButtonElement>('button')].find(
    item => item.textContent?.trim() === '保存'
  )
  if (!button) throw new Error('Missing save button')
  return button
}

function rateInput(root: ParentNode, code: string) {
  const input = root.querySelector<HTMLInputElement>(`#rate-${code}`)
  if (!input) throw new Error(`Missing rate input for ${code}`)
  return input
}

async function typeRate(root: ParentNode, code: string, value: string) {
  const input = rateInput(root, code)
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

afterEach(() => {
  unmountAll()
  vi.clearAllMocks()
})

describe('CurrencySection', () => {
  it('renders the section heading with no rates loaded', () => {
    const root = mountSection()

    expect(root.textContent).toContain('货币与汇率')
    expect(root.querySelector('#display-currency')).not.toBeNull()
    expect(root.textContent).toContain('汇率尚未加载')
  })

  it('offers every currency except the display one as an editable rate', () => {
    const root = mountSection()

    // USD is the quote base, so its rate is fixed at 1 rather than editable.
    expect(root.querySelector('#rate-USD')).toBeNull()
    expect(root.querySelector('#rate-CNY')).not.toBeNull()
  })

  it('saves the picked display currency together with the rates', async () => {
    const root = mountSection()
    await typeRate(root, 'CNY', '7.2')

    saveButton(root).click()
    await nextTick()

    expect(saveCurrency).toHaveBeenCalledWith('USD')
    expect(saveRates).toHaveBeenCalledWith({ CNY: '7.2' }, expect.any(Number))
  })

  it('rejects a rate the gateway could not parse, before any request', async () => {
    const root = mountSection()
    await typeRate(root, 'CNY', 'not-a-number')

    saveButton(root).click()
    await nextTick()

    expect(saveCurrency).not.toHaveBeenCalled()
    expect(saveRates).not.toHaveBeenCalled()
  })

  it('rejects a non-positive rate and one with too many decimals', async () => {
    for (const value of ['0', '-1', '1.123456789']) {
      const root = mountSection()
      await typeRate(root, 'CNY', value)

      saveButton(root).click()
      await nextTick()

      expect(saveRates, value).not.toHaveBeenCalled()
      unmountAll()
    }
  })

  it('keeps the save button disabled until something changes', async () => {
    const root = mountSection()

    expect(saveButton(root).disabled).toBe(true)

    await typeRate(root, 'CNY', '7.2')

    expect(saveButton(root).disabled).toBe(false)
  })
})
