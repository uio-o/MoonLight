<template>
  <PageContainer>
    <PageHeader
      :title="t('ui.telegramNotifications')"
      :description="t('ui.thirdPartyPushServiceForTheTelegramNotificationChannel')"
    />

    <div class="mt-6 space-y-6">
      <CardSection
        :title="t('ui.serviceSettings')"
        :description="t('ui.configureTelegramBotTokenChatIdAndServiceEnabledState')"
      >
        <template #actions>
          <Button
            size="sm"
            :disabled="saving"
            @click="saveConfig"
          >
            {{ saving ? t('ui.saving') : t('ui.save') }}
          </Button>
        </template>

        <div class="space-y-5">
          <div class="flex items-center justify-between gap-4 rounded-lg border border-border/70 px-4 py-3">
            <div>
              <Label class="text-sm font-medium"> {{ t('ui.enableTelegramNotifications') }} </Label>
              <p class="mt-1 text-xs text-muted-foreground">
                {{ t('ui.telegramMustBeEnabledBeforeTheNotificationServiceCanUse') }}
              </p>
            </div>
            <Switch
              v-model="enabled"
              :disabled="!canEnable"
            />
          </div>

          <div class="grid gap-4 lg:grid-cols-2">
            <div>
              <Label
                for="telegram-bot-token"
                class="block text-sm font-medium"
              >
                {{ t('ui.botToken') }}
              </Label>
              <Input
                id="telegram-bot-token"
                v-model="botTokenInput"
                masked
                autocomplete="off"
                spellcheck="false"
                :placeholder="botTokenIsSet ? t('ui.setLeaveEmptyToKeepUnchanged') : '123456789:AAH...'"
                class="mt-1"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                {{ t('modules.telegram.botTokenHint') }}
              </p>
            </div>

            <div>
              <Label
                for="telegram-chat-id"
                class="block text-sm font-medium"
              >
                {{ t('ui.chatId') }}
              </Label>
              <Input
                id="telegram-chat-id"
                v-model="chatIdInput"
                autocomplete="off"
                spellcheck="false"
                :placeholder="chatIdIsSet ? t('ui.setLeaveEmptyToKeepUnchanged') : '-1001234567890'"
                class="mt-1"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                {{ t('modules.telegram.chatIdHint') }}
              </p>
            </div>
          </div>
        </div>
      </CardSection>

      <CardSection
        :title="t('ui.notificationTemplates')"
        :description="t('ui.templatesSupportValueAndValueVariables')"
      >
        <div>
          <Label
            for="telegram-template"
            class="block text-sm font-medium"
          > {{ t('ui.templateContent') }} </Label>
          <Textarea
            id="telegram-template"
            v-model="templateInput"
            rows="10"
            class="mt-1 font-mono text-sm"
            placeholder="{title}\n\n{body}"
            spellcheck="false"
          />
          <p class="mt-1 text-xs text-muted-foreground">
            {{ t('ui.telegramTemplateHint') }}
          </p>
        </div>
      </CardSection>

      <CardSection
        :title="t('ui.testService')"
        :description="t('ui.sendATelegramTestNotificationUsingTheSavedSettings')"
      >
        <div class="flex flex-wrap gap-2">
          <Button
            variant="outline"
            :disabled="testing || !canTest"
            @click="handleTest"
          >
            {{ testing ? t('auth.register.sendingCode') : t('ui.sendTest') }}
          </Button>
          <RouterLink
            to="/admin/notification-service"
            class="inline-flex h-11 items-center rounded-xl px-3 text-sm text-primary hover:underline"
          >
            {{ t('ui.openNotificationService') }}
          </RouterLink>
        </div>

        <div
          v-if="lastTestResult.length > 0"
          class="mt-4 space-y-2"
        >
          <div
            v-for="item in lastTestResult"
            :key="item.channel"
            class="flex items-center justify-between gap-4 rounded-md border border-border px-3 py-2 text-sm"
          >
            <span>{{ formatNotificationChannel(item.channel) }}</span>
            <span :class="item.success ? 'text-green-600 dark:text-green-400' : 'text-destructive'">
              {{ item.message }}
            </span>
          </div>
        </div>
      </CardSection>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { useI18n } from '@/i18n'
const { t } = useI18n()

import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Button, Input, Label, Switch, Textarea } from '@/components/ui'
import { PageHeader, PageContainer, CardSection } from '@/components/layout'
import { adminApi } from '@/api/admin'
import { modulesApi } from '@/api/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import { formatNotificationChannel } from './notificationChannel'

const CONFIG_KEYS = {
  enabled: 'module.telegram_push.enabled',
  bot_token: 'module.telegram_push.bot_token',
  chat_id: 'module.telegram_push.chat_id',
  template: 'module.telegram_push.template',
} as const

const { success, error } = useToast()

const saving = ref(false)
const testing = ref(false)
const enabled = ref(false)
const botTokenIsSet = ref(false)
const botTokenInput = ref('')
const chatIdIsSet = ref(false)
const chatIdInput = ref('')
const templateInput = ref('')
const lastTestResult = ref<Array<{ channel: string; success: boolean; message: string }>>([])

const canEnable = computed(() => botTokenIsSet.value || botTokenInput.value.trim() !== '')
const canTest = computed(() => botTokenIsSet.value || botTokenInput.value.trim() !== '')

onMounted(() => {
  loadConfig()
})

async function loadConfig() {
  try {
    const [moduleStatus, configs] = await Promise.all([
      modulesApi.getStatus('telegram_push'),
      adminApi.getAllSystemConfigs({ cacheTtlMs: 30_000 }),
    ])
    const configsByKey = new Map(configs.map(config => [config.key, config]))
    const botToken = configsByKey.get(CONFIG_KEYS.bot_token)
    const chatId = configsByKey.get(CONFIG_KEYS.chat_id)
    const template = configsByKey.get(CONFIG_KEYS.template)

    enabled.value = moduleStatus.enabled === true
    botTokenIsSet.value = botToken?.is_set === true
    botTokenInput.value = ''
    chatIdIsSet.value = typeof chatId?.value === 'string' && chatId.value.trim() !== ''
    chatIdInput.value = ''
    templateInput.value = typeof template?.value === 'string' ? template.value : ''
  } catch (err) {
    error(parseApiError(err, t('ui.failedToLoadTelegramSettings')))
    log.error('加载 Telegram 推送配置失败:', err)
  }
}

async function saveConfig() {
  saving.value = true
  try {
    const updates: Array<Promise<unknown>> = [
      adminApi.updateSystemConfig(CONFIG_KEYS.chat_id, chatIdInput.value.trim(), 'Telegram Chat ID'),
      adminApi.updateSystemConfig(CONFIG_KEYS.template, templateInput.value, 'Telegram 推送模板'),
    ]
    const trimmedToken = botTokenInput.value.trim()
    if (trimmedToken) {
      updates.push(adminApi.updateSystemConfig(
        CONFIG_KEYS.bot_token,
        trimmedToken,
        'Telegram Bot Token'
      ))
    }
    await Promise.all(updates)
    if (trimmedToken) {
      botTokenIsSet.value = true
      botTokenInput.value = ''
    }
    if (chatIdInput.value.trim()) {
      chatIdIsSet.value = true
    }
    if (!canEnable.value) {
      enabled.value = false
    }
    await modulesApi.setEnabled('telegram_push', enabled.value)
    success(t('ui.telegramSettingsSaved'))
  } catch (err) {
    error(parseApiError(err, t('ui.failedToSaveTelegramSettings')))
    log.error('保存 Telegram 推送配置失败:', err)
  } finally {
    saving.value = false
  }
}

async function handleTest() {
  testing.value = true
  try {
    const result = await adminApi.testImportantNotification({ channel: 'telegram' })
    lastTestResult.value = result.channels || []
    if (result.success) {
      success(result.message || t('ui.testNotificationSent'))
    } else {
      error(result.message || t('ui.failedToSendTestNotification'))
    }
  } catch (err) {
    error(parseApiError(err, t('ui.failedToSendTestNotification')))
    log.error('测试 Telegram 推送失败:', err)
  } finally {
    testing.value = false
  }
}

</script>
