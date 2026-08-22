import { useTranslation } from 'react-i18next'
import { detectSystemLocale } from '../api/desktop'
import { LANG_STORAGE_KEY } from '../i18n'
import { getItem, setItem } from '../utils/storage'

/** 语言切换按钮：中 → EN → 自动 → 中，循环切换 */
export default function LangToggle() {
  const { i18n, t } = useTranslation()
  const saved = getItem(LANG_STORAGE_KEY)
  const mode: 'auto' | 'zh' | 'en' = saved === 'auto' ? 'auto' : i18n.language === 'en' ? 'en' : 'zh'

  const cycle = async () => {
    // 中 → EN → 自动 → 中
    const nextMode = mode === 'zh' ? 'en' : mode === 'en' ? 'auto' : 'zh'
    if (nextMode === 'auto') {
      setItem(LANG_STORAGE_KEY, 'auto')
      try {
        const info = await detectSystemLocale()
        i18n.changeLanguage(info.is_zh ? 'zh' : 'en')
      } catch {
        i18n.changeLanguage('zh')
      }
    } else {
      setItem(LANG_STORAGE_KEY, nextMode)
      i18n.changeLanguage(nextMode)
    }
  }

  const label = mode === 'auto' ? 'AUTO' : mode === 'zh' ? 'EN' : '中'
  const title =
    mode === 'auto'
      ? t('common.langSystem')
      : mode === 'zh'
        ? 'Switch to English'
        : '切换到中文'

  return (
    <button
      onClick={cycle}
      className="shrink-0 px-1.5 py-0.5 rounded text-xs font-mono hover:bg-[var(--bg-card)] transition-colors text-[var(--text-secondary)]"
      title={title}
    >
      {label}
    </button>
  )
}
