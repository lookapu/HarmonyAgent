import { invokeWithError } from './invoke'
import { useNotificationStore } from '../stores/notificationStore'

export interface LocaleInfo {
  locale: string
  is_zh: boolean
}

export async function detectSystemLocale(): Promise<LocaleInfo> {
  return invokeWithError<LocaleInfo>('detect_system_locale')
}

export type NotifyKind = 'success' | 'error' | 'info'

/**
 * 统一通知总线入口：原生系统通知 + 应用内 toast（Rust 端 desktop-notify 事件）
 * 之外，同步落一条进铃铛（notificationStore），历史可查。
 *
 * 放在 invoke 之前：不论原生通知成败都记录（调用点均已 .catch(() => {})），
 * 不依赖 Rust 端是否发出 desktop-notify 事件——那是读不到、也不该依赖的细节。
 * NotifyKind 的 success/error/info 是 store 的 NotifyTone 子集，tone 直接复用。
 */
export async function sendNotification(title: string, body: string, kind: NotifyKind = 'info'): Promise<void> {
  useNotificationStore.getState().push({ tone: kind, title, body })
  return invokeWithError('send_notification', { title, body, kind })
}
