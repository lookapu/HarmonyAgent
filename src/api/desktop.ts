import { invokeWithError } from './invoke'

export interface LocaleInfo {
  locale: string
  is_zh: boolean
}

export async function detectSystemLocale(): Promise<LocaleInfo> {
  return invokeWithError<LocaleInfo>('detect_system_locale')
}

export type NotifyKind = 'success' | 'error' | 'info'

export async function sendNotification(title: string, body: string, kind: NotifyKind = 'info'): Promise<void> {
  return invokeWithError('send_notification', { title, body, kind })
}
