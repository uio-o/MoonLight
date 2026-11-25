import apiClient from './client'

export interface TelegramBindingStatus {
  module_enabled: boolean
  bot_username?: string | null
  bound: boolean
  chat_title?: string | null
  username?: string | null
  verified_at?: string | null
  last_notified_at?: string | null
  notification_count: number
  pending_code?: string | null
  pending_expires_at?: string | null
}

export interface TelegramBindingCode {
  code: string
  expires_at?: string | null
  bot_username?: string | null
  deep_link?: string | null
  webhook_ready: boolean
}

export const telegramApi = {
  async getBinding(): Promise<TelegramBindingStatus> {
    const response = await apiClient.get<TelegramBindingStatus>('/api/user/telegram/binding')
    return response.data
  },

  async createBindingCode(): Promise<TelegramBindingCode> {
    const response = await apiClient.post<TelegramBindingCode>('/api/user/telegram/binding')
    return response.data
  },

  async unbind(): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string }>('/api/user/telegram/binding')
    return response.data
  },
}
