import { useEffect, useMemo, useState } from 'react'

const VAR_NAMES = ['--chart-1', '--chart-2', '--chart-3', '--chart-4', '--chart-5']

function resolveVarToColor(varName: string): string {
  if (typeof window === 'undefined' || !document?.body) return `var(${varName})`
  const el = document.createElement('div')
  el.style.color = `var(${varName})`
  // Position offscreen to avoid layout impact
  el.style.position = 'absolute'
  el.style.left = '-9999px'
  el.style.top = '-9999px'
  document.body.appendChild(el)
  const color = getComputedStyle(el).color || `var(${varName})`
  document.body.removeChild(el)
  return color
}

function resolveAll(): string[] {
  return VAR_NAMES.map((n) => resolveVarToColor(n))
}

function allSame(arr: string[]): boolean {
  return arr.length > 0 && arr.every((c) => c === arr[0])
}

export function useChartColors(): string[] {
  const fallback = useMemo(() => VAR_NAMES.map((n) => `var(${n})`), [])
  const [colors, setColors] = useState<string[]>(fallback)

  useEffect(() => {
    let cancelled = false
    const update = () => {
      const resolved = resolveAll()
      if (!cancelled) setColors(resolved)
    }

    // Initial attempt after frame to allow CSS to apply
    const raf = requestAnimationFrame(() => setTimeout(update, 0))

    // Re-resolve on page load/readiness changes
    const onLoad = () => update()
    window.addEventListener('load', onLoad)
    document.addEventListener('readystatechange', onLoad)

    // Re-resolve on theme class changes (e.g., toggling .dark)
    const mo = new MutationObserver((muts) => {
      for (const m of muts) {
        if (m.type === 'attributes' && m.attributeName === 'class') {
          update()
          break
        }
      }
    })
    mo.observe(document.documentElement, { attributes: true })

    // Also track prefers-color-scheme changes as a fallback
    const mq = window.matchMedia?.('(prefers-color-scheme: dark)')
    const mqHandler = () => update()
    mq?.addEventListener?.('change', mqHandler as any)

    return () => {
      cancelled = true
      cancelAnimationFrame(raf)
      window.removeEventListener('load', onLoad)
      document.removeEventListener('readystatechange', onLoad)
      mo.disconnect()
      mq?.removeEventListener?.('change', mqHandler as any)
    }
  }, [])

  // If everything somehow resolves to the same color, expose the fallback var() tokens
  return allSame(colors) ? fallback : colors
}
