export type ThemeMode = 'light' | 'dark' | 'system'

export function readThemeCookie(): ThemeMode | undefined {
  const match = document.cookie.match(/(?:^|; )theme=([^;]+)/)
  return match ? (decodeURIComponent(match[1]) as ThemeMode) : undefined
}

export function writeThemeCookie(mode: ThemeMode) {
  const maxAge = 60 * 60 * 24 * 365 // 1 year
  document.cookie = `theme=${encodeURIComponent(mode)}; Path=/; Max-Age=${maxAge}`
}

export function applyTheme(mode: ThemeMode) {
  const root = document.documentElement
  const mql = window.matchMedia('(prefers-color-scheme: dark)')
  const setBy = (isDark: boolean) => {
    root.classList.toggle('dark', isDark)
  }
  if (mode === 'system') {
    setBy(mql.matches)
  } else if (mode === 'dark') {
    setBy(true)
  } else {
    setBy(false)
  }
}

import { useEffect, useMemo, useState } from 'react'

export function useThemePreference() {
  const [mode, setMode] = useState<ThemeMode>(() => readThemeCookie() ?? 'system')

  useEffect(() => {
    applyTheme(mode)
    writeThemeCookie(mode)

    if (mode === 'system') {
      const mql = window.matchMedia('(prefers-color-scheme: dark)')
      const onChange = () => applyTheme('system')
      mql.addEventListener('change', onChange)
      return () => mql.removeEventListener('change', onChange)
    }
  }, [mode])

  const cycle = useMemo(
    () => ({
      next(current: ThemeMode): ThemeMode {
        return current === 'system' ? 'light' : current === 'light' ? 'dark' : 'system'
      },
    }),
    []
  )

  return { mode, setMode, cycle }
}
