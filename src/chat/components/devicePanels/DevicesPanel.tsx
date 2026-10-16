/**
 * 设备面板：列出 hdc 设备、型号、在线状态、设为默认。
 * （含设备详情行格式化 deviceDetailRows——只有本面板用）
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { convertFileSrc } from '@tauri-apps/api/core'
import {
  listDevices,
  setDefaultDevice,
  getDeviceDetail,
  hdcAvailable,
  startHdcService,
  stopHdcService,
  captureDeviceScreenshot,
  listDeviceScreenshots,
  deleteDeviceScreenshot,
  listInstalledApps,
  launchApp,
  stopApp,
  listDeviceProcesses,
  startHilogStream,
  stopHilogStream,
  onHilogLine,
  onHilogEnded,
  getDevicePerf,
  type DeviceInfo,
  type DeviceDetail,
  type InstalledApp,
  type DeviceProcess,
  type DevicePerf,
  type ShotFile,
} from '../../../api/devices'
import Icon from '../../../icons/Icon'
import { PerfCard } from './PerfCard'

/* ============ 设备面板：列出 hdc 设备、型号、在线状态、设为默认 ============ */
export function DevicesPanel({
  projectId,
  projectName,
  onChanged,
  onSendImage,
}: {
  projectId?: string
  projectName?: string
  onChanged: () => void
  /** 发送图片到对话（截图右键菜单「发送到对话」，data URL 注入输入框附件） */
  onSendImage?: (dataUrl: string) => void
}) {
  const { t } = useTranslation()
  const [devices, setDevices] = useState<DeviceInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [setting, setSetting] = useState<string | null>(null)
  // hdc 服务可用状态（null=探测中 / true=可用 / false=不可用）
  const [hdcOk, setHdcOk] = useState<boolean | null>(null)
  const [svcBusy, setSvcBusy] = useState(false)
  // 设备详情：展开卡片 id + 缓存详情（避免重复查询）
  const [expanded, setExpanded] = useState<string | null>(null)
  const [detailMap, setDetailMap] = useState<Record<string, DeviceDetail>>({})
  const [detailBusy, setDetailBusy] = useState<string | null>(null)
  // 截屏：进行中的设备 id + 预览相对路径
  const [shotBusy, setShotBusy] = useState<string | null>(null)
  const [shotPreview, setShotPreview] = useState<string | null>(null)
  const [shotError, setShotError] = useState<string | null>(null)
  // 截图记录：项目截图目录列表（时间倒序）+ 右键菜单（复制/发送/删除）
  const [shots, setShots] = useState<ShotFile[]>([])
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; file: ShotFile } | null>(null)
  // 应用/进程管理：每个设备的子 tab（apps | procs | log | perf）+ 数据缓存
  const [tabMap, setTabMap] = useState<Record<string, 'apps' | 'procs' | 'log' | 'perf'>>({})
  const [appsMap, setAppsMap] = useState<Record<string, InstalledApp[]>>({})
  const [procsMap, setProcsMap] = useState<Record<string, DeviceProcess[]>>({})
  const [appsBusy, setAppsBusy] = useState<string | null>(null)
  const [appFilter, setAppFilter] = useState('')
  const [opBusy, setOpBusy] = useState<string | null>(null)
  // 实时 hilog：每个设备的运行状态/过滤条件/缓存行
  const [hilogActive, setHilogActive] = useState<Record<string, boolean>>({})
  const [hilogOpts, setHilogOpts] = useState<Record<string, { pkg: string; tag: string; level: string }>>({})
  const [hilogLines, setHilogLines] = useState<Record<string, string[]>>({})
  const [hilogErr, setHilogErr] = useState<Record<string, string>>({})
  const hilogRef = useRef<HTMLDivElement | null>(null)
  // 性能监控：每个设备的采样序列（最近 40 个点）+ 采样定时器
  const [perfMap, setPerfMap] = useState<Record<string, DevicePerf[]>>({})
  const [perfErr, setPerfErr] = useState<Record<string, string>>({})
  const perfTimerRef = useRef<Record<string, ReturnType<typeof setInterval>>>({})

  // 订阅后端 hilog 行/结束事件（面板存活期一次）
  useEffect(() => {
    let unlistenLine: (() => void) | undefined
    let unlistenEnd: (() => void) | undefined
    void onHilogLine((p) => {
      setHilogLines((m) => {
        const arr = m[p.device_id] ?? []
        const next = arr.length >= 800 ? [...arr.slice(arr.length - 700), p.line] : [...arr, p.line]
        return { ...m, [p.device_id]: next }
      })
    }).then((u) => { unlistenLine = u })
    void onHilogEnded((id) => {
      setHilogActive((m) => ({ ...m, [id]: false }))
    }).then((u) => { unlistenEnd = u })
    return () => {
      unlistenLine?.()
      unlistenEnd?.()
    }
  }, [])

  // 新日志到达时自动滚到底部
  useEffect(() => {
    const el = hilogRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [hilogLines])

  // 面板卸载时停止所有实时流，避免后台残留
  useEffect(() => {
    return () => {
      Object.keys(hilogActive).forEach((id) => {
        void stopHilogStream(id)
      })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 性能监控：展开且选中 perf tab 的设备每 2s 采样一次
  useEffect(() => {
    // 找出需要采样的设备：在线、已展开、当前 tab 为 perf
    const target = devices.find(
      (d) => expanded === d.id && d.state === 'Online' && tabMap[d.id] === 'perf',
    )
    // 停止不再需要采样的定时器
    Object.keys(perfTimerRef.current).forEach((id) => {
      if (!target || id !== target.id) {
        clearInterval(perfTimerRef.current[id])
        delete perfTimerRef.current[id]
      }
    })
    if (!target) return
    if (perfTimerRef.current[target.id]) return
    setPerfErr((m) => ({ ...m, [target.id]: '' }))
    const tick = async () => {
      try {
        const p = await getDevicePerf(target.id)
        setPerfMap((m) => {
          const arr = m[target.id] ?? []
          const next = [...arr, p]
          if (next.length > 40) next.splice(0, next.length - 40)
          return { ...m, [target.id]: next }
        })
      } catch (e) {
        setPerfErr((m) => ({ ...m, [target.id]: e instanceof Error ? e.message : String(e) }))
      }
    }
    void tick()
    const timers = perfTimerRef.current
    const timerId = setInterval(() => void tick(), 2000)
    timers[target.id] = timerId
    return () => {
      if (timers[target.id] === timerId) {
        clearInterval(timerId)
        delete timers[target.id]
      }
    }
  }, [devices, expanded, tabMap])

  const toggleHilog = async (deviceId: string) => {
    const running = hilogActive[deviceId]
    if (running) {
      try { await stopHilogStream(deviceId) } catch { /* ignore */ }
      setHilogActive((m) => ({ ...m, [deviceId]: false }))
      return
    }
    const o = hilogOpts[deviceId] ?? { pkg: '', tag: '', level: '' }
    setHilogErr((m) => ({ ...m, [deviceId]: '' }))
    try {
      await startHilogStream(deviceId, {
        package: o.pkg.trim() || undefined,
        tag: o.tag.trim() || undefined,
        level: (o.level || undefined) as 'D' | 'I' | 'W' | 'E' | 'F' | undefined,
      })
      setHilogActive((m) => ({ ...m, [deviceId]: true }))
      if (!hilogLines[deviceId]) setHilogLines((m) => ({ ...m, [deviceId]: [] }))
    } catch (e) {
      setHilogErr((m) => ({ ...m, [deviceId]: e instanceof Error ? e.message : String(e) }))
    }
  }

  const refresh = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const list = await listDevices()
      setDevices(list)
      setHdcOk(true)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setError(msg)
      // 枚举失败说明 hdc 未安装或服务未启动
      setHdcOk(false)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh()
    // 初始探测 hdc 可用性（listDevices 失败时 refresh 也会置 false）
    hdcAvailable().then((ok) => setHdcOk(ok)).catch(() => setHdcOk(false))
    const timer = setInterval(refresh, 5000)
    return () => clearInterval(timer)
  }, [refresh])

  const handleSetDefault = async (id: string) => {
    setSetting(id)
    try {
      await setDefaultDevice(id)
      await refresh()
      onChanged()
    } finally {
      setSetting(null)
    }
  }

  /** 启动/停止 hdc 服务端，操作后刷新列表 */
  const toggleService = async (start: boolean) => {
    setSvcBusy(true)
    setError(null)
    try {
      if (start) await startHdcService()
      else await stopHdcService()
      // daemon 启停需要一点时间，等待后再刷新
      await new Promise((r) => setTimeout(r, start ? 1000 : 500))
      await refresh()
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSvcBusy(false)
    }
  }

  /** 展开/收起设备详情（首次展开时异步查询并缓存） */
  const toggleDetail = async (d: DeviceInfo) => {
    if (expanded === d.id) {
      setExpanded(null)
      return
    }
    setExpanded(d.id)
    if (!detailMap[d.id]) {
      setDetailBusy(d.id)
      try {
        const det = await getDeviceDetail(d.id)
        setDetailMap((m) => ({ ...m, [d.id]: det }))
      } catch {
        // 详情查询失败：缓存空对象，展开区显示失败提示
        setDetailMap((m) => ({ ...m, [d.id]: { brand: '', manufacturer: '', model: '', os_version: '', resolution: '', battery: '', battery_status: '', battery_temp: '', storage: '', cpu_freq: '', ram: '' } }))
      } finally {
        setDetailBusy(null)
      }
    }
  }

  /** 截取设备屏幕：截图到项目 screenshots 目录并预览，完成后刷新截图记录 */
  const handleCapture = async (deviceId: string) => {
    if (!projectId) return
    setShotBusy(deviceId)
    setShotError(null)
    try {
      const path = await captureDeviceScreenshot(projectId, deviceId, projectName)
      setShotPreview(path)
      void loadShots()
    } catch (e) {
      setShotError(e instanceof Error ? e.message : String(e))
    } finally {
      setShotBusy(null)
    }
  }

  /** 加载项目截图记录（时间倒序），projectId 变化/截图完成后调用 */
  const loadShots = useCallback(async () => {
    if (!projectId) return
    try {
      const list = await listDeviceScreenshots(projectId)
      setShots(list)
    } catch {
      // 静默：截图目录不存在等场景不打扰
    }
  }, [projectId])
  useEffect(() => {
    void loadShots()
  }, [loadShots])

  /** 复制截图图片到剪贴板（fetch asset → ClipboardItem；失败回退复制路径） */
  const copyShotImage = async (f: ShotFile) => {
    try {
      const res = await fetch(convertFileSrc(f.path))
      const blob = await res.blob()
      await navigator.clipboard.write([
        new ClipboardItem({ [blob.type || 'image/png']: blob }),
      ])
    } catch {
      await navigator.clipboard.writeText(f.path)
    } finally {
      setCtxMenu(null)
    }
  }

  /** 复制截图文件路径 */
  const copyShotPath = async (f: ShotFile) => {
    try {
      await navigator.clipboard.writeText(f.path)
    } catch {
      // 忽略
    } finally {
      setCtxMenu(null)
    }
  }

  /** 发送截图到对话（data URL 注入输入框附件，随消息多模态上传） */
  const sendShotToChat = async (f: ShotFile) => {
    try {
      const res = await fetch(convertFileSrc(f.path))
      const blob = await res.blob()
      const dataUrl = await new Promise<string>((resolve, reject) => {
        const r = new FileReader()
        r.onload = () => resolve(String(r.result))
        r.onerror = () => reject(r.error)
        r.readAsDataURL(blob)
      })
      onSendImage?.(dataUrl)
    } catch {
      // 读取失败静默（用户可改用复制路径）
    } finally {
      setCtxMenu(null)
    }
  }

  /** 删除一张截图（本地删除 + 列表移除） */
  const deleteShotItem = async (f: ShotFile) => {
    if (!projectId) return
    try {
      await deleteDeviceScreenshot(projectId, f.name)
      setShots((cur) => cur.filter((s) => s.name !== f.name))
      if (shotPreview === f.path) setShotPreview(null)
    } catch {
      // 忽略
    } finally {
      setCtxMenu(null)
    }
  }

  // 截图删除两击确认：首次点击进入确认态（3 秒后自动恢复），再次点击才真正删除
  const [confirmDeleteShot, setConfirmDeleteShot] = useState<string | null>(null)
  const deleteShot = async (f: ShotFile) => {
    if (confirmDeleteShot !== f.path) {
      setConfirmDeleteShot(f.path)
      setTimeout(() => setConfirmDeleteShot((c) => (c === f.path ? null : c)), 3000)
      return
    }
    setConfirmDeleteShot(null)
    await deleteShotItem(f)
  }

  /** 切换设备子 tab（apps/procs/log），首次进入时加载数据 */
  const switchTab = async (deviceId: string, tab: 'apps' | 'procs' | 'log' | 'perf') => {
    setTabMap((m) => ({ ...m, [deviceId]: tab }))
    if (tab === 'apps' && !appsMap[deviceId]) {
      setAppsBusy(deviceId)
      try {
        const list = await listInstalledApps(deviceId)
        setAppsMap((m) => ({ ...m, [deviceId]: list }))
      } catch {
        setAppsMap((m) => ({ ...m, [deviceId]: [] }))
      } finally {
        setAppsBusy(null)
      }
    } else if (tab === 'procs' && !procsMap[deviceId]) {
      setAppsBusy(deviceId)
      try {
        const list = await listDeviceProcesses(deviceId)
        setProcsMap((m) => ({ ...m, [deviceId]: list }))
      } catch {
        setProcsMap((m) => ({ ...m, [deviceId]: [] }))
      } finally {
        setAppsBusy(null)
      }
    }
  }

  const handleLaunch = async (deviceId: string, pkg: string) => {
    setOpBusy(pkg)
    try {
      await launchApp(deviceId, pkg)
    } catch {
      // 静默失败：HarmonyOS 不同版本 ability 名差异较大，启动失败时不打断
    } finally {
      setOpBusy(null)
    }
  }

  const handleStop = async (deviceId: string, pkg: string) => {
    setOpBusy(pkg)
    try {
      await stopApp(deviceId, pkg)
      setProcsMap((m) => ({
        ...m,
        [deviceId]: (m[deviceId] ?? []).filter((p) => p.name !== pkg),
      }))
    } catch {
      // 静默
    } finally {
      setOpBusy(null)
    }
  }

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)] shrink-0">
        <span className="text-[length:var(--app-text-sm)] font-medium text-[var(--text-secondary)] flex items-center gap-1.5">
          <Icon name="phone" size={13} />
          {t('home.devices')}
          {devices.length > 0 && (
            <span className="text-[var(--text-muted)]">· {devices.length}</span>
          )}
          {/* hdc 服务状态点：绿=可用 / 红=不可用（未安装或未启动） */}
          <span
            className={`w-1.5 h-1.5 rounded-full ${
              hdcOk === true ? 'bg-[var(--success)]' : hdcOk === false ? 'bg-[var(--danger)]' : 'bg-[var(--text-muted)]'
            }`}
            title={hdcOk === false ? t('home.hdcUnavailable') : t('home.hdcService')}
          />
        </span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() => toggleService(true)}
            disabled={svcBusy || hdcOk === true}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] disabled:opacity-40 transition-colors"
            title={t('home.hdcStart')}
            aria-label={t('home.hdcStart')}
          >
            <Icon name="bolt" size={12} />
          </button>
          <button
            onClick={() => toggleService(false)}
            disabled={svcBusy || hdcOk === false}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] disabled:opacity-40 transition-colors"
            title={t('home.hdcStop')}
            aria-label={t('home.hdcStop')}
          >
            <Icon name="close" size={12} />
          </button>
          <button
            onClick={refresh}
            disabled={loading}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 transition-colors"
            title={t('common.refresh')}
            aria-label={t('common.refresh')}
          >
            <Icon name="refresh" size={13} className={loading ? 'animate-spin' : ''} />
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-2.5 space-y-2">
        {error && (
          <div className="rounded-lg border border-[var(--danger)]/30 bg-[var(--danger)]/10 px-3 py-2 text-[length:var(--app-text-xs)] text-[var(--danger)]">
            {error}
          </div>
        )}
        {!error && devices.length === 0 && !loading && (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6 py-10">
            <Icon name="phone" size={28} className="opacity-30" />
            <span className="text-[length:var(--app-text-sm)] text-[var(--text-secondary)]">{t('home.devicesEmpty')}</span>
            <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] leading-relaxed">{t('home.devicesEmptyHint')}</span>
          </div>
        )}
        {devices.map((d) => {
          const online = d.connection
            ? d.connection === 'online' && d.authorized !== false
            : d.state === 'Connected' || d.state === 'Ready' || d.state === 'Online'
          return (
            <div
              key={d.id}
              className={`rounded-xl modern-card p-3 transition-[color,background-color,border-color,box-shadow] ${
                d.is_default
                  ? 'border-[var(--accent)]/50 shadow-md shadow-[var(--accent)]/10'
                  : 'border-[var(--border)] hover:border-[var(--border-strong)]'
              }`}
            >
              <div className="flex items-start gap-2.5">
                <div className={`w-8 h-8 rounded-lg flex items-center justify-center shrink-0 ${
                  online ? 'bg-[var(--success)]/12 text-[var(--success)]' : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
                }`}>
                  <Icon name="phone" size={16} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <span className="text-[length:var(--app-text-sm)] font-medium truncate">
                      {d.model || d.id}
                    </span>
                    {d.is_default && (
                      <span className="text-[length:var(--app-text-micro)] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] font-medium shrink-0">
                        {t('home.deviceDefault')}
                      </span>
                    )}
                  </div>
                  <div className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] font-mono truncate mt-0.5" title={d.id}>
                    {d.id}
                  </div>
                  <div className="flex items-center gap-2 mt-1.5">
                    <span className={`flex items-center gap-1 text-[length:var(--app-text-xs)] ${online ? 'text-[var(--success)]' : 'text-[var(--warning)]'}`}>
                      <span className={`w-1.5 h-1.5 rounded-full ${online ? 'bg-[var(--success)]' : 'bg-[var(--warning)]'} ${online ? 'animate-pulse' : ''}`} />
                      {d.state}
                    </span>
                    {d.os_version && (
                      <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{d.os_version}</span>
                    )}
                  </div>
                  {(d.architecture || d.resolution) && (
                    <div className="flex items-center gap-1.5 mt-1 text-[length:var(--app-text-micro)] text-[var(--text-muted)]">
                      {d.architecture && <span className="font-mono">{d.architecture}</span>}
                      {d.architecture && d.resolution && <span>·</span>}
                      {d.resolution && <span>{d.resolution}</span>}
                    </div>
                  )}
                </div>
              </div>
              {online && d.capabilities && d.capabilities.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1" title={`Observed at ${d.observed_at ?? 0}`}>
                  {d.capabilities.map((capability) => (
                    <span
                      key={capability}
                      className="px-1.5 py-0.5 rounded bg-[var(--bg-hover)] text-[length:var(--app-text-micro)] text-[var(--text-muted)] font-mono"
                    >
                      {capability}
                    </span>
                  ))}
                </div>
              )}
              {!d.is_default && online && (
                <button
                  onClick={() => handleSetDefault(d.id)}
                  disabled={setting === d.id}
                  className="mt-2.5 w-full h-7 rounded-lg text-[length:var(--app-text-xs)] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 hover:bg-[var(--accent-soft)] transition-colors disabled:opacity-50"
                >
                  {setting === d.id ? t('home.deviceSetting') : t('home.deviceSetDefault')}
                </button>
              )}
              {/* 截屏按钮（在线设备，需已绑定项目目录） */}
              {online && (
                <button
                  onClick={() => handleCapture(d.id)}
                  disabled={shotBusy === d.id || !projectId}
                  className="mt-2 w-full h-6 rounded-lg text-[length:var(--app-text-xs)] border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 hover:bg-[var(--accent-soft)] transition-colors disabled:opacity-40 flex items-center justify-center gap-1"
                  title={!projectId ? t('home.deviceShotNoProject') : t('home.deviceShot')}
                >
                  <Icon name={shotBusy === d.id ? 'refresh' : 'devices'} size={11} className={shotBusy === d.id ? 'animate-spin' : ''} />
                  {shotBusy === d.id ? t('home.deviceShooting') : t('home.deviceShot')}
                </button>
              )}
              {/* 截图记录：横向滚动缩略图（时间倒序），点击放大、右键更多操作 */}
              {shots.length > 0 && (
                <div className="mt-2">
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{t('home.deviceShots')}</span>
                    <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">{shots.length}</span>
                  </div>
                  <div className="flex gap-1.5 overflow-x-auto pb-1">
                    {shots.map((f) => (
                      <div key={f.name} className="relative group/shot shrink-0">
                        <img
                          src={convertFileSrc(f.path)}
                          alt={f.name}
                          title={f.name}
                          onClick={() => { setCtxMenu(null); setShotPreview(f.path) }}
                          onContextMenu={(e) => {
                            e.preventDefault()
                            setCtxMenu({ x: e.clientX, y: e.clientY, file: f })
                          }}
                          className="w-14 h-10 object-cover rounded-md border border-[var(--border)] cursor-zoom-in hover:border-[var(--accent)] transition-colors"
                        />
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {/* 详情展开按钮 */}
              <button
                onClick={() => toggleDetail(d)}
                disabled={!online}
                className="mt-2 w-full h-6 rounded-lg text-[length:var(--app-text-xs)] border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40 flex items-center justify-center gap-1"
              >
                <Icon name="chevron-right" size={11} className={`transition-transform ${expanded === d.id ? 'rotate-90' : ''}`} />
                {expanded === d.id ? t('home.deviceDetailCollapse') : t('home.deviceDetailExpand')}
              </button>
              {/* 设备详情（品牌/系统/分辨率/电池/内存，按需查询并缓存） */}
              {expanded === d.id && (
                <div className="mt-2 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)]/60 p-2.5 space-y-1.5">
                  {detailBusy === d.id ? (
                    <div className="shimmer h-20 rounded bg-[var(--bg-hover)]" />
                  ) : (
                    <>
                      {deviceDetailRows(t, detailMap[d.id]).map((row) =>
                        row.value ? (
                          <div key={row.label} className="flex items-center justify-between gap-2 text-[length:var(--app-text-xs)]">
                            <span className="text-[var(--text-muted)] shrink-0">{row.label}</span>
                            <span className="text-[var(--text-secondary)] font-mono text-right truncate">{row.value}</span>
                          </div>
                        ) : null,
                      )}
                      {!deviceDetailRows(t, detailMap[d.id]).some((r) => r.value) && (
                        <div className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('home.deviceDetailError')}</div>
                      )}
                    </>
                  )}
                </div>
              )}
              {/* 应用/进程管理：在线设备展开后显示 tab 切换 */}
              {expanded === d.id && online && (
                <div className="mt-2">
                  <div className="flex gap-1 mb-1.5">
                    <button
                      onClick={() => void switchTab(d.id, 'apps')}
                      className={`flex-1 h-6 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
                        tabMap[d.id] === 'apps'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabApps')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'procs')}
                      className={`flex-1 h-6 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
                        tabMap[d.id] === 'procs'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabProcs')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'log')}
                      className={`flex-1 h-6 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
                        tabMap[d.id] === 'log'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabLog')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'perf')}
                      className={`flex-1 h-6 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
                        tabMap[d.id] === 'perf'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabPerf')}
                    </button>
                  </div>
                  {tabMap[d.id] === 'apps' && (
                    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)]/60 max-h-56 flex flex-col">
                      <div className="relative px-2 py-1.5 border-b border-[var(--border)]">
                        <input
                          value={appFilter}
                          onChange={(e) => setAppFilter(e.target.value)}
                          placeholder={t('home.deviceAppFilter')}
                          className="w-full h-6 pl-6 pr-2 rounded-md modern-card border-[var(--border)] text-[length:var(--app-text-xs)] outline-none focus:border-[var(--accent)]"
                        />
                        <Icon name="search" size={11} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]" />
                      </div>
                      <div className="flex-1 overflow-y-auto py-1 min-h-[80px]">
                        {appsBusy === d.id ? (
                          <div className="shimmer h-16 mx-2 rounded bg-[var(--bg-hover)]" />
                        ) : (appsMap[d.id] ?? []).length === 0 ? (
                          <div className="px-2 py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('home.deviceAppsEmpty')}</div>
                        ) : (
                          (appsMap[d.id] ?? [])
                            .filter((a) => !appFilter.trim() || a.package.toLowerCase().includes(appFilter.trim().toLowerCase()))
                            .slice(0, 50)
                            .map((a) => (
                              <div key={a.package} className="flex items-center gap-1.5 px-2 py-1 hover:bg-[var(--bg-hover)] group">
                                <Icon name="package" size={11} className="text-[var(--text-muted)] shrink-0" />
                                <span className="flex-1 min-w-0 text-[length:var(--app-text-xs)] font-mono truncate text-[var(--text-secondary)]" title={a.package}>
                                  {a.package}
                                </span>
                                {a.launcher && (
                                  <button
                                    onClick={() => void handleLaunch(d.id, a.package)}
                                    disabled={opBusy === a.package}
                                    className="px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] text-[var(--accent)] hover:bg-[var(--accent-soft)] disabled:opacity-40"
                                  >
                                    {t('home.deviceAppLaunch')}
                                  </button>
                                )}
                              </div>
                            ))
                        )}
                      </div>
                    </div>
                  )}
                  {tabMap[d.id] === 'procs' && (
                    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)]/60 max-h-56 overflow-y-auto py-1 min-h-[80px]">
                      {appsBusy === d.id ? (
                        <div className="shimmer h-16 mx-2 rounded bg-[var(--bg-hover)]" />
                      ) : (procsMap[d.id] ?? []).length === 0 ? (
                        <div className="px-2 py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('home.deviceProcsEmpty')}</div>
                      ) : (
                        (procsMap[d.id] ?? []).slice(0, 80).map((p) => (
                          <div key={`${p.pid}-${p.name}`} className="flex items-center gap-1.5 px-2 py-1 hover:bg-[var(--bg-hover)] group">
                            <span className="text-[length:var(--app-text-micro)] font-mono text-[var(--text-muted)] w-10 shrink-0">{p.pid}</span>
                            <span className="flex-1 min-w-0 text-[length:var(--app-text-xs)] font-mono truncate text-[var(--text-secondary)]" title={p.name}>
                              {p.name}
                            </span>
                            <button
                              onClick={() => void handleStop(d.id, p.name)}
                              disabled={opBusy === p.name}
                              className="px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] text-[var(--danger)] hover:bg-[var(--danger)]/10 opacity-0 group-hover:opacity-100 disabled:opacity-40"
                            >
                              {t('home.deviceProcStop')}
                            </button>
                          </div>
                        ))
                      )}
                    </div>
                  )}
                  {tabMap[d.id] === 'log' && (
                    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)]/60 flex flex-col">
                      {/* 过滤条件 + 启停/清空（flex-wrap：右侧栏过窄时换行，避免横向溢出） */}
                      <div className="flex flex-wrap items-center gap-1 px-2 py-1.5 border-b border-[var(--border)]">
                        <input
                          value={hilogOpts[d.id]?.pkg ?? ''}
                          onChange={(e) => setHilogOpts((m) => ({ ...m, [d.id]: { ...(m[d.id] ?? { tag: '', level: '' }), pkg: e.target.value } }))}
                          placeholder={t('home.deviceHilogPkg')}
                          disabled={hilogActive[d.id]}
                          className="flex-1 min-w-0 basis-20 h-6 px-1.5 rounded-md modern-card border-[var(--border)] text-[length:var(--app-text-2xs)] font-mono outline-none focus:border-[var(--accent)] disabled:opacity-50"
                        />
                        <input
                          value={hilogOpts[d.id]?.tag ?? ''}
                          onChange={(e) => setHilogOpts((m) => ({ ...m, [d.id]: { ...(m[d.id] ?? { pkg: '', level: '' }), tag: e.target.value } }))}
                          placeholder={t('home.deviceHilogTag')}
                          disabled={hilogActive[d.id]}
                          className="flex-1 min-w-0 basis-16 h-6 px-1.5 rounded-md modern-card border-[var(--border)] text-[length:var(--app-text-2xs)] font-mono outline-none focus:border-[var(--accent)] disabled:opacity-50"
                        />
                        <select
                          value={hilogOpts[d.id]?.level ?? ''}
                          onChange={(e) => setHilogOpts((m) => ({ ...m, [d.id]: { ...(m[d.id] ?? { pkg: '', tag: '' }), level: e.target.value } }))}
                          disabled={hilogActive[d.id]}
                          className="h-6 px-1 rounded-md modern-card border-[var(--border)] text-[length:var(--app-text-2xs)] outline-none focus:border-[var(--accent)] disabled:opacity-50"
                        >
                          <option value="">{t('home.deviceHilogLevelAll')}</option>
                          <option value="D">D</option>
                          <option value="I">I</option>
                          <option value="W">W</option>
                          <option value="E">E</option>
                          <option value="F">F</option>
                        </select>
                        <button
                          onClick={() => void toggleHilog(d.id)}
                          className={`ml-auto h-6 px-2 rounded-md text-[length:var(--app-text-2xs)] font-medium transition-colors ${
                            hilogActive[d.id]
                              ? 'bg-[var(--danger)]/10 text-[var(--danger)] hover:bg-[var(--danger)]/20'
                              : 'bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80'
                          }`}
                        >
                          {hilogActive[d.id] ? t('home.deviceHilogStop') : t('home.deviceHilogStart')}
                        </button>
                        <button
                          onClick={() => setHilogLines((m) => ({ ...m, [d.id]: [] }))}
                          className="h-6 px-1.5 rounded-md text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]"
                        >
                          {t('home.deviceHilogClear')}
                        </button>
                      </div>
                      {hilogErr[d.id] && (
                        <div className="px-2 py-1 text-[length:var(--app-text-2xs)] text-[var(--danger)] border-b border-[var(--border)]">{hilogErr[d.id]}</div>
                      )}
                      {/* 日志输出区 */}
                      <div
                        ref={(el) => { if (tabMap[d.id] === 'log') hilogRef.current = el }}
                        className="h-48 overflow-y-auto px-2 py-1 font-mono text-[length:var(--app-text-2xs)] leading-relaxed text-[var(--text-secondary)] whitespace-pre-wrap break-all"
                      >
                        {(hilogLines[d.id] ?? []).length === 0 ? (
                          <span className="text-[var(--text-muted)]">
                            {hilogActive[d.id] ? t('home.deviceHilogWaiting') : t('home.deviceHilogHint')}
                          </span>
                        ) : (
                          (hilogLines[d.id] ?? []).map((ln, i) => (
                            <div
                              key={i}
                              className={
                                ln.includes(' E ') || ln.includes(' E/')
                                  ? 'text-[var(--danger)]'
                                  : ln.includes(' W ') || ln.includes(' W/')
                                  ? 'text-amber-500'
                                  : ''
                              }
                            >
                              {ln}
                            </div>
                          ))
                        )}
                      </div>
                    </div>
                  )}
                  {tabMap[d.id] === 'perf' && (
                    <div className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)]/60 p-2">
                      {perfErr[d.id] && (
                        <div className="mb-1.5 text-[length:var(--app-text-2xs)] text-[var(--danger)]">{perfErr[d.id]}</div>
                      )}
                      {(perfMap[d.id] ?? []).length === 0 && !perfErr[d.id] ? (
                        <div className="py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
                          {t('home.devicePerfSampling')}
                        </div>
                      ) : (
                        (() => {
                          const seq = perfMap[d.id] ?? []
                          const last = seq[seq.length - 1]
                          return (
                            <div className="grid grid-cols-2 gap-1.5">
                              <PerfCard
                                label={t('home.devicePerfCpu')}
                                value={last?.cpu ?? -1}
                                unit="%"
                                color="#10b981"
                                data={seq.map((p) => p.cpu)}
                              />
                              <PerfCard
                                label={t('home.devicePerfMem')}
                                value={last?.mem ?? -1}
                                unit="%"
                                color="#3b82f6"
                                data={seq.map((p) => p.mem)}
                              />
                              <PerfCard
                                label={t('home.devicePerfTemp')}
                                value={last?.temp ?? -1}
                                unit="℃"
                                color="#f59e0b"
                                data={seq.map((p) => p.temp)}
                              />
                              <PerfCard
                                label={t('home.devicePerfBattery')}
                                value={last?.battery ?? -1}
                                unit="%"
                                color="#8b5cf6"
                                data={seq.map((p) => p.battery)}
                              />
                            </div>
                          )
                        })()
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
      {/* 截图右键菜单（复制图片/复制路径/发送到对话/删除）；portal 到 body 顶层，菜单跟随鼠标不受父容器影响 */}
      {ctxMenu &&
        createPortal(
        <>
          <div className="fixed inset-0 z-[59]" onClick={() => setCtxMenu(null)} onContextMenu={(e) => { e.preventDefault(); setCtxMenu(null) }} />
          <div
            className="fixed z-[60] w-40 rounded-lg modern-card shadow-lg py-1 text-[length:var(--app-text-xs)] animate-fade-in-up"
            style={{
              left: Math.min(ctxMenu.x, window.innerWidth - 176),
              top: Math.min(ctxMenu.y, window.innerHeight - 180),
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              onClick={() => void copyShotImage(ctxMenu.file)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
            >
              <Icon name="copy" size={11} /> {t('home.deviceShotCopy')}
            </button>
            <button
              onClick={() => void copyShotPath(ctxMenu.file)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
            >
              <Icon name="file" size={11} /> {t('home.deviceShotCopyPath')}
            </button>
            {onSendImage && (
              <button
                onClick={() => void sendShotToChat(ctxMenu.file)}
                className="w-full flex items-center gap-2 px-3 py-1.5 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
              >
                <Icon name="send" size={11} /> {t('home.deviceShotSend')}
              </button>
            )}
            <div className="my-1 border-t border-[var(--border)]" />
            <button
              onClick={() => void deleteShotItem(ctxMenu.file)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-[var(--danger)] hover:bg-[var(--danger)]/10 transition-colors"
            >
              <Icon name="delete" size={11} /> {t('home.deviceShotDelete')}
            </button>
          </div>
        </>,
          document.body,
        )}
      {/* 截图预览浮层（截图失败时也弹出，避免无反馈）；portal 到 body 顶层，不受父容器裁剪 */}
      {(shotPreview || shotError) && (() => {
        // 当前预览的文件（截图失败时无文件，仅展示错误）
        const previewFile = shotPreview ? shots.find((s) => s.path === shotPreview) : undefined
        return createPortal(
        <div
          className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6"
          onClick={() => { setShotPreview(null); setShotError(null) }}
        >
          <div
            className="relative max-w-[80vw] rounded-xl overflow-hidden modern-card border-[var(--border)] shadow-2xl animate-fade-in-up"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--border)]">
              <Icon name="devices" size={13} className="text-[var(--accent)]" />
              <span className="text-[length:var(--app-text-xs)] font-mono truncate text-[var(--text-secondary)]">
                {shotPreview ? shotPreview.split(/[\\/]/).pop() : t('home.deviceShotFailed')}
              </span>
              {shotError && <span className="text-[length:var(--app-text-xs)] text-[var(--danger)] ml-1">{shotError}</span>}
              <button
                onClick={() => setShotPreview(null)}
                aria-label={t('common.close')}
                title={t('common.close')}
                className="ml-auto p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]"
              >
                <Icon name="close" size={14} />
              </button>
            </div>
            {shotPreview && (
              <img
                src={convertFileSrc(shotPreview)}
                alt="设备截图"
                className="block mx-auto max-w-full max-h-[72vh] object-contain"
                onContextMenu={(e) => {
                  if (!previewFile) return
                  e.preventDefault()
                  setCtxMenu({ x: e.clientX, y: e.clientY, file: previewFile })
                }}
              />
            )}
            {/* 操作按钮条：复制/发送/删除（与缩略图右键菜单一致；flex-wrap 小窗口时换行） */}
            {previewFile && (
              <div className="flex flex-wrap items-center gap-1 px-3 py-2 border-t border-[var(--border)]">
                <button
                  onClick={() => void copyShotImage(previewFile)}
                  className="flex items-center gap-1 px-2 h-6 rounded-md text-[length:var(--app-text-xs)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                >
                  <Icon name="copy" size={11} /> {t('home.deviceShotCopy')}
                </button>
                <button
                  onClick={() => void copyShotPath(previewFile)}
                  className="flex items-center gap-1 px-2 h-6 rounded-md text-[length:var(--app-text-xs)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                >
                  <Icon name="file" size={11} /> {t('home.deviceShotCopyPath')}
                </button>
                {onSendImage && (
                  <button
                    onClick={() => void sendShotToChat(previewFile)}
                    className="flex items-center gap-1 px-2 h-6 rounded-md text-[length:var(--app-text-xs)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--accent-soft)] transition-colors shrink-0"
                  >
                    <Icon name="send" size={11} /> {t('home.deviceShotSend')}
                  </button>
                )}
                {/* 危险操作分隔线 + 两击确认：删除不可恢复，与常规操作拉开距离 */}
                <span className="mx-0.5 w-px h-4 bg-[var(--border)] shrink-0" aria-hidden="true" />
                <button
                  onClick={() => void deleteShot(previewFile)}
                  className={`flex items-center gap-1 px-2 h-6 rounded-md text-[length:var(--app-text-xs)] transition-colors shrink-0 ${
                    confirmDeleteShot === previewFile.path
                      ? 'text-[var(--danger)] bg-[var(--danger)]/15'
                      : 'text-[var(--danger)] opacity-70 hover:opacity-100 hover:bg-[var(--danger)]/10'
                  }`}
                >
                  <Icon name="delete" size={11} /> {confirmDeleteShot === previewFile.path ? t('home.deviceShotDeleteConfirm') : t('home.deviceShotDelete')}
                </button>
                <span className="ml-auto shrink-0 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{t('home.deviceShotSaved')}</span>
              </div>
            )}
          </div>
        </div>,
          document.body,
        )
      })()}
    </div>
  )
}

/** 设备详情行格式化（过滤空值；电池加 %、内存 kB → GB） */
function deviceDetailRows(t: (key: string) => string, d?: DeviceDetail): { label: string; value: string }[] {
  if (!d) return []
  const n = Number(d.ram)
  const ram = Number.isFinite(n) && n > 0 ? `${(n / 1024 / 1024).toFixed(1)} GB` : ''
  return [
    { label: t('home.detailBrand'), value: d.brand },
    { label: t('home.detailManufacturer'), value: d.manufacturer },
    { label: t('home.detailOs'), value: d.os_version },
    { label: t('home.detailResolution'), value: d.resolution },
    { label: t('home.detailBattery'), value: d.battery },
    { label: t('home.detailBatteryStatus'), value: d.battery_status },
    { label: t('home.detailBatteryTemp'), value: d.battery_temp },
    { label: t('home.detailCpuFreq'), value: d.cpu_freq },
    { label: t('home.detailStorage'), value: d.storage },
    { label: t('home.detailRam'), value: ram },
  ]
}

