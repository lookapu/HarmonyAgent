const IMAGE_EXTS = ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.svg', '.avif']

export function isImagePath(path: string): boolean {
  return IMAGE_EXTS.some((ext) => path.toLowerCase().endsWith(ext))
}

function normalizePath(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/+$/, '')
}

function isWindowsAbsolutePath(path: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(path)
}

/** 判断拖入的绝对路径是否位于项目根内；Windows 路径忽略大小写，POSIX 路径保留大小写语义。 */
export function pathInProject(path: string, root: string): boolean {
  let candidate = normalizePath(path)
  let projectRoot = normalizePath(root)
  if (isWindowsAbsolutePath(candidate) && isWindowsAbsolutePath(projectRoot)) {
    candidate = candidate.toLowerCase()
    projectRoot = projectRoot.toLowerCase()
  }
  return candidate === projectRoot || candidate.startsWith(`${projectRoot}/`)
}
