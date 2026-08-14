import bolt from './bolt.svg'
import pkg from './package.svg'
import settings from './settings.svg'
import payments from './payments.svg'
import mcp from './mcp.svg'
import skill from './skill.svg'
import health from './health.svg'
import refresh from './refresh.svg'
import addCircle from './add-circle.svg'
import del from './delete.svg'
import check from './check.svg'
import close from './close.svg'
import download from './download.svg'
import edit from './edit.svg'
import info from './info.svg'
import switchIcon from './switch.svg'
import moreVert from './more-vert.svg'
import proxy from './proxy.svg'
import folder from './folder.svg'
import chat from './chat.svg'
import send from './send.svg'
import plus from './plus.svg'
import panel from './panel.svg'
import spark from './spark.svg'
import devices from './devices.svg'
import sun from './sun.svg'
import moon from './moon.svg'
import chevronLeft from './chevron-left.svg'
import chevronRight from './chevron-right.svg'
import gitBranch from './git-branch.svg'
import file from './file.svg'
import pin from './pin.svg'
import archive from './archive.svg'
import lightbulb from './lightbulb.svg'
import receipt from './receipt.svg'
import copy from './copy.svg'
import headphones from './headphones.svg'
import language from './language.svg'
import notifications from './notifications.svg'
import search from './search.svg'
import terminal from './terminal.svg'
import phone from './phone.svg'
import arrowDown from './arrow-down.svg'

export const icons = {
  bolt,
  package: pkg,
  settings,
  payments,
  mcp,
  skill,
  health,
  refresh,
  'add-circle': addCircle,
  delete: del,
  check,
  close,
  download,
  edit,
  info,
  switch: switchIcon,
  'more-vert': moreVert,
  proxy,
  folder,
  chat,
  send,
  plus,
  panel,
  spark,
  devices,
  sun,
  moon,
  'chevron-left': chevronLeft,
  'chevron-right': chevronRight,
  'git-branch': gitBranch,
  file,
  pin,
  archive,
  lightbulb,
  receipt,
  copy,
  headphones,
  language,
  notifications,
  search,
  terminal,
  phone,
  'arrow-down': arrowDown,
} as const

export type IconName = keyof typeof icons

interface IconProps {
  name: IconName
  size?: number
  className?: string
  /** 用于 accent/success 等彩色背景按钮上：将图标强制渲染为白色 */
  white?: boolean
}

export default function Icon({ name, size = 20, className = '', white = false }: IconProps) {
  const src = icons[name]
  return (
    <img
      src={src}
      alt={name}
      width={size}
      height={size}
      className={`inline-block ${className}`}
      style={{ filter: white ? 'brightness(0) invert(1)' : 'var(--icon-filter)' }}
    />
  )
}
