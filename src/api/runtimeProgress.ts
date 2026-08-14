/** 运行时（Node / Git / JDK）下载/安装进度（后端事件推送，不阻塞界面） */
export interface RuntimeProgress {
  /** 阶段：check / download / verify / extract / done */
  phase: 'check' | 'download' | 'verify' | 'extract' | 'done'
  /** 阶段描述（直接展示） */
  message: string
  /** 下载进度百分比（0-100，download 阶段有效） */
  percent: number | null
  /** 已下载字节数 */
  downloaded: number | null
  /** 总字节数（未知时为 null） */
  total: number | null
  /** 实时速度（字节/秒） */
  speed: number | null
}
