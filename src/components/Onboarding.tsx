/**
 * 首启引导：环境体检 → 添加项目 → 配置 Provider
 *
 * ## 为什么需要它
 *
 * 此前新装用户落地在一个空三栏 IDE：中间是「开始新的会话」，右栏是
 * 「工作区模块 0 / 未识别到子工程模块」。没有项目、没有 Provider、没有体检结论，
 * 唯一的下一步是「添加项目」——而那需要用户已经知道 HarmonyOS 工程长什么样、
 * 自己装了哪些 SDK。第一步的成本全压在用户身上。
 *
 * ## 三步的顺序是按「阻塞程度」排的
 *
 * 1. **环境体检**：只读，一次 `tools_health`，把「缺什么、缺了会怎样」一次说清。
 *    放在最前是因为它**不需要用户做任何事**就能给出信息，且缺 SDK 时后面两步都会失败——
 *    早说比晚说省一轮返工。
 * 2. **添加项目**：真正的主路径。
 * 3. **配置 Provider**：没有模型就发不出第一条消息，但这一步要用户手上有 Key，
 *    因此放最后，且不阻塞完成。
 *
 * ## 刻意不做的事
 *
 * - **不阻塞**：每步都能跳过，「跳过」直接结束而不是把人卡在弹窗里。
 * - **不强制体检通过**：缺 SDK 也允许继续——本地读代码/改代码不依赖 SDK。
 * - **不自动探测 Provider Key**：那是越权，我们只报告「当前有 0 个」。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Modal } from './ui/Modal'
import { Button } from './ui/Button'
import { Spinner } from './ui/Spinner'
import Icon from '../icons/Icon'
import type { IconName } from '../icons/Icon'
import { toolsHealth, type ToolchainCheck } from '../api/health'
import { listProviders } from '../api/provider'
import { markFirstRunSeen } from '../utils/firstRun'
import { track } from '../utils/telemetry'
import { cn } from '../utils/cn'

const STEPS = ['env', 'project', 'provider'] as const
type Step = (typeof STEPS)[number]

export interface OnboardingProps {
  open: boolean
  /** 用户点「完成」或「跳过」后关闭。调用方负责收起自己的 state。 */
  onClose: () => void
  /** 「添加项目」：由调用方打开自己的 AddProjectDialog */
  onAddProject?: () => void
  /** 「去配置 Provider」：由调用方跳转到 Provider 页 */
  onGoProviders?: () => void
  /** 已有项目数 > 0 时第 2 步显示为已完成 */
  hasProjects?: boolean
}

export default function Onboarding({
  open,
  onClose,
  onAddProject,
  onGoProviders,
  hasProjects = false,
}: OnboardingProps) {
  const { t } = useTranslation()
  const [step, setStep] = useState<Step>('env')
  const [checks, setChecks] = useState<ToolchainCheck[] | null>(null)
  const [checksError, setChecksError] = useState<string | null>(null)
  const [providerCount, setProviderCount] = useState<number | null>(null)
  const [providersError, setProvidersError] = useState<string | null>(null)

  // 每次打开都从第 1 步重来：上次看到一半关掉的用户，重开应该从头看，
  // 而不是停在一个他已经忘了内容的中间步骤
  useEffect(() => {
    if (open) setStep('env')
  }, [open])

  const loadEnv = useCallback(async () => {
    setChecks(null)
    setChecksError(null)
    try {
      setChecks(await toolsHealth())
    } catch (e) {
      // 「体检跑不起来」与「体检发现全都缺失」是两件事，前者不能报成后者
      setChecksError(String(e))
    }
  }, [])

  const loadProviders = useCallback(async () => {
    setProviderCount(null)
    setProvidersError(null)
    try {
      setProviderCount((await listProviders()).length)
    } catch (e) {
      setProvidersError(String(e))
    }
  }, [])

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void (async () => {
      // 两路互不依赖：Provider 查不到也不该挡住环境体检的结果
      await Promise.all([
        (async () => {
          if (cancelled) return
          try {
            const r = await toolsHealth()
            if (!cancelled) setChecks(r)
          } catch (e) {
            if (!cancelled) setChecksError(String(e))
          }
        })(),
        (async () => {
          try {
            const r = await listProviders()
            if (!cancelled) setProviderCount(r.length)
          } catch (e) {
            if (!cancelled) setProvidersError(String(e))
          }
        })(),
      ])
    })()
    return () => {
      cancelled = true
    }
  }, [open])

  const stepIndex = STEPS.indexOf(step)

  const finish = () => {
    markFirstRunSeen()
    // 「完成」与「跳过」共用这一个出口（末步只有「完成」按钮，中间步只有
    // 「跳过 / 下一步」），因此用**停在第几步**来区分，而不是给每个按钮加回调——
    // 加回调会漏：Modal 的 X 与 Esc 也走这里。
    // 漏记一个出口的后果是统计里凭空多出一类「退出方式」，看不出是哪种交互。
    track(stepIndex === STEPS.length - 1 ? 'onboarding_finished' : 'onboarding_skipped', {
      step,
    })
    onClose()
  }

  // 每一步的**到达**（不是「看了几次」）。按会话去重而不是只挡连续重复：
  // 用户 env→project→env→project 来回翻，若每次曝光都记，面板上的
  // 「引导到达第 3 步」人数会随翻页次数放大，而这三个事件本来是
  // 能量化引导效果的漏斗——分母被自己的回退行为污染，比例就没意义了。
  // 用 ref 存已到达集合：StrictMode 只重跑 effect、保留 ref，state 反而要多渲染一次。
  const reachedSteps = useRef<Set<Step> | null>(null)
  useEffect(() => {
    reachedSteps.current ??= new Set<Step>()
    if (!open || reachedSteps.current.has(step)) return
    reachedSteps.current.add(step)
    track('onboarding_step_viewed', { step })
  }, [open, step])

  return (
    <Modal
      open={open}
      onClose={finish}
      size="2xl"
      icon="spark"
      header="formal"
      maxHeight="80vh"
      title={t('onboarding.title')}
      footer={
        <div className="flex w-full items-center justify-between gap-3">
          <StepDots current={stepIndex} />
          <div className="flex items-center gap-2">
            <Button variant="ghost" onClick={finish}>
              {t('onboarding.skip')}
            </Button>
            {stepIndex > 0 && (
              <Button variant="secondary" onClick={() => setStep(STEPS[stepIndex - 1])}>
                {t('onboarding.back')}
              </Button>
            )}
            {stepIndex < STEPS.length - 1 ? (
              <Button variant="primary" onClick={() => setStep(STEPS[stepIndex + 1])}>
                {t('onboarding.next')}
              </Button>
            ) : (
              <Button variant="primary" onClick={finish}>
                {t('onboarding.done')}
              </Button>
            )}
          </div>
        </div>
      }
    >
      <div className="space-y-4">
        {step === 'env' && (
          <StepEnv
            checks={checks}
            error={checksError}
            onRetry={() => void loadEnv()}
            onGoSettings={onClose}
          />
        )}
        {step === 'project' && (
          <StepProject hasProjects={hasProjects} onAddProject={onAddProject} onSkip={finish} />
        )}
        {step === 'provider' && (
          <StepProvider
            count={providerCount}
            error={providersError}
            onRetry={() => void loadProviders()}
            onGoProviders={onGoProviders}
            onSkip={finish}
          />
        )}
      </div>
    </Modal>
  )
}

function StepDots({ current }: { current: number }) {
  const { t } = useTranslation()
  return (
    <div className="flex items-center gap-1.5" role="group" aria-label={t('onboarding.progress')}>
      {STEPS.map((s, i) => (
        <span
          key={s}
          aria-current={i === current ? 'step' : undefined}
          title={t(`onboarding.step.${s}`)}
          className={cn(
            'h-1.5 rounded-full transition-all',
            i === current
              ? 'w-5 bg-[var(--accent)]'
              : i < current
                ? 'w-1.5 bg-[var(--accent-600)]'
                : 'w-1.5 bg-[var(--border-strong)]',
          )}
        />
      ))}
    </div>
  )
}

/** 一条体检结论。found=false 时把 suggestion 一并摆出来——只说「缺失」用户无从下手。 */
function CheckRow({ c }: { c: ToolchainCheck }) {
  const { t } = useTranslation()
  return (
    <div className="flex items-start gap-2.5 py-1.5">
      <span
        aria-hidden="true"
        className={cn(
          'mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full',
          c.found ? 'bg-[var(--success-50)]' : 'bg-[var(--warning-50)]',
        )}
      >
        <Icon
          name={c.found ? 'check' : 'info'}
          size={11}
          className={c.found ? 'text-[var(--success)]' : 'text-[var(--warning-600)]'}
        />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-[length:var(--app-text-sm)] font-medium">{c.name}</span>
          <span
            className={cn(
              'text-[length:var(--app-text-2xs)]',
              c.found ? 'text-[var(--success)]' : 'text-[var(--warning-600)]',
            )}
          >
            {c.found ? t('onboarding.found') : t('onboarding.missing')}
          </span>
        </div>
        {c.detail && (
          <div className="truncate font-mono text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
            {c.detail}
          </div>
        )}
        {!c.found && c.suggestion && (
          <div className="mt-0.5 text-[length:var(--app-text-2xs)] leading-relaxed text-[var(--text-secondary)]">
            {c.suggestion}
          </div>
        )}
      </div>
    </div>
  )
}

function StepBody({
  icon,
  title,
  desc,
  children,
}: {
  icon: IconName
  title: string
  desc: string
  children?: React.ReactNode
}) {
  return (
    <div>
      <div className="flex items-start gap-2.5">
        <span className="mt-0.5 shrink-0 text-[var(--accent)]">
          <Icon name={icon} size={16} />
        </span>
        <div className="min-w-0">
          <h4 className="text-[length:var(--app-text-md)] font-semibold">{title}</h4>
          <p className="mt-0.5 text-[length:var(--app-text-xs)] leading-relaxed text-[var(--text-secondary)]">
            {desc}
          </p>
        </div>
      </div>
      <div className="mt-3">{children}</div>
    </div>
  )
}

function StepEnv({
  checks,
  error,
  onRetry,
  onGoSettings,
}: {
  checks: ToolchainCheck[] | null
  error: string | null
  onRetry: () => void
  onGoSettings: () => void
}) {
  const { t } = useTranslation()
  // 缺失数在子组件里就地算，不从父层传：父层那个副本只在这一处用，
  // 传下去等于多一个可能与 checks 不同步的量
  const missingCount = checks ? checks.filter((c) => !c.found).length : 0
  return (
    <StepBody
      icon="health"
      title={t('onboarding.step.env')}
      desc={t('onboarding.envDesc')}
    >
      {error ? (
        <div className="space-y-2 rounded-md border border-[var(--danger)]/40 bg-[var(--danger-50)] p-3">
          <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
            {t('onboarding.envFailed')}: {error}
          </p>
          <p className="text-[length:var(--app-text-2xs)] text-[var(--text-secondary)]">
            {t('onboarding.envFailedHint')}
          </p>
          <Button size="xs" icon="refresh" onClick={onRetry}>
            {t('common.retry')}
          </Button>
        </div>
      ) : checks === null ? (
        <div className="flex items-center gap-2 py-3 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
          <Spinner size={12} />
          {t('onboarding.checking')}
        </div>
      ) : (
        <div className="divide-y divide-[var(--border)] rounded-md border border-[var(--border)] px-3">
          {checks.map((c) => (
            <CheckRow key={c.name} c={c} />
          ))}
        </div>
      )}

      {checks && !error && (
        <p className="mt-3 text-[length:var(--app-text-2xs)] leading-relaxed text-[var(--text-muted)]">
          {missingCount === 0
            ? t('onboarding.envAllGood')
            : t('onboarding.envMissingSome', { n: missingCount })}
          {missingCount > 0 && (
            <>
              {' '}
              <button
                type="button"
                onClick={onGoSettings}
                className="text-[var(--accent)] hover:underline"
              >
                {t('onboarding.envGoSettings')}
              </button>
            </>
          )}
        </p>
      )}
    </StepBody>
  )
}

function StepProject({
  hasProjects,
  onAddProject,
  onSkip,
}: {
  hasProjects: boolean
  onAddProject?: () => void
  onSkip: () => void
}) {
  const { t } = useTranslation()
  if (hasProjects) {
    return (
      <StepBody icon="check" title={t('onboarding.step.project')} desc={t('onboarding.projectDesc')}>
        <div className="flex items-center gap-2 rounded-md border border-[var(--success)]/40 bg-[var(--success-50)] px-3 py-2.5 text-[length:var(--app-text-xs)] text-[var(--success)]">
          <Icon name="check" size={13} />
          {t('onboarding.projectDone')}
        </div>
      </StepBody>
    )
  }
  return (
    <StepBody icon="folder" title={t('onboarding.step.project')} desc={t('onboarding.projectDesc')}>
      <ul className="mb-3 space-y-1.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
        {(['1', '2', '3'] as const).map((n) => (
          <li key={n} className="flex gap-2">
            <span className="tnum shrink-0 text-[var(--text-muted)]">{n}.</span>
            <span>{t(`onboarding.projectTip.${n}`)}</span>
          </li>
        ))}
      </ul>
      <div className="flex gap-2">
        <Button variant="primary" icon="plus" onClick={onAddProject}>
          {t('onboarding.projectAdd')}
        </Button>
        <Button variant="ghost" onClick={onSkip}>
          {t('onboarding.projectLater')}
        </Button>
      </div>
    </StepBody>
  )
}

function StepProvider({
  count,
  error,
  onRetry,
  onGoProviders,
  onSkip,
}: {
  count: number | null
  error: string | null
  onRetry: () => void
  onGoProviders?: () => void
  onSkip: () => void
}) {
  const { t } = useTranslation()
  if (error) {
    return (
      <StepBody icon="bolt" title={t('onboarding.step.provider')} desc={t('onboarding.providerDesc')}>
        <div className="space-y-2">
          <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
            {t('onboarding.providerFailed')}: {error}
          </p>
          <Button size="xs" icon="refresh" onClick={onRetry}>
            {t('common.retry')}
          </Button>
        </div>
      </StepBody>
    )
  }
  if (count === null) {
    return (
      <StepBody icon="bolt" title={t('onboarding.step.provider')} desc={t('onboarding.providerDesc')}>
        <div className="flex items-center gap-2 py-3 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
          <Spinner size={12} />
          {t('onboarding.checking')}
        </div>
      </StepBody>
    )
  }
  if (count > 0) {
    return (
      <StepBody icon="bolt" title={t('onboarding.step.provider')} desc={t('onboarding.providerDesc')}>
        <div className="flex items-center gap-2 rounded-md border border-[var(--success)]/40 bg-[var(--success-50)] px-3 py-2.5 text-[length:var(--app-text-xs)] text-[var(--success)]">
          <Icon name="check" size={13} />
          {t('onboarding.providerDone', { n: count })}
        </div>
      </StepBody>
    )
  }
  return (
    <StepBody icon="bolt" title={t('onboarding.step.provider')} desc={t('onboarding.providerDesc')}>
      <p className="mb-3 text-[length:var(--app-text-xs)] leading-relaxed text-[var(--text-secondary)]">
        {t('onboarding.providerNone')}
      </p>
      <div className="flex gap-2">
        <Button variant="primary" icon="bolt" onClick={onGoProviders}>
          {t('onboarding.providerGo')}
        </Button>
        <Button variant="ghost" onClick={onSkip}>
          {t('onboarding.projectLater')}
        </Button>
      </div>
    </StepBody>
  )
}
