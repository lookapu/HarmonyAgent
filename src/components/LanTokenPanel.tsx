import { useState, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { QRCodeSVG } from 'qrcode.react'
import { getLanServerStatus, createLanToken, revokeLanToken, type LanTokenInfo } from '../api/lan'

const DAY = 86400

/** 有效期档位：preset → 到期时间戳（unix 秒，0=永久） */
function expiryFromPreset(preset: string, customDate: string): number {
  const now = Math.floor(Date.now() / 1000)
  if (preset === 'never') return 0
  if (preset === '7' || preset === '30' || preset === '90') {
    return now + Number(preset) * DAY
  }
  // 自定义日期：当天 23:59:59 到期
  const t = Date.parse(customDate + 'T23:59:59')
  if (Number.isNaN(t)) return 0
  return Math.floor(t / 1000)
}

/** 时长展示（秒 → X 小时/分钟/秒） */
function fmtDuration(secs: number, t: TFunction): string {
  if (secs >= 3600) return t('config.lanDurationHours', { n: Math.round(secs / 3600) })
  if (secs >= 60) return t('config.lanDurationMins', { n: Math.round(secs / 60) })
  return t('config.lanDurationSecs', { n: secs })
}

/**
 * 已发放令牌管理（独立卡片，挂在局域网访问面板下方 / 独立 LAN 页）。
 * - 自包含拉取服务状态：令牌列表、本机 IP、实际端口
 * - 生成（名称 + 有效期）→ 明文高亮展示一次（可复制） + 独立二维码
 * - 每个未失效令牌常驻展示二维码（每 IP 一个，可直接扫码进入）；过期不展示；
 *   旧令牌（无明文）提示重建；可单独撤销
 */
export default function LanTokenPanel() {
  const { t } = useTranslation()
  const [ips, setIps] = useState<string[]>([])
  const [port, setPort] = useState(0)
  const [tokens, setTokens] = useState<LanTokenInfo[]>([])
  const [token, setToken] = useState<string | null>(null) // 新生成令牌明文（仅高亮一次）
  const [showCreate, setShowCreate] = useState(false)
  const [name, setName] = useState('')
  const [preset, setPreset] = useState('never')
  const [customDate, setCustomDate] = useState('')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const s = await getLanServerStatus()
      setIps(s.ips || [])
      setPort(s.listen_port || 0)
      setTokens(s.tokens || [])
    } catch {
      /* 状态加载失败静默，面板保持初始态 */
    }
  }, [])

  useEffect(() => {
    load()
  }, [load])

  const run = async (fn: () => Promise<unknown>, failKey: string): Promise<boolean> => {
    setBusy(true)
    setErr(null)
    setNote(null)
    try {
      await fn()
      await load()
      return true
    } catch (e) {
      setErr(`${t(failKey)}: ${e}`)
      return false
    } finally {
      setBusy(false)
    }
  }

  const handleCreate = async () => {
    if (!name.trim()) {
      setErr(t('config.lanTokenNameEmpty'))
      return
    }
    const expiresAt = expiryFromPreset(preset, customDate)
    setBusy(true)
    setErr(null)
    setNote(null)
    try {
      // createLanToken 返回明文（高亮展示一次）
      const plain = await createLanToken(name.trim(), expiresAt)
      setToken(plain)
      setShowCreate(false)
      setName('')
      setPreset('never')
      setCustomDate('')
      await load()
      setNote(t('config.lanTokenCreated'))
    } catch (e) {
      setErr(`${t('config.lanError')}: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const handleRevoke = async (tk: LanTokenInfo) => {
    if (!window.confirm(t('config.lanTokenRevokeConfirm', { name: tk.name || '#' + tk.id }))) return
    const ok = await run(() => revokeLanToken(tk.id), 'config.lanError')
    if (ok) setNote(t('config.lanTokenRevoked'))
  }

  const handleCopyToken = async () => {
    if (!token) return
    try {
      await navigator.clipboard.writeText(token)
      setNote(t('config.lanTokenCopied'))
    } catch {
      /* 剪贴板不可用时忽略 */
    }
  }

  /** 给定明文令牌，生成可扫码直达的 URL 列表（每 IP 一个；服务停止时也展示，方便预生成） */
  const qrUrls = (plain: string) =>
    ips.length > 0 ? ips.filter((ip) => ip).map((ip) => `http://${ip}:${port}/#${plain}`) : []

  /** 有效期展示文案 */
  const validityText = (tk: LanTokenInfo): string => {
    if (tk.remaining_secs < 0) return t('config.lanTokenExpired')
    if (tk.expires_at === 0) return t('config.lanTokenForever')
    return t('config.lanTokenRemaining', { days: Math.ceil(tk.remaining_secs / DAY) })
  }

  return (
    <div className="border border-[var(--border)] rounded-lg p-4 bg-[var(--bg-secondary)]">
      <div className="flex items-center justify-between mb-1">
        <h3 className="text-sm font-semibold">{t('config.lanTokenSection')}</h3>
        <button
          onClick={() => setShowCreate((v) => !v)}
          disabled={busy}
          className="px-3 py-1 border border-[var(--border)] rounded-lg text-xs hover:bg-[var(--bg-muted)] disabled:opacity-50"
        >
          {t('config.lanTokenCreate')}
        </button>
      </div>
      <p className="text-xs text-[var(--text-secondary)] mb-3">{t('config.lanTokenCreateHint')}</p>

      {/* 生成表单 */}
      {showCreate && (
        <div className="border border-[var(--border)] rounded-lg p-3 mb-3 bg-[var(--bg-primary)]">
          <input
            type="text"
            value={name}
            placeholder={t('config.lanTokenName')}
            maxLength={40}
            onChange={(e) => setName(e.target.value)}
            className="w-full px-2 py-1.5 rounded-md border border-[var(--border)] bg-[var(--bg-secondary)] text-sm text-[var(--text-primary)] mb-2"
          />
          <div className="flex flex-wrap items-center gap-2 text-sm text-[var(--text-secondary)]">
            <span>{t('config.lanTokenExpire')}:</span>
            {[
              ['never', t('config.lanTokenNever')],
              ['7', t('config.lanToken7d')],
              ['30', t('config.lanToken30d')],
              ['90', t('config.lanToken90d')],
              ['custom', t('config.lanTokenCustom')],
            ].map(([val, label]) => (
              <label key={val} className="flex items-center gap-1 cursor-pointer">
                <input
                  type="radio"
                  name="lan-expiry"
                  checked={preset === val}
                  onChange={() => setPreset(val)}
                  className="accent-[var(--accent)]"
                />
                {label}
              </label>
            ))}
          </div>
          {preset === 'custom' && (
            <input
              type="date"
              value={customDate}
              min={new Date().toISOString().slice(0, 10)}
              onChange={(e) => setCustomDate(e.target.value)}
              className="mt-2 px-2 py-1 rounded-md border border-[var(--border)] bg-[var(--bg-secondary)] text-sm text-[var(--text-primary)]"
            />
          )}
          <button
            onClick={handleCreate}
            disabled={busy}
            className="mt-2 px-3 py-1.5 btn-primary rounded-lg text-xs disabled:opacity-50"
          >
            {t('config.lanTokenCreateBtn')}
          </button>
        </div>
      )}

      {/* 新令牌明文 + 二维码（高亮展示一次） */}
      {token && (
        <div className="border border-[var(--accent)]/40 rounded-lg p-3 mb-3 bg-[var(--bg-primary)]">
          <p className="text-xs text-[var(--warning)] mb-1">{t('config.lanTokenCreated')}</p>
          <div className="flex items-center gap-2 mb-2">
            <code className="px-3 py-1.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] font-mono text-lg tracking-[0.5em] text-[var(--accent)]">
              {token}
            </code>
            <button
              onClick={handleCopyToken}
              className="px-3 py-1.5 border border-[var(--border)] rounded-lg text-xs hover:bg-[var(--bg-muted)]"
            >
              {t('config.lanTokenCopy')}
            </button>
            <button
              onClick={() => setToken(null)}
              className="px-3 py-1.5 border border-[var(--border)] rounded-lg text-xs hover:bg-[var(--bg-muted)]"
            >
              {t('config.lanTokenHide')}
            </button>
          </div>
          {qrUrls(token).length > 0 ? (
            <div className="flex flex-wrap gap-4">
              {qrUrls(token).map((url) => (
                <div
                  key={url}
                  className="p-2 rounded-lg bg-white border border-[var(--border)]"
                  title={url}
                >
                  <QRCodeSVG value={url} size={120} marginSize={1} />
                </div>
              ))}
            </div>
          ) : (
            <p className="text-xs text-[var(--warning)]">{t('config.lanNoIps')}</p>
          )}
        </div>
      )}

      {/* 令牌列表 */}
      {tokens.length === 0 && !token ? (
        <p className="text-xs text-[var(--text-secondary)]">{t('config.lanTokenEmpty')}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {tokens.map((tk) => {
            const active = tk.remaining_secs >= 0
            return (
              <li
                key={tk.id}
                className="border border-[var(--border)] rounded-lg px-3 py-2 bg-[var(--bg-primary)]"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm text-[var(--text-primary)] truncate">
                        {tk.name || `#${tk.id}`}
                      </span>
                      <span
                        className={`text-xs px-1.5 py-0.5 rounded-full ${
                          active
                            ? 'bg-[var(--bg-muted)] text-[var(--text-secondary)]'
                            : 'bg-[var(--danger)]/15 text-[var(--danger)]'
                        }`}
                      >
                        {validityText(tk)}
                      </span>
                    </div>
                    <p className="text-xs text-[var(--text-secondary)] truncate">
                      {tk.last_device
                        ? t('config.lanTokenLastUse', {
                            device: tk.last_device,
                            duration:
                              tk.last_duration_secs > 0
                                ? fmtDuration(tk.last_duration_secs, t)
                                : '',
                          })
                        : t('config.lanTokenNeverUsed')}
                    </p>
                  </div>
                  <button
                    onClick={() => handleRevoke(tk)}
                    disabled={busy}
                    className="shrink-0 px-2.5 py-1 border border-[var(--danger)]/40 text-[var(--danger)] rounded-lg text-xs hover:bg-[var(--danger)]/10 disabled:opacity-50"
                  >
                    {t('config.lanTokenRevoke')}
                  </button>
                </div>
                {/* 有效期内：二维码常驻展示，可直接扫码进入 */}
                {active && tk.token_plain && (
                  <div className="mt-2 border-t border-[var(--border)] pt-2">
                    <div className="text-xs text-[var(--text-secondary)] mb-1">
                      {t('config.lanTokenQr')}
                    </div>
                    {qrUrls(tk.token_plain).length > 0 ? (
                      <div className="flex flex-wrap gap-3">
                        {qrUrls(tk.token_plain).map((url) => (
                          <div
                            key={url}
                            className="p-1.5 rounded-lg bg-white border border-[var(--border)]"
                            title={url}
                          >
                            <QRCodeSVG value={url} size={96} marginSize={1} />
                          </div>
                        ))}
                      </div>
                    ) : (
                      <p className="text-xs text-[var(--warning)]">{t('config.lanNoIps')}</p>
                    )}
                  </div>
                )}
                {/* 有效但无明文（046 之前的旧令牌）：无法恢复二维码 */}
                {active && !tk.token_plain && (
                  <p className="mt-1 text-xs text-[var(--warning)]">{t('config.lanTokenQrLegacy')}</p>
                )}
              </li>
            )
          })}
        </ul>
      )}

      {note && <p className="text-xs text-[var(--success)] mt-2">{note}</p>}
      {err && <p className="text-xs text-[var(--danger)] mt-2">{err}</p>}
    </div>
  )
}
