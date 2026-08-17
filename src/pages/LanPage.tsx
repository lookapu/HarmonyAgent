import { useTranslation } from 'react-i18next'
import LanPanel from '../components/LanPanel'
import LanTokenPanel from '../components/LanTokenPanel'

/**
 * 局域网访问独立页面（侧边栏第一入口）。
 * 服务面板 + 令牌管理独立卡片；设置页内也保留相同两块（双入口，便于直达）。
 */
export default function LanPage() {
  const { t } = useTranslation()
  return (
    <div className="h-full flex flex-col gap-4">
      <div>
        <h2 className="text-xl font-semibold">{t('nav.lan')}</h2>
        <p className="text-xs text-[var(--text-secondary)] mt-1">{t('config.lanDesc')}</p>
      </div>
      <LanPanel />
      <LanTokenPanel />
    </div>
  )
}
