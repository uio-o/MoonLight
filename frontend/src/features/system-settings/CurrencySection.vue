<template>
  <CardSection
    :title="t('ui.currencyAndExchangeRates')"
    :description="t('ui.chooseTheCurrencyUsedForCostAnalysisAndProviderExpenses')"
  >
    <template #actions>
      <div class="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          :disabled="refreshing"
          :title="t('ui.fetchTheLatestRatesFromThePublicFeed')"
          @click="refresh"
        >
          <RefreshCw
            class="w-3.5 h-3.5 mr-1.5"
            :class="refreshing && 'animate-spin'"
          />
          {{ refreshing ? t('ui.fetching') : t('ui.refreshRates') }}
        </Button>
        <Button
          size="sm"
          :disabled="saving || !dirty"
          @click="save"
        >
          {{ saving ? t('ui.saving') : t('ui.save') }}
        </Button>
      </div>
    </template>

    <div class="space-y-5">
      <div class="max-w-md">
        <Label
          for="display-currency"
          class="block text-sm font-medium"
        > {{ t('ui.displayCurrency') }} </Label>
        <Select
          :model-value="displayCurrency"
          @update:model-value="pickDisplayCurrency"
        >
          <SelectTrigger id="display-currency">
            <SelectValue :placeholder="t('ui.selectCurrency')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="code in currencies"
              :key="code"
              :value="code"
            >
              {{ code }}
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          {{ t('ui.amountsAlreadyRecordedKeepTheirOriginalCurrency') }}
        </p>
      </div>

      <div>
        <div class="flex flex-wrap items-baseline justify-between gap-2">
          <Label class="block text-sm font-medium">{{ t('ui.exchangeRatesPerUsd') }}</Label>
          <p class="text-xs text-muted-foreground">
            {{ rateStatus }}
          </p>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">
          {{ t('ui.ratesAreQuotedAsUnitsOfEachCurrencyPerOneUsd') }}
        </p>
        <div class="mt-3 grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3 lg:grid-cols-4">
          <div
            v-for="code in editableCurrencies"
            :key="code"
          >
            <Label
              :for="`rate-${code}`"
              class="block text-xs text-muted-foreground"
            > 1 USD = {{ code }}</Label>
            <Input
              :id="`rate-${code}`"
              :model-value="rates[code] || ''"
              inputmode="decimal"
              placeholder="0"
              class="mt-1"
              @update:model-value="editRate(code, $event)"
            />
          </div>
        </div>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RefreshCw } from 'lucide-vue-next'
import { useI18n } from '@/i18n'
import { useToast } from '@/composables/useToast'
import { useFxRates } from '@/composables/useFxRates'
import { log } from '@/utils/logger'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'
import { fxApi } from '@/api/fx'
import { SUPPORTED_CURRENCIES, type FxRateTable, type SupportedCurrency } from '@/api/providerFinance'

const { t } = useI18n()
const { success, error } = useToast()
const { table, ensureLoaded, refresh: pullRates, adopt } = useFxRates()

const saving = ref(false)
const refreshing = ref(false)

const displayCurrency = ref<SupportedCurrency>('USD')
const rates = ref<Record<string, string>>({})
const loaded = ref<FxRateTable | null>(null)

/** Mirror the shared table into a local draft so edits can be discarded. */
function sync(next: FxRateTable | null) {
  if (!next) return
  loaded.value = next
  displayCurrency.value = (SUPPORTED_CURRENCIES as readonly string[]).includes(next.display_currency)
    ? (next.display_currency as SupportedCurrency)
    : 'USD'
  rates.value = { ...next.rates }
}
sync(table.value)

onMounted(async () => {
  const next = await ensureLoaded()
  if (next) sync(next)
})

const currencies = computed(() => [...SUPPORTED_CURRENCIES])
/** The display currency is the quote base, so its own rate is fixed at 1. */
const editableCurrencies = computed(() =>
  currencies.value.filter(code => code !== displayCurrency.value)
)

const dirty = computed(() => {
  // Nothing loaded yet: any rate typed, or any currency other than the default,
  // is a change worth saving — otherwise a first-ever table could not be saved.
  if (!loaded.value) {
    if (displayCurrency.value !== 'USD') return true
    return Object.values(rates.value).some(value => (value || '').trim() !== '')
  }
  if (loaded.value.display_currency !== displayCurrency.value) return true
  const before = loaded.value.rates
  const now = rates.value
  for (const code of new Set([...Object.keys(before), ...Object.keys(now)])) {
    if ((before[code] || '').trim() !== (now[code] || '').trim()) return true
  }
  return false
})

const rateStatus = computed(() => {
  if (!loaded.value) return t('ui.ratesNotLoadedYet')
  if (!loaded.value.fetched_at_unix_secs) return t('ui.noRatesFetchedYet')
  const when = new Date(loaded.value.fetched_at_unix_secs * 1000)
  const source = loaded.value.source === 'live' ? t('ui.liveFeed') : t('ui.manualEntry')
  const stale = loaded.value.stale ? ` · ${t('ui.staleRefreshRecommended')}` : ''
  return `${source} · ${when.toLocaleString()}${stale}`
})

function pickDisplayCurrency(value: string) {
  if (!(SUPPORTED_CURRENCIES as readonly string[]).includes(value)) return
  displayCurrency.value = value as SupportedCurrency
}

function editRate(code: string, value: string) {
  rates.value = { ...rates.value, [code]: value }
}

async function refresh() {
  refreshing.value = true
  try {
    // The composable reports a failed fetch as a null table rather than a
    // rejection, so the outcome is read from the result, not from a catch.
    const next = await pullRates()
    if (!next) {
      error(t('ui.failedToFetchExchangeRatesKeepingTheLastKnownOnes'))
      return
    }
    sync(next)
    success(t('ui.exchangeRatesUpdated'))
  } catch (cause) {
    const failure = cause as { response?: { data?: { detail?: string } }; message?: string }
    error(
      failure.response?.data?.detail ||
        failure.message ||
        t('ui.failedToFetchExchangeRatesKeepingTheLastKnownOnes')
    )
    log.error('刷新汇率失败:', cause)
  } finally {
    refreshing.value = false
  }
}

async function save() {
  if (saving.value || !dirty.value) return
  saving.value = true
  // Only rates the gateway's fixed-point parser accepts are sent: a typo must
  // not persist and then silently stop converting.
  const cleaned: Record<string, string> = {}
  for (const code of Object.keys(rates.value)) {
    const value = (rates.value[code] || '').trim()
    if (!value) continue
    if (!/^\d+(\.\d{1,8})?$/.test(value) || Number(value) <= 0) {
      error(t('ui.rateMustBeAPositiveNumberWithAtMostEightDecimals', { code }))
      saving.value = false
      return
    }
    cleaned[code] = value
  }
  try {
    // A hand edit inherits the previous fetch time, so tweaking one rate does
    // not mark the rest of the table as freshly fetched.
    const inherited = Boolean(loaded.value?.fetched_at_unix_secs)
    const fetchedAt = loaded.value?.fetched_at_unix_secs || Math.floor(Date.now() / 1000)
    await fxApi.saveDisplayCurrency(displayCurrency.value)
    await fxApi.saveManualRates(cleaned, fetchedAt)
    const next: FxRateTable = {
      base: 'USD',
      display_currency: displayCurrency.value,
      source: 'manual',
      fetched_at_unix_secs: fetchedAt,
      stale: inherited ? Boolean(loaded.value?.stale) : false,
      supported: [...SUPPORTED_CURRENCIES],
      rates: { USD: '1', ...cleaned },
    }
    sync(next)
    adopt(next)
    success(t('ui.currencySettingsSaved'))
  } catch (cause) {
    error(t('ui.failedToSaveCurrencySettings'))
    log.error('保存币种设置失败:', cause)
  } finally {
    saving.value = false
  }
}
</script>
