import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { convertFileSrc } from '@tauri-apps/api/core'
import { useProjectStore } from '../../stores/projectStore'
import { getHarmonyRoot, setHarmonyProjectPath, rescanWorkspaceModules, readProjectFile, type HarmonyRootInfo } from '../../api/project'
import {
  analyzeHarmonyProject,
  analyzeGenericProject,
  checkOhpmDeps,
  runOhpmInstall,
  type ProjectCapability,
  type AnalyzedBuildError,
  type OhpmDepCheck,
} from '../../api/harmonyAnalyze'
import {
  indexProjectSymbols,
  refreshProjectSymbols,
  searchSymbols as searchSymbolsApi,
  searchSymbolsAll,
  symbolIndexMeta,
  type CodeSymbol,
  type CrossProjectSymbol,
  type SymbolIndexMeta,
} from '../../api/symbols'
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
} from '../../api/devices'
import Icon, { type IconName } from '../../icons/Icon'

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
        <span className="text-[12px] font-medium text-[var(--text-secondary)] flex items-center gap-1.5">
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
          >
            <Icon name="bolt" size={12} />
          </button>
          <button
            onClick={() => toggleService(false)}
            disabled={svcBusy || hdcOk === false}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] disabled:opacity-40 transition-colors"
            title={t('home.hdcStop')}
          >
            <Icon name="close" size={12} />
          </button>
          <button
            onClick={refresh}
            disabled={loading}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 transition-colors"
            title={t('common.refresh')}
          >
            <Icon name="refresh" size={13} className={loading ? 'animate-spin' : ''} />
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-2.5 space-y-2">
        {error && (
          <div className="rounded-lg border border-[var(--danger)]/30 bg-[var(--danger)]/10 px-3 py-2 text-[11.5px] text-[var(--danger)]">
            {error}
          </div>
        )}
        {!error && devices.length === 0 && !loading && (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6 py-10">
            <Icon name="phone" size={28} className="opacity-30" />
            <span className="text-[12px] text-[var(--text-secondary)]">{t('home.devicesEmpty')}</span>
            <span className="text-[11px] text-[var(--text-muted)] leading-relaxed">{t('home.devicesEmptyHint')}</span>
          </div>
        )}
        {devices.map((d) => {
          const online = d.state === 'Connected' || d.state === 'Ready' || d.state === 'Online'
          return (
            <div
              key={d.id}
              className={`rounded-xl modern-card p-3 transition-all ${
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
                    <span className="text-[12.5px] font-medium truncate">
                      {d.model || d.id}
                    </span>
                    {d.is_default && (
                      <span className="text-[9px] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] font-medium shrink-0">
                        {t('home.deviceDefault')}
                      </span>
                    )}
                  </div>
                  <div className="text-[10.5px] text-[var(--text-muted)] font-mono truncate mt-0.5" title={d.id}>
                    {d.id}
                  </div>
                  <div className="flex items-center gap-2 mt-1.5">
                    <span className={`flex items-center gap-1 text-[10.5px] ${online ? 'text-[var(--success)]' : 'text-[var(--warning)]'}`}>
                      <span className={`w-1.5 h-1.5 rounded-full ${online ? 'bg-[var(--success)]' : 'bg-[var(--warning)]'} ${online ? 'animate-pulse' : ''}`} />
                      {d.state}
                    </span>
                    {d.os_version && (
                      <span className="text-[10.5px] text-[var(--text-muted)]">{d.os_version}</span>
                    )}
                  </div>
                </div>
              </div>
              {!d.is_default && online && (
                <button
                  onClick={() => handleSetDefault(d.id)}
                  disabled={setting === d.id}
                  className="mt-2.5 w-full h-7 rounded-lg text-[11px] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 hover:bg-[var(--accent-soft)] transition-colors disabled:opacity-50"
                >
                  {setting === d.id ? t('home.deviceSetting') : t('home.deviceSetDefault')}
                </button>
              )}
              {/* 截屏按钮（在线设备，需已绑定项目目录） */}
              {online && (
                <button
                  onClick={() => handleCapture(d.id)}
                  disabled={shotBusy === d.id || !projectId}
                  className="mt-2 w-full h-6 rounded-lg text-[10.5px] border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 hover:bg-[var(--accent-soft)] transition-colors disabled:opacity-40 flex items-center justify-center gap-1"
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
                    <span className="text-[10px] text-[var(--text-muted)]">{t('home.deviceShots')}</span>
                    <span className="text-[9.5px] text-[var(--text-muted)]">{shots.length}</span>
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
                className="mt-2 w-full h-6 rounded-lg text-[10.5px] border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40 flex items-center justify-center gap-1"
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
                          <div key={row.label} className="flex items-center justify-between gap-2 text-[11px]">
                            <span className="text-[var(--text-muted)] shrink-0">{row.label}</span>
                            <span className="text-[var(--text-secondary)] font-mono text-right truncate">{row.value}</span>
                          </div>
                        ) : null,
                      )}
                      {!deviceDetailRows(t, detailMap[d.id]).some((r) => r.value) && (
                        <div className="text-[10.5px] text-[var(--text-muted)]">{t('home.deviceDetailError')}</div>
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
                      className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                        tabMap[d.id] === 'apps'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabApps')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'procs')}
                      className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                        tabMap[d.id] === 'procs'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabProcs')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'log')}
                      className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                        tabMap[d.id] === 'log'
                          ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      {t('home.deviceTabLog')}
                    </button>
                    <button
                      onClick={() => void switchTab(d.id, 'perf')}
                      className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
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
                          className="w-full h-6 pl-6 pr-2 rounded-md modern-card border-[var(--border)] text-[10.5px] outline-none focus:border-[var(--accent)]"
                        />
                        <Icon name="search" size={11} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]" />
                      </div>
                      <div className="flex-1 overflow-y-auto py-1 min-h-[80px]">
                        {appsBusy === d.id ? (
                          <div className="shimmer h-16 mx-2 rounded bg-[var(--bg-hover)]" />
                        ) : (appsMap[d.id] ?? []).length === 0 ? (
                          <div className="px-2 py-3 text-center text-[10.5px] text-[var(--text-muted)]">{t('home.deviceAppsEmpty')}</div>
                        ) : (
                          (appsMap[d.id] ?? [])
                            .filter((a) => !appFilter.trim() || a.package.toLowerCase().includes(appFilter.trim().toLowerCase()))
                            .slice(0, 50)
                            .map((a) => (
                              <div key={a.package} className="flex items-center gap-1.5 px-2 py-1 hover:bg-[var(--bg-hover)] group">
                                <Icon name="package" size={11} className="text-[var(--text-muted)] shrink-0" />
                                <span className="flex-1 min-w-0 text-[10.5px] font-mono truncate text-[var(--text-secondary)]" title={a.package}>
                                  {a.package}
                                </span>
                                {a.launcher && (
                                  <button
                                    onClick={() => void handleLaunch(d.id, a.package)}
                                    disabled={opBusy === a.package}
                                    className="px-1.5 py-0.5 rounded text-[9.5px] text-[var(--accent)] hover:bg-[var(--accent-soft)] disabled:opacity-40"
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
                        <div className="px-2 py-3 text-center text-[10.5px] text-[var(--text-muted)]">{t('home.deviceProcsEmpty')}</div>
                      ) : (
                        (procsMap[d.id] ?? []).slice(0, 80).map((p) => (
                          <div key={`${p.pid}-${p.name}`} className="flex items-center gap-1.5 px-2 py-1 hover:bg-[var(--bg-hover)] group">
                            <span className="text-[9.5px] font-mono text-[var(--text-muted)] w-10 shrink-0">{p.pid}</span>
                            <span className="flex-1 min-w-0 text-[10.5px] font-mono truncate text-[var(--text-secondary)]" title={p.name}>
                              {p.name}
                            </span>
                            <button
                              onClick={() => void handleStop(d.id, p.name)}
                              disabled={opBusy === p.name}
                              className="px-1.5 py-0.5 rounded text-[9.5px] text-[var(--danger)] hover:bg-[var(--danger)]/10 opacity-0 group-hover:opacity-100 disabled:opacity-40"
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
                          className="flex-1 min-w-0 basis-20 h-6 px-1.5 rounded-md modern-card border-[var(--border)] text-[10px] font-mono outline-none focus:border-[var(--accent)] disabled:opacity-50"
                        />
                        <input
                          value={hilogOpts[d.id]?.tag ?? ''}
                          onChange={(e) => setHilogOpts((m) => ({ ...m, [d.id]: { ...(m[d.id] ?? { pkg: '', level: '' }), tag: e.target.value } }))}
                          placeholder={t('home.deviceHilogTag')}
                          disabled={hilogActive[d.id]}
                          className="flex-1 min-w-0 basis-16 h-6 px-1.5 rounded-md modern-card border-[var(--border)] text-[10px] font-mono outline-none focus:border-[var(--accent)] disabled:opacity-50"
                        />
                        <select
                          value={hilogOpts[d.id]?.level ?? ''}
                          onChange={(e) => setHilogOpts((m) => ({ ...m, [d.id]: { ...(m[d.id] ?? { pkg: '', tag: '' }), level: e.target.value } }))}
                          disabled={hilogActive[d.id]}
                          className="h-6 px-1 rounded-md modern-card border-[var(--border)] text-[10px] outline-none focus:border-[var(--accent)] disabled:opacity-50"
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
                          className={`ml-auto h-6 px-2 rounded-md text-[10px] font-medium transition-colors ${
                            hilogActive[d.id]
                              ? 'bg-[var(--danger)]/10 text-[var(--danger)] hover:bg-[var(--danger)]/20'
                              : 'bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80'
                          }`}
                        >
                          {hilogActive[d.id] ? t('home.deviceHilogStop') : t('home.deviceHilogStart')}
                        </button>
                        <button
                          onClick={() => setHilogLines((m) => ({ ...m, [d.id]: [] }))}
                          className="h-6 px-1.5 rounded-md text-[10px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]"
                        >
                          {t('home.deviceHilogClear')}
                        </button>
                      </div>
                      {hilogErr[d.id] && (
                        <div className="px-2 py-1 text-[10px] text-[var(--danger)] border-b border-[var(--border)]">{hilogErr[d.id]}</div>
                      )}
                      {/* 日志输出区 */}
                      <div
                        ref={(el) => { if (tabMap[d.id] === 'log') hilogRef.current = el }}
                        className="h-48 overflow-y-auto px-2 py-1 font-mono text-[10px] leading-relaxed text-[var(--text-secondary)] whitespace-pre-wrap break-all"
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
                        <div className="mb-1.5 text-[10px] text-[var(--danger)]">{perfErr[d.id]}</div>
                      )}
                      {(perfMap[d.id] ?? []).length === 0 && !perfErr[d.id] ? (
                        <div className="py-3 text-center text-[10.5px] text-[var(--text-muted)]">
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
            className="fixed z-[60] w-40 rounded-lg modern-card shadow-lg py-1 text-[11px] animate-fade-in-up"
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
              <span className="text-[11px] font-mono truncate text-[var(--text-secondary)]">
                {shotPreview ? shotPreview.split(/[\\/]/).pop() : t('home.deviceShotFailed')}
              </span>
              {shotError && <span className="text-[10.5px] text-[var(--danger)] ml-1">{shotError}</span>}
              <button
                onClick={() => setShotPreview(null)}
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
                  className="flex items-center gap-1 px-2 h-6 rounded-md text-[10.5px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                >
                  <Icon name="copy" size={11} /> {t('home.deviceShotCopy')}
                </button>
                <button
                  onClick={() => void copyShotPath(previewFile)}
                  className="flex items-center gap-1 px-2 h-6 rounded-md text-[10.5px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                >
                  <Icon name="file" size={11} /> {t('home.deviceShotCopyPath')}
                </button>
                {onSendImage && (
                  <button
                    onClick={() => void sendShotToChat(previewFile)}
                    className="flex items-center gap-1 px-2 h-6 rounded-md text-[10.5px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--accent-soft)] transition-colors shrink-0"
                  >
                    <Icon name="send" size={11} /> {t('home.deviceShotSend')}
                  </button>
                )}
                {/* 危险操作分隔线 + 两击确认：删除不可恢复，与常规操作拉开距离 */}
                <span className="mx-0.5 w-px h-4 bg-[var(--border)] shrink-0" aria-hidden="true" />
                <button
                  onClick={() => void deleteShot(previewFile)}
                  className={`flex items-center gap-1 px-2 h-6 rounded-md text-[10.5px] transition-colors shrink-0 ${
                    confirmDeleteShot === previewFile.path
                      ? 'text-[var(--danger)] bg-[var(--danger)]/15'
                      : 'text-[var(--danger)] opacity-70 hover:opacity-100 hover:bg-[var(--danger)]/10'
                  }`}
                >
                  <Icon name="delete" size={11} /> {confirmDeleteShot === previewFile.path ? t('home.deviceShotDeleteConfirm') : t('home.deviceShotDelete')}
                </button>
                <span className="ml-auto shrink-0 text-[10px] text-[var(--text-muted)]">{t('home.deviceShotSaved')}</span>
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

/* ============ 工程能力分析面板：构建错误智能分析 + Kit/权限/依赖/模块盘点 + ohpm 依赖管理 ============ */
export function AnalyzePanel({
  projectPath,
  projectId,
  projectName,
  onRunBuild,
  onFixErrors,
  onAutoFix,
  refreshTick,
  moduleScanTick,
  agentBusy,
  onHarmonyRootChanged,
  root,
}: {
  projectPath: string
  projectId: string
  projectName: string
  onRunBuild: () => void
  onFixErrors: (errors: AnalyzedBuildError[]) => void
  onAutoFix: (errors: AnalyzedBuildError[]) => void
  refreshTick: number
  moduleScanTick: number
  agentBusy: boolean
  onHarmonyRootChanged: () => void
  /** 会话工作目录（worktree 模式为 worktree 路径，本地模式为 undefined） */
  root?: string
}) {
  const { t } = useTranslation()
  const [data, setData] = useState<ProjectCapability | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 非鸿蒙工程（混合工作区其它子工程 / 纯其它语言工程）的通用概览文本
  const [genericOverview, setGenericOverview] = useState<string | null>(null)
  const [sub, setSub] = useState<'errors' | 'cap' | 'deps'>('errors')
  const [fixing, setFixing] = useState(false)
  // 会话"鸿蒙主工程"：混合工作区中实际进行鸿蒙开发的子工程（未配置时自动兜底）
  const [rootInfo, setRootInfo] = useState<HarmonyRootInfo | null>(null)
  const [rootLoading, setRootLoading] = useState(false)
  const rescannedRef = useRef(false)
  // ohpm 依赖版本核对 + 安装
  const [depChecks, setDepChecks] = useState<OhpmDepCheck[] | null>(null)
  const [depsLoading, setDepsLoading] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [depsMsg, setDepsMsg] = useState<string | null>(null)
  // 修复闭环反馈：记录 Agent 修复前的错误数，刷新后若清零则显示成功提示
  const [fixedFrom, setFixedFrom] = useState(0)
  // 记录"点击自动修复"瞬间的错误数，作为修复成功判定基准
  const triggerFixSnapshot = useCallback((count: number) => {
    setFixedFrom(count)
  }, [])

  // 解析"鸿蒙主工程"：配置了用配置；未配置且工作区仅一个鸿蒙模块时自动兜底；
  // 无任何鸿蒙候选时尝试触发一次工作区扫描（旧项目可能未扫描过）再查
  useEffect(() => {
    if (!projectId) return
    let cancelled = false
    setRootLoading(true)
    void (async () => {
      try {
        let info = await getHarmonyRoot(projectId, root)
        if (
          !cancelled &&
          !rescannedRef.current &&
          info.candidates.length === 0 &&
          info.configured == null &&
          info.root === projectPath
        ) {
          rescannedRef.current = true
          try {
            await rescanWorkspaceModules(projectId)
            if (!cancelled) info = await getHarmonyRoot(projectId, root)
          } catch {
            // 扫描失败静默，保留原解析结果
          }
        }
        if (!cancelled) setRootInfo(info)
      } catch {
        if (!cancelled) setRootInfo(null)
      } finally {
        if (!cancelled) setRootLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [projectId, projectPath, root, moduleScanTick])

  // 实际分析目录：解析后的鸿蒙主工程根（=项目根本身时行为不变）
  const effPath = rootInfo?.root ?? projectPath

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    setGenericOverview(null)
    try {
      const cap = await analyzeHarmonyProject(effPath)
      setData(cap)
    } catch {
      // 目标不是鸿蒙工程（混合工作区中的前端/Go/Java 等子工程）：降级为通用工程概览
      setData(null)
      try {
        setGenericOverview(await analyzeGenericProject(effPath))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      setLoading(false)
    }
  }, [effPath])

  // 切换"鸿蒙主工程"：持久化并触发父级刷新项目引用，随后重新分析
  const handleRootChange = async (value: string) => {
    try {
      await setHarmonyProjectPath(projectId, value === projectPath ? '' : value, root)
      onHarmonyRootChanged()
      setRootInfo(await getHarmonyRoot(projectId, root).catch(() => null))
      setData(null)
      setGenericOverview(null)
      setDepChecks(null)
      void load()
    } catch {
      // 设置失败保留原状态
    }
  }

  const rootLabel = (abs: string) => {
    const base = projectPath.replace(/[\\/]+$/, '')
    return abs.startsWith(base + '/') ? abs.slice(base.length + 1) : abs
  }

  useEffect(() => {
    void load()
  }, [load])

  // Agent 任务结束（refreshTick 变化）后自动重新分析
  useEffect(() => {
    if (refreshTick === 0) return
    void load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshTick])

  // 进入「依赖」子 tab 时加载版本核对
  useEffect(() => {
    if (sub !== 'deps' || depChecks !== null) return
    setDepsLoading(true)
    setDepsMsg(null)
    checkOhpmDeps(effPath)
      .then(setDepChecks)
      .catch((e) => setDepsMsg(e instanceof Error ? e.message : String(e)))
      .finally(() => setDepsLoading(false))
  }, [sub, depChecks, effPath])

  const handleInstall = async () => {
    setInstalling(true)
    setDepsMsg(null)
    try {
      const log = await runOhpmInstall(effPath)
      setDepsMsg(`${t('home.analyzeInstalled')}\n${log}`)
      // 安装后重新核对版本
      const checks = await checkOhpmDeps(effPath)
      setDepChecks(checks)
    } catch (e) {
      setDepsMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setInstalling(false)
    }
  }

  const errCount = data?.build_errors.length ?? 0

  // 修复闭环反馈：Agent 修复前记录的错误数 > 0，刷新后错误清零 → 显示修复成功并沉淀工程记忆
  const memorySavedRef = useRef(false)
  const fixedCatsRef = useRef<string>('')
  useEffect(() => {
    if (!data) return
    if (fixedFrom > 0 && errCount === 0 && !memorySavedRef.current) {
      memorySavedRef.current = true
      // 自动淡出成功提示（8s 后清除基准）
      const t = window.setTimeout(() => setFixedFrom(0), 8000)
      // 沉淀一条 pitfall 记忆：记录该工程曾出现并修复构建错误，供后续对话参考
      const cats = fixedCatsRef.current || '其他'
      void useProjectStore.getState().saveMemory({
        category: 'pitfall',
        title: `构建错误自动修复记录（${fixedFrom} 处）`,
        content: `该工程最近一次构建出现 ${fixedFrom} 处错误（分类：${cats}），经 Agent 自动修复后构建已通过。如再次遇到同类问题，可参考本次修复思路：依赖类先 ohpm install；SDK 类核对 compatibleSdkVersion；类型/语法类阅读错误行号上下文修正；签名类需在 DevEco 配置。`,
      }).catch(() => {})
      return () => window.clearTimeout(t)
    }
    if (fixedFrom === 0) memorySavedRef.current = false
  }, [data, fixedFrom, errCount, projectId])

  const errColor = (kind: string) => {
    switch (kind) {
      case 'ERROR':
      case 'ArkTS:ERROR':
        return 'bg-[var(--danger)]/10 text-[var(--danger)]'
      case 'WARNING':
      case 'warning':
        return 'bg-amber-500/10 text-amber-500'
      default:
        return 'bg-[var(--accent-soft)] text-[var(--accent)]'
    }
  }

  const subTabs: { key: typeof sub; label: string; badge?: number; icon: IconName }[] = [
    { key: 'errors', label: t('home.analyzeErrors'), badge: errCount, icon: 'close' },
    { key: 'cap', label: t('home.analyzeCap'), icon: 'spark' },
    { key: 'deps', label: t('home.analyzeDeps'), icon: 'package' },
  ]

  return (
    <div className="p-3 space-y-2.5">
      {/* 头部：项目名 + 刷新/构建 */}
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium truncate">{projectName}</div>
          <div className="text-[10.5px] text-[var(--text-muted)] truncate font-mono" title={projectPath}>
            {projectPath}
          </div>
        </div>
        <button
          type="button"
          onClick={() => void load()}
          title={t('home.analyzeRefresh')}
          className="p-1.5 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
        >
          <Icon name="refresh" size={13} className={loading ? 'animate-spin' : ''} />
        </button>
        {!genericOverview && (
          <button
            type="button"
            onClick={onRunBuild}
            title={t('home.analyzeRunBuild')}
            className="h-6 px-2 rounded-md text-[10px] font-medium bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80 transition-opacity flex items-center gap-1"
          >
            <Icon name="bolt" size={10} />
            {t('home.analyzeRunBuild')}
          </button>
        )}
      </div>

      {/* 鸿蒙主工程选择器：混合工作区（项目根非鸿蒙工程）时切换实际分析的子工程 */}
      {rootInfo &&
        (rootInfo.candidates.length > 0 || rootInfo.configured != null || rootInfo.auto) && (
          <div className="flex items-center gap-1.5 rounded-lg modern-card px-2 py-1.5">
            <span className="shrink-0 text-[10px] text-[var(--text-muted)]">
              {t('home.analyzeHarmonyRoot')}
            </span>
            <select
              value={rootInfo.root}
              disabled={rootLoading}
              onChange={(e) => void handleRootChange(e.target.value)}
              className="h-6 flex-1 min-w-0 rounded-md border border-[var(--border)] bg-[var(--bg-window)] px-1.5 text-[10.5px] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
            >
              <option value={projectPath}>{projectName}</option>
              {rootInfo.candidates.map((c) => (
                <option key={c} value={c}>
                  {rootLabel(c)}
                </option>
              ))}
            </select>
            {rootInfo.auto && !rootInfo.configured && (
              <span className="shrink-0 rounded bg-[var(--accent-soft)] px-1 py-0.5 text-[9px] text-[var(--accent)]">
                {t('home.analyzeRootAuto')}
              </span>
            )}
          </div>
        )}

      {/* 子 tab：分段控件（构建错误 / 能力 / 依赖）；非鸿蒙工程（通用概览）时隐藏 */}
      {!genericOverview && (
        <div className="flex gap-0.5 p-0.5 rounded-lg bg-[var(--bg-hover)]/50 border border-[var(--border)]">
        {subTabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            onClick={() => setSub(tb.key)}
            title={tb.badge ? `${tb.label} (${tb.badge})` : tb.label}
            className={`flex items-center justify-center gap-1 flex-1 h-6 rounded-[6px] text-[10.5px] transition-all border ${
              sub === tb.key
                ? 'tab-soft font-medium border-[var(--border)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] border-transparent'
            }`}
          >
            <Icon name={tb.icon} size={11} className={sub === tb.key ? '' : 'opacity-60'} />
            <span className="whitespace-nowrap">{tb.label}</span>
            {typeof tb.badge === 'number' && tb.badge > 0 && (
              <span
                className={`px-1 rounded text-[9px] font-mono leading-[14px] ${
                  tb.key === 'errors' ? 'bg-[var(--danger)]/15 text-[var(--danger)]' : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
                }`}
              >
                {tb.badge}
              </span>
            )}
          </button>
        ))}
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-[var(--danger)]/30 bg-[var(--danger)]/5 px-2.5 py-2 text-[10.5px] text-[var(--danger)]">
          {error}
        </div>
      )}

      {loading && !data && !genericOverview ? (
        <div className="space-y-2">
          <div className="shimmer h-10 rounded-lg bg-[var(--bg-hover)]" />
          <div className="shimmer h-24 rounded-lg bg-[var(--bg-hover)]" />
          <div className="shimmer h-16 rounded-lg bg-[var(--bg-hover)]" />
        </div>
      ) : (
        <>
          {/* 通用工程概览：非鸿蒙子工程（Node/Go/Rust/Python/Java/C/C++/Flutter/.NET 等） */}
          {genericOverview && !data && !error && (
            <div className="rounded-lg modern-card p-2.5">
              <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-[var(--text-secondary)] leading-relaxed">
                {genericOverview}
              </pre>
            </div>
          )}
          {data && (
          <>
            {/* ============ 子 tab：构建错误 ============ */}
            {sub === 'errors' && (
              <div className="space-y-1.5">
                {agentBusy && (
                  <div className="rounded-lg border border-[var(--accent)]/30 bg-[var(--accent-soft)]/40 px-2.5 py-2 text-[10.5px] text-[var(--accent)] flex items-center gap-1.5">
                    <Icon name="refresh" size={11} className="animate-spin" />
                    {t('home.analyzeAgentBusy')}
                  </div>
                )}
                {!agentBusy && fixedFrom > 0 && errCount === 0 && (
                  <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-2 text-[10.5px] text-emerald-500 flex items-center gap-1.5">
                    <Icon name="check" size={11} />
                    {t('home.analyzeFixSuccess', { n: fixedFrom })}
                  </div>
                )}
                {errCount === 0 ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[11px] text-[var(--text-muted)]">
                    {t('home.analyzeNoErrors')}
                  </div>
                ) : (
                  <>
                    {data.build_errors.map((e, i) => (
                      <div key={i} className="rounded-lg modern-card p-2.5 space-y-1">
                        <div className="flex items-center gap-1.5 flex-wrap">
                          <span className={`px-1.5 py-0.5 rounded text-[9px] font-mono font-medium ${errColor(e.kind)}`}>
                            {e.kind}
                          </span>
                          <span className="px-1.5 py-0.5 rounded text-[9px] bg-[var(--bg-hover)] text-[var(--text-muted)]">
                            {t(`home.cat_${e.category}`, { defaultValue: e.category })}
                          </span>
                          {e.file && (
                            <span className="font-mono text-[10px] text-[var(--text-secondary)] truncate flex-1 min-w-0" title={e.file}>
                              {e.file}
                              {e.line ? `:${e.line}` : ''}
                              {e.column ? `:${e.column}` : ''}
                            </span>
                          )}
                        </div>
                        <div className="text-[10.5px] text-[var(--text-secondary)] break-all">{e.message}</div>
                        {e.suggestion && (
                          <div className="text-[10px] text-[var(--text-muted)] flex gap-1">
                            <span className="shrink-0 text-[var(--accent)]">{t('home.analyzeSuggestion')}</span>
                            <span>{e.suggestion}</span>
                          </div>
                        )}
                      </div>
                    ))}
                    <div className="flex gap-1.5">
                      <button
                        type="button"
                        onClick={() => {
                          const catSet = new Set(data.build_errors.map((e) => e.category))
                          fixedCatsRef.current = [...catSet].join('/')
                          memorySavedRef.current = false
                          triggerFixSnapshot(data.build_errors.length)
                          void onAutoFix(data.build_errors)
                        }}
                        disabled={fixing || agentBusy}
                        title={t('home.analyzeAutoFix')}
                        className="flex-1 h-7 rounded-lg text-[10.5px] font-medium bg-[var(--danger)]/10 text-[var(--danger)] hover:bg-[var(--danger)]/20 transition-colors disabled:opacity-50"
                      >
                        {t('home.analyzeAutoFix')}
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setFixing(true)
                          try {
                            onFixErrors(data.build_errors)
                          } finally {
                            setFixing(false)
                          }
                        }}
                        disabled={fixing}
                        title={t('home.analyzeFixByAgent')}
                        className="h-7 px-2.5 rounded-lg text-[10.5px] text-[var(--text-muted)] border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors disabled:opacity-50"
                      >
                        {t('home.analyzeFixByAgent')}
                      </button>
                    </div>
                  </>
                )}
              </div>
            )}

            {/* ============ 子 tab：工程能力 ============ */}
            {sub === 'cap' && (
              <div className="space-y-2.5">
                {/* 工程摘要：网格统计卡 */}
                <div className="rounded-lg modern-card p-2">
                  <div className="grid grid-cols-2 gap-1.5">
                    {[
                      { label: t('home.analyzeBundle'), value: data.project.bundle_name ?? '--', mono: true, full: true },
                      { label: t('home.analyzeSdk'), value: data.project.sdk_version ?? (data.project.api_version != null ? `API ${data.project.api_version}` : '--'), mono: true, full: false },
                      { label: t('home.analyzeApi'), value: data.project.api_version != null ? `API ${data.project.api_version}` : '--', mono: true, full: false },
                      { label: t('home.analyzeModules'), value: `${data.modules.length}`, mono: true, full: false },
                      { label: t('home.analyzeKits'), value: `${data.kit_usage.length}`, mono: true, full: false },
                      { label: t('home.analyzePermissions'), value: `${data.permissions.length}`, mono: true, full: false },
                    ].map((it) => (
                      <div
                        key={it.label}
                        className={`rounded-md bg-[var(--bg-hover)]/50 border border-[var(--border)]/60 px-2 py-1.5 min-w-0 ${it.full ? 'col-span-2' : ''}`}
                      >
                        <div className="text-[9px] text-[var(--text-muted)] leading-none mb-1">{it.label}</div>
                        <div className={`text-[11px] text-[var(--text-secondary)] font-medium truncate ${it.mono ? 'font-mono' : ''}`} title={typeof it.value === 'string' ? it.value : undefined}>
                          {it.value}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                {/* 模块列表 */}
                {data.modules.length > 0 && (
                  <div className="rounded-lg modern-card p-2.5 space-y-2">
                    <div className="flex items-center gap-1.5">
                      <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.analyzeModuleList')}</span>
                      <span className="px-1.5 py-0.5 rounded-full text-[9px] font-medium bg-[var(--accent)]/10 text-[var(--accent)]">
                        {data.modules.length}
                      </span>
                    </div>
                    {data.modules.map((m) => (
                      <div key={m.rel_path} className="space-y-0.5 rounded-md bg-[var(--bg-hover)]/40 border border-[var(--border)]/50 px-2 py-1.5">
                        <div className="flex items-center gap-1.5">
                          <Icon name="package" size={10} className="text-[var(--accent)]/70 shrink-0" />
                          <span className="font-mono text-[10.5px] text-[var(--text-secondary)] truncate flex-1" title={m.rel_path}>
                            {m.rel_path}
                          </span>
                          <span className="px-1 py-0.5 rounded text-[9px] bg-[var(--bg-hover)] text-[var(--text-muted)] font-mono">
                            {m.kind || 'module'}
                          </span>
                        </div>
                        {m.device_types.length > 0 && (
                          <div className="pl-4 flex flex-wrap gap-1">
                            {m.device_types.map((dt) => (
                              <span key={dt} className="px-1 py-0.5 rounded text-[9px] bg-[var(--accent-soft)] text-[var(--accent)]">
                                {dt}
                              </span>
                            ))}
                          </div>
                        )}
                        {m.kits.length > 0 && (
                          <div className="pl-4 flex flex-wrap gap-1">
                            {m.kits.map((k) => (
                              <span key={k} className="font-mono text-[9px] px-1 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                                {k}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                )}

                {/* Kit 使用 Top：带占比进度条 */}
                {data.kit_usage.length > 0 && (() => {
                  const maxCount = data.kit_usage[0]?.count ?? 1
                  return (
                    <div className="rounded-lg modern-card p-2.5">
                      <div className="text-[11px] font-medium text-[var(--text-secondary)] mb-1.5">{t('home.analyzeKitUsage')}</div>
                      <div className="space-y-1.5">
                        {data.kit_usage.slice(0, 12).map((k) => (
                          <div key={k.kit}>
                            <div className="flex items-center gap-2">
                              <span className="font-mono text-[10px] text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={k.kit}>
                                {k.kit}
                              </span>
                              <span className="text-[9.5px] font-mono text-[var(--text-muted)] w-8 text-right">{k.count}</span>
                            </div>
                            <div className="h-[3px] mt-0.5 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                              <div
                                className="h-full rounded-full bg-gradient-to-r from-[var(--accent)]/70 to-[var(--accent)]/30 transition-all"
                                style={{ width: `${Math.max(6, (k.count / maxCount) * 100)}%` }}
                              />
                            </div>
                          </div>
                        ))}
                      </div>
                    </div>
                  )
                })()}

                {/* 权限 */}
                {data.permissions.length > 0 && (
                  <div className="rounded-lg modern-card p-2.5">
                    <div className="text-[11px] font-medium text-[var(--text-secondary)] mb-1.5">{t('home.analyzePermissions')}</div>
                    <div className="space-y-1">
                      {data.permissions.map((p) => (
                        <div key={p.name} className="flex items-center gap-1.5">
                          <span className="font-mono text-[10px] text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={p.name}>
                            {p.name}
                          </span>
                          {p.reason && (
                            <span className="text-[9px] text-[var(--text-muted)] truncate max-w-[110px]" title={p.reason}>
                              {p.reason}
                            </span>
                          )}
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            )}

            {/* ============ 子 tab：ohpm 依赖 ============ */}
            {sub === 'deps' && (
              <div className="space-y-1.5">
                {/* 版本核对 + 安装操作 */}
                <div className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={() => void handleInstall()}
                    disabled={installing}
                    className="h-6 px-2 rounded-md text-[10px] font-medium bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80 transition-opacity flex items-center gap-1 disabled:opacity-50"
                  >
                    <Icon name="download" size={10} className={installing ? 'animate-spin' : ''} />
                    {installing ? t('home.analyzeInstalling') : t('home.analyzeInstall')}
                  </button>
                  {depsLoading && (
                    <span className="text-[10px] text-[var(--text-muted)]">{t('home.analyzeCheckingDeps')}</span>
                  )}
                </div>
                {depsMsg && (
                  <div className="rounded-lg modern-card px-2.5 py-2 text-[10px] font-mono text-[var(--text-secondary)] whitespace-pre-wrap break-all max-h-40 overflow-y-auto">
                    {depsMsg}
                  </div>
                )}
                {depChecks === null && !depsLoading ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[11px] text-[var(--text-muted)]">
                    {t('home.analyzeNoDeps')}
                  </div>
                ) : depChecks && depChecks.length === 0 ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[11px] text-[var(--text-muted)]">
                    {t('home.analyzeNoDeps')}
                  </div>
                ) : (
                  depChecks && (
                    (() => {
                      // 按模块分组：根依赖（module=""）在前
                      const groups = new Map<string, OhpmDepCheck[]>()
                      for (const d of depChecks) {
                        const key = d.module || '(root)'
                        if (!groups.has(key)) groups.set(key, [])
                        groups.get(key)!.push(d)
                      }
                      return [...groups.entries()].map(([mod, deps]) => (
                        <div key={mod} className="rounded-lg modern-card p-2.5">
                          <div className="flex items-center gap-1.5 mb-1">
                            <Icon name="package" size={10} className="text-[var(--accent)] shrink-0" />
                            <span className="font-mono text-[10.5px] text-[var(--text-secondary)] truncate flex-1">{mod}</span>
                            <span className="text-[9px] text-[var(--text-muted)]">{deps.length}</span>
                          </div>
                          <div className="space-y-1">
                            {deps.map((d) => {
                              const missing = !d.installed
                              const outdated = d.installed && d.declared && !d.declared.includes(d.installed)
                              return (
                                <div key={`${d.name}-${d.declared}`} className="text-[10px]">
                                  <div className="flex items-center gap-1.5">
                                    <span className="font-mono text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={d.name}>
                                      {d.name}
                                    </span>
                                    <span className="font-mono text-[var(--text-muted)]">{d.declared}</span>
                                    {d.dev && (
                                      <span className="px-1 rounded text-[8.5px] bg-[var(--bg-hover)] text-[var(--text-muted)]">
                                        {t('home.analyzeDevDep')}
                                      </span>
                                    )}
                                  </div>
                                  {missing ? (
                                    <div className="pl-3 text-[9px] text-[var(--danger)]">
                                      {t('home.analyzeNotInstalled')}
                                    </div>
                                  ) : (
                                    outdated && (
                                      <div className="pl-3 text-[9px] text-amber-500">
                                        {t('home.analyzeInstalledVer')} {d.installed}
                                      </div>
                                    )
                                  )}
                                </div>
                              )
                            })}
                          </div>
                        </div>
                      ))
                    })()
                  )
                )}
              </div>
            )}
          </>
          )}
        </>
      )}
    </div>
  )
}

/** 性能指标卡：当前值 + SVG 迷你趋势曲线（数据点 < 2 时只显示数值） */
export function PerfCard({
  label,
  value,
  unit,
  color,
  data,
}: {
  label: string
  value: number
  unit: string
  color: string
  data: number[]
}) {
  const valid = data.filter((v) => v >= 0)
  const show = value >= 0
  return (
    <div className="rounded-md modern-card border-[var(--border)] px-2 py-1.5">
      <div className="flex items-baseline justify-between gap-1">
        <span className="text-[9.5px] text-[var(--text-muted)]">{label}</span>
        <span className="text-[11px] font-mono font-medium" style={{ color: show ? color : 'var(--text-muted)' }}>
          {show ? `${Math.round(value * 10) / 10}${unit}` : '--'}
        </span>
      </div>
      {valid.length >= 2 ? (
        <svg viewBox="0 0 100 26" preserveAspectRatio="none" className="mt-1 h-6 w-full">
          <polyline
            fill="none"
            stroke={color}
            strokeWidth="1.5"
            strokeLinejoin="round"
            strokeLinecap="round"
            points={valid
              .map((v, i) => {
                const x = (i / Math.max(valid.length - 1, 1)) * 100
                const y = 25 - (Math.max(0, Math.min(v, 100)) / 100) * 24
                return `${x.toFixed(1)},${y.toFixed(1)}`
              })
              .join(' ')}
          />
        </svg>
      ) : (
        <div className="mt-1 h-6" />
      )}
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

const SYMBOL_KINDS: { value: string; labelKey: string }[] = [
  { value: '', labelKey: 'home.symbolKindAll' },
  { value: 'component', labelKey: 'home.symbolKindComponent' },
  { value: 'function', labelKey: 'home.symbolKindFunction' },
  { value: 'method', labelKey: 'home.symbolKindMethod' },
  { value: 'class', labelKey: 'home.symbolKindClass' },
  { value: 'interface', labelKey: 'home.symbolKindInterface' },
  { value: 'route', labelKey: 'home.symbolKindRoute' },
  { value: 'struct', labelKey: 'home.symbolKindStruct' },
  { value: 'enum', labelKey: 'home.symbolKindEnum' },
]

function symbolKindColor(kind: string): string {
  switch (kind) {
    case 'component':
      return 'var(--accent)'
    case 'function':
    case 'method':
      return '#10b981'
    case 'class':
    case 'struct':
      return '#f59e0b'
    case 'interface':
      return '#8b5cf6'
    case 'route':
      return '#ec4899'
    case 'enum':
      return '#06b6d4'
    default:
      return 'var(--text-muted)'
  }
}

/* ============ 符号检索面板：按名称检索组件/函数/类等，点击内联查看并定位行号 ============ */
export function SymbolsPanel({
  projectId,
  projectName,
  onReference,
  root,
}: {
  projectId: string
  projectName?: string
  onReference: (path: string) => void
  /** 会话工作目录（worktree 模式为 worktree 路径，本地模式为 undefined） */
  root?: string
}) {
  const { t } = useTranslation()
  const [query, setQuery] = useState('')
  const [kind, setKind] = useState('')
  const [scope, setScope] = useState<'project' | 'all'>('project')
  const [symbols, setSymbols] = useState<CodeSymbol[]>([])
  const [loading, setLoading] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [searching, setSearching] = useState(false)
  const [meta, setMeta] = useState<SymbolIndexMeta | null>(null)
  const [error, setError] = useState<string | null>(null)
  // 内联代码查看：当前打开的符号/文件
  const [viewer, setViewer] = useState<{ file: string; line: number; content: string } | null>(null)
  const [viewerLoading, setViewerLoading] = useState(false)
  const viewerRef = useRef<HTMLDivElement>(null)
  // Outline 分组视图：按第一层文件夹聚合，两级（目录/文件）均可折叠
  // （默认全部折叠，点击展开；有搜索词时自动展开匹配项）
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(() => new Set())
  const [collapsedFiles, setCollapsedFiles] = useState<Set<string>>(() => new Set())
  const didInitCollapse = useRef(false)

  const dirs = useMemo(() => {
    type FileGroup = { key: string; file: string; syms: CodeSymbol[] }
    type DirGroup = { key: string; dir: string; project?: string; files: FileGroup[] }
    const map = new Map<string, DirGroup>()
    for (const s of symbols) {
      const cs = s as CrossProjectSymbol
      const project = scope === 'all' && cs.project_name ? cs.project_name : undefined
      // 第一层文件夹：相对路径的首段；项目根下的零散文件归入“（项目根）”
      const top = s.file.split('/')[0] || t('home.symbolRootDir')
      const dirKey = project ? `${project}::${top}` : top
      let d = map.get(dirKey)
      if (!d) {
        d = { key: dirKey, dir: top, project, files: [] }
        map.set(dirKey, d)
      }
      const fKey = `${dirKey}::${s.file}`
      const f = d.files.find((x) => x.key === fKey)
      if (f) f.syms.push(s)
      else d.files.push({ key: fKey, file: s.file, syms: [s] })
    }
    const list = Array.from(map.values()).sort((a, b) => a.key.localeCompare(b.key))
    for (const d of list) d.files.sort((a, b) => a.file.localeCompare(b.file))
    return list
  }, [symbols, scope, t])

  // 默认全部折叠：首次加载完成（无搜索词）时把所有目录/文件 key 加入折叠集合；
  // 用户输入搜索词时自动展开全部，便于浏览匹配结果。
  useEffect(() => {
    if (query.trim() || kind) {
      setCollapsedDirs(new Set())
      setCollapsedFiles(new Set())
      return
    }
    if (dirs.length > 0 && !didInitCollapse.current) {
      didInitCollapse.current = true
      setCollapsedDirs(new Set(dirs.map((d) => d.key)))
      setCollapsedFiles(new Set(dirs.flatMap((d) => d.files.map((f) => f.key))))
    }
  }, [dirs, query, kind])

  const toggleDir = (d: string) => {
    setCollapsedDirs((prev) => {
      const next = new Set(prev)
      if (next.has(d)) next.delete(d)
      else next.add(d)
      return next
    })
  }

  const toggleFile = (f: string) => {
    setCollapsedFiles((prev) => {
      const next = new Set(prev)
      if (next.has(f)) next.delete(f)
      else next.add(f)
      return next
    })
  }

  const handleRefresh = async () => {
    if (scope !== 'project' || refreshing) return
    setRefreshing(true)
    setError(null)
    try {
      const list = await refreshProjectSymbols(projectId, root)
      setSymbols(list)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setRefreshing(false)
    }
  }

  // 索引元信息：加载/刷新完成后拉取一次，展示符号/文件数量与缓存来源
  useEffect(() => {
    if (scope !== 'project' || loading || refreshing) return
    let cancelled = false
    symbolIndexMeta(projectId, root)
      .then((m) => {
        if (!cancelled) setMeta(m)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectId, root, scope, loading, refreshing])

  // 首次进入：构建当前项目索引（全量扫描；「全部项目」范围不预加载，等用户输入检索）
  useEffect(() => {
    if (scope !== 'project') return
    let cancelled = false
    setLoading(true)
    setError(null)
    indexProjectSymbols(projectId, root)
      .then((list) => {
        if (!cancelled) setSymbols(list)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [projectId, root, scope])

  // 输入防抖检索（当前项目或全部项目）
  useEffect(() => {
    const handle = setTimeout(() => {
      let cancelled = false
      if (!query.trim() && !kind) {
        if (scope === 'all') setSymbols([])
        return
      }
      const p =
        scope === 'all'
          ? searchSymbolsAll(query.trim(), kind || undefined)
          : searchSymbolsApi(projectId, query.trim(), kind || undefined, root)
      if (!cancelled) setSearching(true)
      p.then((list) => {
        if (!cancelled) setSymbols(list)
      })
        .catch(() => {})
        .finally(() => {
          if (!cancelled) setSearching(false)
        })
      return () => {
        cancelled = true
      }
    }, 220)
    return () => clearTimeout(handle)
  }, [query, kind, projectId, root, scope])

  const openSymbol = async (sym: CodeSymbol) => {
    setViewer({ file: sym.file, line: sym.line, content: '' })
    setViewerLoading(true)
    try {
      const res = await readProjectFile(projectId, sym.file, root)
      setViewer({ file: sym.file, line: sym.line, content: res.content })
      // 等行渲染后滚动到目标行
      requestAnimationFrame(() => {
        viewerRef.current
          ?.querySelector(`[data-line="${sym.line}"]`)
          ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
      })
    } catch (e) {
      // catch 变量在非 strict 配置下为 {}，instanceof 收窄无效；用断言安全提取 message
      const msg = String((e as Error | null)?.message ?? e)
      setViewer({ file: sym.file, line: sym.line, content: `// ${msg}` })
    } finally {
      setViewerLoading(false)
    }
  }

  const lines = viewer ? viewer.content.split('\n') : []
  const startLine = Math.max(1, viewer ? viewer.line - 8 : 1)

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* 搜索栏 */}
      <div className="p-2.5 border-b border-[var(--border)] shrink-0 space-y-2">
        <div className="relative">
          <span className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]">
            <Icon name="search" size={13} />
          </span>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('home.symbolSearch')}
            spellCheck={false}
            className="w-full h-8 pl-8 pr-7 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--text-muted)] hover:text-[var(--text-secondary)]"
            >
              <Icon name="close" size={12} />
            </button>
          )}
        </div>
        {scope === 'project' && (
          <button
            onClick={handleRefresh}
            disabled={refreshing}
            className="w-full h-7 flex items-center justify-center gap-1.5 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[11px] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)] transition-colors disabled:opacity-50"
          >
            <Icon name="refresh" size={12} className={refreshing ? 'animate-spin' : ''} />
            {t('home.symbolRefresh')}
          </button>
        )}
        {/* 范围切换：当前项目 / 全部项目 */}
        <div className="flex gap-1.5">
          <button
            onClick={() => setScope('project')}
            className={`flex-1 h-8 rounded-lg text-[11px] border transition-colors ${
              scope === 'project'
                ? 'border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                : 'border-[var(--border)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-strong)]'
            }`}
          >
            {projectName ? t('home.symbolScopeProject', { name: projectName }) : t('home.symbolScopeProjectShort')}
          </button>
          <button
            onClick={() => setScope('all')}
            className={`flex-1 h-8 rounded-lg text-[11px] border transition-colors ${
              scope === 'all'
                ? 'border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                : 'border-[var(--border)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-strong)]'
            }`}
          >
            {t('home.symbolScopeAll')}
          </button>
        </div>
        <select
          value={kind}
          onChange={(e) => setKind(e.target.value)}
          className="w-full h-8 px-2 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[12px] text-[var(--text-secondary)] outline-none focus:border-[var(--accent)] transition-colors"
        >
          {SYMBOL_KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {t(k.labelKey)}
            </option>
          ))}
        </select>
      </div>

      {/* 内联代码查看器 */}
      {viewer && (
        <div className="border-b border-[var(--border)] shrink-0 max-h-[55%] flex flex-col bg-[var(--bg-primary)]">
          <div className="flex items-center gap-2 px-2.5 py-1.5 border-b border-[var(--border)] bg-[var(--bg-card)]">
            <Icon name="file" size={12} className="text-[var(--accent)]" />
            <span className="text-[11px] font-mono truncate flex-1 text-[var(--text-secondary)]">
              {viewer.file}:{viewer.line}
            </span>
            <button
              onClick={() => onReference(viewer.file)}
              title={t('home.symbolReference')}
              className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]"
            >
              <Icon name="chat" size={12} />
            </button>
            <button
              onClick={() => setViewer(null)}
              className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]"
            >
              <Icon name="close" size={12} />
            </button>
          </div>
          <div ref={viewerRef} className="flex-1 min-h-0 overflow-auto font-mono text-[11px] leading-[1.65] py-1">
            {viewerLoading ? (
              <div className="shimmer h-32 mx-2 my-1 rounded bg-[var(--bg-hover)]" />
            ) : (
              lines
                .map((l, i) => ({ no: i + 1, text: l }))
                .filter((row) => row.no >= startLine && row.no <= startLine + 30)
                .map((row) => (
                  <div
                    key={row.no}
                    data-line={row.no}
                    className={`flex gap-3 px-2.5 ${
                      row.no === viewer.line
                        ? 'bg-[var(--accent-soft)]'
                        : row.no % 2 === 0
                          ? 'bg-transparent'
                          : 'bg-transparent'
                    }`}
                  >
                    <span
                      className={`select-none text-right w-7 shrink-0 ${
                        row.no === viewer.line ? 'text-[var(--accent)] font-semibold' : 'text-[var(--text-muted)]/50'
                      }`}
                    >
                      {row.no}
                    </span>
                    <pre className="whitespace-pre-wrap break-all text-[var(--text-secondary)] m-0">{row.text}</pre>
                  </div>
                ))
            )}
          </div>
        </div>
      )}

      {/* 结果列表（按文件分组 Outline 视图） */}
      <div className="flex-1 min-h-0 overflow-y-auto p-1.5">
        {loading ? (
          <div className="p-3 space-y-1.5">
            <div className="flex items-center gap-1.5 text-[11px] text-[var(--text-muted)] pb-1">
              <Icon name="refresh" size={11} className="animate-spin" />
              {t('home.symbolScanning')}
            </div>
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="shimmer h-9 rounded-lg bg-[var(--bg-hover)]" />
            ))}
          </div>
        ) : error ? (
          <div className="p-3 text-[11px] text-[var(--danger)]">{error}</div>
        ) : symbols.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center gap-1.5 text-center px-4">
            <Icon name="search" size={22} className="opacity-30" />
            <span className="text-[11.5px] text-[var(--text-muted)]">{t('home.symbolEmpty')}</span>
          </div>
        ) : (
          <div className="space-y-1">
            {searching && (
              <div className="flex items-center gap-1.5 text-[10px] text-[var(--text-muted)] px-1 pt-0.5 pb-1">
                <Icon name="refresh" size={10} className="animate-spin" />
                {t('home.symbolSearching')}
              </div>
            )}
            {query.trim() || kind ? (
              <div className="text-[10px] text-[var(--text-muted)] px-1 pt-0.5 pb-1">
                {t('home.symbolResultCount', { count: symbols.length })}
              </div>
            ) : (
              <div className="text-[10px] text-[var(--text-muted)] px-1 pt-0.5 pb-1 flex items-center gap-1.5 flex-wrap">
                {t('home.symbolIndexCount', { count: symbols.length })}
                {meta && meta.files > 0 && (
                  <>
                    <span>·</span>
                    <span>{t('home.symbolFileCount', { count: meta.files })}</span>
                    {meta.source === 'disk' && (
                      <span className="px-1 py-px rounded bg-[var(--bg-hover)] text-[9px]">{t('home.symbolSourceDisk')}</span>
                    )}
                  </>
                )}
              </div>
            )}
            {dirs.slice(0, 120).map((d) => {
              const dirHidden = collapsedDirs.has(d.key)
              return (
                <div key={d.key} className="rounded-lg border border-[var(--border)] overflow-hidden bg-[var(--bg-card)]">
                  <button
                    onClick={() => toggleDir(d.key)}
                    className="w-full flex items-center gap-1.5 px-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors text-left"
                    title={d.dir}
                  >
                    <Icon name="chevron-right" size={11} className={`transition-transform shrink-0 ${dirHidden ? '' : 'rotate-90'}`} />
                    <Icon name="folder" size={12} className="text-[var(--accent)] shrink-0" />
                    {d.project && (
                      <span className="shrink-0 text-[9px] font-medium px-1.5 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                        {d.project}
                      </span>
                    )}
                    <span className="text-[11.5px] font-medium truncate flex-1 text-[var(--text-primary)]">{d.dir}</span>
                    <span className="text-[10px] text-[var(--text-muted)] shrink-0 tabular-nums">{d.files.length}</span>
                  </button>
                  {!dirHidden && (
                    <div className="border-t border-[var(--border)] bg-[var(--bg-primary)]/40">
                      {d.files.slice(0, 120).map((f) => {
                        const fileHidden = collapsedFiles.has(f.key)
                        return (
                          <div key={f.key}>
                            <button
                              onClick={() => toggleFile(f.key)}
                              className="w-full flex items-center gap-1.5 pl-6 pr-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors text-left"
                              title={f.file}
                            >
                              <Icon name="chevron-right" size={10} className={`transition-transform shrink-0 ${fileHidden ? '' : 'rotate-90'}`} />
                              <Icon name="file" size={12} className="text-[var(--accent)] shrink-0" />
                              <span className="text-[11px] font-mono truncate flex-1 text-[var(--text-secondary)]">{f.file}</span>
                              <span className="text-[10px] text-[var(--text-muted)] shrink-0 tabular-nums">{f.syms.length}</span>
                            </button>
                            {!fileHidden && (
                              <div className="border-t border-[var(--border)]/60">
                                {f.syms.slice(0, 60).map((s, i) => (
                                  <button
                                    key={`${s.file}:${s.line}:${i}`}
                                    onClick={() => openSymbol(s)}
                                    className="list-item w-full text-left pl-9 pr-2.5 py-1.5 flex items-start gap-2 group"
                                  >
                                    <span
                                      className="mt-0.5 shrink-0 text-[9px] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded"
                                      style={{
                                        color: symbolKindColor(s.kind),
                                        background: `color-mix(in srgb, ${symbolKindColor(s.kind)} 14%, transparent)`,
                                      }}
                                    >
                                      {s.kind}
                                    </span>
                                    <span className="min-w-0 flex-1">
                                      <span className="block text-[12px] font-medium text-[var(--text-primary)] truncate">
                                        {s.name}
                                        {s.parent && <span className="text-[var(--text-muted)] font-normal"> · {s.parent}</span>}
                                      </span>
                                      <span className="block text-[10px] text-[var(--text-muted)] font-mono truncate mt-0.5">
                                        L{s.line}
                                      </span>
                                    </span>
                                  </button>
                                ))}
                                {f.syms.length > 60 && (
                                  <div className="text-center text-[10px] text-[var(--text-muted)] py-1.5">
                                    {t('home.symbolMore', { count: f.syms.length - 60 })}
                                  </div>
                                )}
                              </div>
                            )}
                          </div>
                        )
                      })}
                      {d.files.length > 120 && (
                        <div className="text-center text-[10px] text-[var(--text-muted)] py-1.5">
                          {t('home.symbolMore', { count: d.files.length - 120 })}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )
            })}
            {dirs.length > 120 && (
              <div className="text-center text-[10.5px] text-[var(--text-muted)] py-2">
                {t('home.symbolTruncated', { count: dirs.length })}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

