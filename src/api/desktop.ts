import { invoke } from '@tauri-apps/api/core'

export interface LocaleInfo {
  locale: string
  is_zh: boolean
}

export async function detectSystemLocale(): Promise<LocaleInfo> {
  return invoke<LocaleInfo>('detect_system_locale')
}

export type NotifyKind = 'success' | 'error' | 'info'

export async function sendNotification(title: string, body: string, kind: NotifyKind = 'info'): Promise<void> {
  return invoke('send_notification', { title, body, kind })
}
