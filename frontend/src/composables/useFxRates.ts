import { readonly, ref } from 'vue'
import { fxApi } from '@/api/fx'
import type { FxRateTable } from '@/api/providerFinance'

/** The rate table changes about once a day and every surface that shows money
 *  wants the same one, so it is loaded once per app rather than per component. */
const table = ref<FxRateTable | null>(null)
const loading = ref(false)
const failed = ref(false)
let inFlight: Promise<FxRateTable | null> | null = null

async function load(fromFeed: boolean): Promise<FxRateTable | null> {
  if (inFlight) return inFlight
  loading.value = true
  inFlight = (async () => {
    try {
      const next = fromFeed ? await fxApi.refresh() : await fxApi.table()
      table.value = next
      failed.value = false
      return next
    } catch {
      failed.value = true
      return null
    } finally {
      loading.value = false
      inFlight = null
    }
  })()
  return inFlight
}

export function useFxRates() {
  /** Load the table, pulling from the feed only when what is stored is stale.
   *
   *  A stale table is refreshed rather than shown as-is because a conversion
   *  preview from a day-old rate is worse than no preview.  When the feed is
   *  unreachable the stored table is kept, so the return value is whatever is in
   *  effect afterwards rather than the result of the last fetch.
   */
  async function ensureLoaded(): Promise<FxRateTable | null> {
    if (table.value && !table.value.stale) return table.value
    if (table.value) {
      await load(true)
    } else {
      const current = await load(false)
      if (current?.stale) await load(true)
    }
    return table.value
  }

  return {
    table: readonly(table),
    loading: readonly(loading),
    failed: readonly(failed),
    ensureLoaded,
    refresh: () => load(true),
    /** Adopt a table the settings page just saved, without another round trip. */
    adopt: (next: FxRateTable) => {
      table.value = next
      failed.value = false
    },
  }
}
