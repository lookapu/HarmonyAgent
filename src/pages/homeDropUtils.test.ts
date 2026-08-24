import { describe, expect, it } from 'vitest'
import { isImagePath, pathInProject } from './homeDropUtils'

describe('home file drop path handling', () => {
  it('accepts project children but rejects sibling-prefix paths', () => {
    expect(pathInProject('/work/app/src/main.ts', '/work/app')).toBe(true)
    expect(pathInProject('/work/application/secret.txt', '/work/app')).toBe(false)
  })

  it('preserves POSIX case sensitivity', () => {
    expect(pathInProject('/work/App/main.ts', '/work/app')).toBe(false)
  })

  it('normalizes Windows separators and drive-letter casing', () => {
    expect(pathInProject('c:\\Work\\App\\src\\main.ts', 'C:/work/app')).toBe(true)
  })

  it('recognizes supported image extensions case-insensitively', () => {
    expect(isImagePath('/tmp/preview.AVIF')).toBe(true)
    expect(isImagePath('/tmp/preview.txt')).toBe(false)
  })
})
