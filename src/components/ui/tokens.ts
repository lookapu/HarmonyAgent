/**
 * ui/ 原子层共享的 tone → className 映射
 *
 * tone 值刻意与 index.css 里既有的 .badge-tone-* 一一对应（info/ok/warn/bad），
 * 只把 bad 改名为 err 以对齐 Tone 联合类型的命名习惯，neutral 为本层新增。
 * 取色直接复用那套已经按主题手调过的 token：
 *   info → --accent / --accent-100      ok  → --success / --success-100
 *   warn → --warning-600 / --warning-50  err → --danger / --danger-50
 *
 * 底色透明度因此是「每主题一档」（暗色 10~18%、亮色 8~14%），而不是全主题统一
 * 一个百分比：亮底上同样的透明度会明显更重，既有的 -50 / -100 双档就是为此调的。
 * 本层要收敛的是此前 /10 /12 /15 /20 /25 五种任意透明度混用的问题——一律走这里。
 *
 * 边框不随 tone 变化：方形徽章统一用 --border 发丝线（IDE 靠描边分层），
 * 语义色只由文字 + 底色承担。
 */

export type Tone = 'neutral' | 'info' | 'ok' | 'warn' | 'err'

export const toneText: Record<Tone, string> = {
  neutral: 'text-[var(--text-secondary)]',
  info: 'text-[var(--accent)]',
  ok: 'text-[var(--success)]',
  warn: 'text-[var(--warning-600)]',
  err: 'text-[var(--danger)]',
}

export const toneWash: Record<Tone, string> = {
  neutral: 'bg-[var(--bg-hover)]',
  info: 'bg-[var(--accent-100)]',
  ok: 'bg-[var(--success-100)]',
  warn: 'bg-[var(--warning-50)]',
  err: 'bg-[var(--danger-50)]',
}
