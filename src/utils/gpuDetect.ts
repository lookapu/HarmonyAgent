/**
 * GPU 能力检测与渲染性能分级
 *
 * 通过 WebGL 渲染器字符串判断是否走硬件加速路径：
 * - 独立显卡 / Apple Metal / 良好集成显卡 → "high"：启用全部动画、较高 overscan、完整语法高亮
 * - 基本硬件加速（普通集成显卡）→ "medium"：平衡配置（当前默认）
 * - 软件渲染（SwiftShader/llvmpipe/Microsoft Basic Render）→ "low"：
 *   降低 overscan、禁用 smooth 动画、收紧高亮阈值、轻量 Markdown 解析
 *
 * 平台区分（Win/Mac）：Mac 通常 Core Animation 合成器效率高，Windows 下软件渲染更常见（远程桌面/虚拟机）
 */

export type RenderTier = 'high' | 'medium' | 'low'

export interface GpuInfo {
  /** 渲染性能分级 */
  tier: RenderTier
  /** WebGL 渲染器名称（如 "Apple M1"、"NVIDIA GeForce RTX 3060"、"SwiftShader"） */
  renderer: string
  /** 是否硬件加速（非软件渲染） */
  hardwareAccelerated: boolean
  /** 平台：'mac' | 'windows' | 'other' */
  platform: 'mac' | 'windows' | 'other'
  /** CPU 核心数（超线程逻辑核） */
  cores: number
  /** 设备内存 GB（不可用时为 0） */
  memoryGB: number
}

// 软件渲染器关键词（黑名单）——出现这些说明 GPU 不可用或被禁用
const SOFTWARE_RENDERER_PATTERNS = [
  /swiftshader/i,           // Chrome/Edge 软件渲染回退
  /llvmpipe/i,              // Mesa 软件渲染（Linux/某些虚拟化）
  /softpipe/i,              // Mesa 软件渲染
  /svga3d/i,                // VMware 虚拟 GPU
  /virgl/i,                 // VirtIO 虚拟 GPU（部分场景性能差）
  /microsoft basic render/i,// Windows 基本渲染驱动（无驱动/远程桌面）
  /microsoft enhanced/i,    // Windows 增强会话（远程桌面软件路径）
  /gdi generic/i,           // Windows GDI 软件渲染
  /software rasterizer/i,   // 通用软件光栅化器
  /vbox/i,                  // VirtualBox 虚拟 GPU
  /parallels/i,             // Parallels 虚拟 GPU（部分版本无硬件加速）
  /dummy/i,                 // 占位/虚拟
]

// 高性能 GPU 关键词
const HIGH_END_GPU_PATTERNS = [
  // Apple Silicon / Metal
  /apple m\d/i, /apple a\d/i, /apple gpu/i, /metal/i,
  // NVIDIA 独显
  /geforce rtx/i, /geforce gtx/i, /quadro/i, /tesla/i, /nvidia/i,
  // AMD 独显
  /radeon rx/i, /radeon pro/i, /radeon vega/i, /amd radeon/i, /firepro/i,
  // Intel 独显
  /intel.*iris/i, /intel.*arc/i, /intel.*uhd/i,
]

function detectPlatform(): 'mac' | 'windows' | 'other' {
  if (typeof navigator === 'undefined') return 'other'
  const ua = navigator.userAgent.toLowerCase()
  const plat = (navigator.platform || '').toLowerCase()
  if (plat.includes('mac') || ua.includes('macintosh') || ua.includes('mac os')) return 'mac'
  if (plat.includes('win') || ua.includes('windows')) return 'windows'
  return 'other'
}

function detectWebGL(): { renderer: string; accelerated: boolean } {
  if (typeof document === 'undefined') return { renderer: '', accelerated: false }
  const canvas = document.createElement('canvas')
  let gl: WebGLRenderingContext | WebGL2RenderingContext | null
  try {
    gl = canvas.getContext('webgl2') || canvas.getContext('webgl')
  } catch {
    return { renderer: '', accelerated: false }
  }
  if (!gl) return { renderer: '', accelerated: false }

  let renderer = ''
  try {
    const ext = gl.getExtension('WEBGL_debug_renderer_info')
    if (ext) {
      renderer = gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) as string || ''
    }
    if (!renderer) {
      renderer = gl.getParameter(gl.RENDERER) as string || ''
    }
  } catch {
    renderer = ''
  }

  // 判断是否软件渲染
  const isSoftware = SOFTWARE_RENDERER_PATTERNS.some((p) => p.test(renderer))
  // 注意：renderer 为空时（被浏览器隐私策略屏蔽，如 Firefox 默认），
  // 不能确定是软件渲染，给 medium 保底
  const accelerated = !isSoftware

  // 清理 canvas
  gl.getExtension('WEBGL_lose_context')?.loseContext()
  return { renderer, accelerated }
}

function computeTier(info: { renderer: string; accelerated: boolean; platform: 'mac' | 'windows' | 'other'; cores: number; memoryGB: number }): RenderTier {
  if (!info.accelerated) return 'low'

  const { renderer, cores, memoryGB, platform } = info

  // Mac 默认至少 medium（Core Animation 合成器效率高，即使 Intel 集显也不差）
  // 但虚拟机/远程桌面场景 accelerated 已为 false，不会走到这里
  const isHighEnd = HIGH_END_GPU_PATTERNS.some((p) => p.test(renderer))

  // 高分：Apple Silicon / 独显 / 8核+16G+ → high
  if (isHighEnd && cores >= 6) return 'high'
  // Mac 且核心数足够（Apple Silicon 统一内存架构，4 核也够用）→ high
  if (platform === 'mac' && cores >= 6 && (memoryGB === 0 || memoryGB >= 8)) return 'high'
  // Windows 独显 → high
  if (platform === 'windows' && isHighEnd && cores >= 6) return 'high'
  // 基本硬件加速但未达高分之列 → medium
  return 'medium'
}

let cachedInfo: GpuInfo | null = null
let detectionRan = false

/** 检测 GPU 能力（单例，多次调用返回缓存结果） */
export function detectGpu(): GpuInfo {
  if (detectionRan && cachedInfo) return cachedInfo
  detectionRan = true

  const platform = detectPlatform()
  const { renderer, accelerated } = detectWebGL()
  const cores = typeof navigator !== 'undefined' ? navigator.hardwareConcurrency || 4 : 4
  const memoryGB = (navigator as Navigator & { deviceMemory?: number }).deviceMemory || 0

  const tier = computeTier({ renderer, accelerated, platform, cores, memoryGB })

  cachedInfo = {
    tier,
    renderer,
    hardwareAccelerated: accelerated,
    platform,
    cores,
    memoryGB,
  }

  // 开发调试：输出到控制台方便排查
  if (typeof console !== 'undefined') {
    console.debug('[GPU detect]', cachedInfo)
  }

  return cachedInfo
}

/** 根据渲染分级返回推荐的虚拟列表 overscan 数 */
export function getRecommendedOverscan(tier: RenderTier): number {
  switch (tier) {
    case 'high': return 6
    case 'medium': return 4
    case 'low': return 2
  }
}

/** 根据渲染分级返回是否启用 smooth 滚动动画 */
export function shouldUseSmoothScroll(tier: RenderTier): boolean {
  return tier !== 'low'
}

/** 根据渲染分级返回代码块语法高亮阈值（字符数） */
export function getCodeHighlightLimit(tier: RenderTier): number {
  switch (tier) {
    case 'high': return 6000   // 高性能 GPU：支持更大代码块高亮
    case 'medium': return 3000 // 默认
    case 'low': return 1200    // 低性能：收紧阈值，更多代码块走纯文本
  }
}

/** 根据渲染分级返回 Markdown 轻量模式阈值（字符数） */
export function getMarkdownLightThreshold(tier: RenderTier): number {
  switch (tier) {
    case 'high': return 30000
    case 'medium': return 15000
    case 'low': return 6000
  }
}

/** 根据渲染分级返回代码块默认折叠阈值（行数） */
export function getCollapseThreshold(tier: RenderTier): number {
  switch (tier) {
    case 'high': return 50
    case 'medium': return 30
    case 'low': return 20
  }
}

/** CSS class 名映射：在根元素上挂 data-render-tier 属性，CSS 可据此启停特效 */
export function getTierClass(tier: RenderTier): string {
  return `render-tier-${tier}`
}
