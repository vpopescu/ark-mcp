import { useEffect, useMemo, useState } from 'react'
import { getApiBase } from './config'

export type PromSample = { name: string; labels: Record<string, string>; value: number }
export type Snapshot = { time: number; map: Map<string, number> }
export type LatencyPoint = { t: number; avgMs: number }

function parseLabels(s: string): Record<string, string> {
  const out: Record<string, string> = {}
  let i = 0
  while (i < s.length) {
    let j = i
    while (j < s.length && s[j] !== '=') j++
    const key = s.slice(i, j).trim()
    if (j >= s.length || s[j] !== '=') break
    j++
    if (j < s.length && s[j] === '"') {
      j++
      let val = ''
      while (j < s.length) {
        const ch = s[j]
        if (ch === '\\' && j + 1 < s.length) {
          const nxt = s[++j]
          if (nxt === 'n') val += '\n'
          else if (nxt === '"') val += '"'
          else if (nxt === '\\') val += '\\'
          else val += nxt
          j++
          continue
        }
        if (ch === '"') { j++; break }
        val += ch
        j++
      }
      out[key] = val
      while (j < s.length && (s[j] === ',' || s[j] === ' ')) j++
      i = j
    } else {
      break
    }
  }
  return out
}

function keyOf(s: PromSample): string {
  const keys = Object.keys(s.labels).sort()
  const lbl = keys.map(k => `${k}=${JSON.stringify(s.labels[k])}`).join(',')
  return `${s.name}{${lbl}}`
}

function parsePromText(text: string): PromSample[] {
  const out: PromSample[] = []
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    let name = ''
    let labels: Record<string, string> = {}
    let rest = ''
    const i = line.indexOf('{')
    if (i >= 0) {
      const j = line.indexOf('}', i + 1)
      if (j < 0) continue
      name = line.slice(0, i)
      labels = parseLabels(line.slice(i + 1, j))
      rest = line.slice(j + 1).trim()
    } else {
      const sp = line.indexOf(' ')
      if (sp < 0) continue
      name = line.slice(0, sp)
      rest = line.slice(sp + 1).trim()
    }
    const valStr = rest.split(/\s+/)[0]
    const value = Number(valStr)
    if (!Number.isFinite(value)) continue
    out.push({ name, labels, value })
  }
  return out
}

function toSnapshot(samples: PromSample[]): Snapshot {
  const map = new Map<string, number>()
  for (const s of samples) map.set(keyOf(s), s.value)
  return { time: Date.now(), map }
}

export type PerToolLatencySeries = { plugin: string; tool: string; points: LatencyPoint[] }
export type PerToolLatencyHistory = Record<string, PerToolLatencySeries>
export type ToolCallTotals = Record<string, { plugin: string; tool: string; total: number }>
export type MetricsPayload = {
  samples: PromSample[]
  curr: Snapshot | null
  prev: Snapshot | null
  latencyHistory: LatencyPoint[]
  perToolLatencyHistory: PerToolLatencyHistory
  toolCallTotals: ToolCallTotals
}

// Singleton polling bus
let listeners = new Set<(p: MetricsPayload) => void>()
let timer: any = null
let last: MetricsPayload = { samples: [], curr: null, prev: null, latencyHistory: [], perToolLatencyHistory: {}, toolCallTotals: {} }

const HISTORY_KEY = 'ark.ui.metrics.latencyHistory'
const PER_TOOL_HISTORY_KEY = 'ark.ui.metrics.perToolLatencyHistory'
const TOOL_CALLS_TOTALS_KEY = 'ark.ui.metrics.toolCallsTotals'
const HISTORY_MAX_AGE_MS = 35 * 24 * 60 * 60 * 1000 // 35 days

function loadHistory(): LatencyPoint[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw) as LatencyPoint[]
    const cutoff = Date.now() - HISTORY_MAX_AGE_MS
    return (Array.isArray(arr) ? arr : []).filter((p) => typeof p?.t === 'number' && Number.isFinite(p.t) && typeof p?.avgMs === 'number' && Number.isFinite(p.avgMs) && p.t >= cutoff)
  } catch {
    return []
  }
}

function saveHistory(points: LatencyPoint[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(points))
  } catch { }
}

function loadPerToolHistory(): PerToolLatencyHistory {
  try {
    const raw = localStorage.getItem(PER_TOOL_HISTORY_KEY)
    if (!raw) return {}
    const obj = JSON.parse(raw) as any
    const out: PerToolLatencyHistory = {}
    const cutoff = Date.now() - HISTORY_MAX_AGE_MS
    for (const [key, val] of Object.entries(obj || {})) {
      const plugin = (val as any)?.plugin || 'unknown'
      const tool = (val as any)?.tool || 'unknown'
      const pts = Array.isArray((val as any)?.points) ? (val as any).points : []
      const points: LatencyPoint[] = pts
        .filter((p: any) => typeof p?.t === 'number' && Number.isFinite(p.t) && typeof p?.avgMs === 'number' && Number.isFinite(p.avgMs) && p.t >= cutoff)
        .slice(-10000)
      if (points.length > 0) out[key] = { plugin, tool, points }
    }
    return out
  } catch {
    return {}
  }
}

function savePerToolHistory(history: PerToolLatencyHistory) {
  try {
    localStorage.setItem(PER_TOOL_HISTORY_KEY, JSON.stringify(history))
  } catch { }
}

function loadToolCallTotals(): ToolCallTotals {
  try {
    const raw = localStorage.getItem(TOOL_CALLS_TOTALS_KEY)
    if (!raw) return {}
    const obj = JSON.parse(raw) as any
    const out: ToolCallTotals = {}
    for (const [key, val] of Object.entries(obj || {})) {
      const plugin = (val as any)?.plugin || 'unknown'
      const tool = (val as any)?.tool || 'unknown'
      const total = Number((val as any)?.total) || 0
      out[key] = { plugin, tool, total }
    }
    return out
  } catch {
    return {}
  }
}

function saveToolCallTotals(totals: ToolCallTotals) {
  try {
    localStorage.setItem(TOOL_CALLS_TOTALS_KEY, JSON.stringify(totals))
  } catch { }
}

async function fetchOnce(base: string) {
  try {
    const res = await fetch(`${base.replace(/\/$/, '')}/metrics`, { cache: 'no-cache' })
    if (!res.ok) return
    const text = await res.text()
    const samples = parsePromText(text)
    const snap = toSnapshot(samples)
    const prev = last.curr
    // Compute interval average latency across all tools using ark_tool_latency_ms_{sum,count}
    let history = last.latencyHistory.length ? last.latencyHistory.slice() : loadHistory()
    // Per-tool latency history (in-memory, persisted)
    const basePerTool = (last.perToolLatencyHistory && Object.keys(last.perToolLatencyHistory).length)
      ? last.perToolLatencyHistory
      : loadPerToolHistory()
    const perToolHistory: PerToolLatencyHistory = { ...(basePerTool || {}) }

    // Tool call totals (persisted)
    const toolTotalsBase = (last.toolCallTotals && Object.keys(last.toolCallTotals).length)
      ? last.toolCallTotals
      : loadToolCallTotals()
    const toolCallTotals: ToolCallTotals = { ...(toolTotalsBase || {}) }
    // Build current totals by series for potential seeding
    const curSumByKey = new Map<string, { plugin: string; tool: string; value: number }>()
    const curCountByKey = new Map<string, { plugin: string; tool: string; value: number }>()
    let cumSum = 0
    let cumCount = 0
    for (const s of samples) {
      if (s.name === 'ark_tool_latency_ms_sum' || s.name === 'ark_tool_latency_ms_count') {
        const plugin = s.labels.plugin || 'unknown'
        const tool = s.labels.tool || 'unknown'
        const seriesKey = `${plugin}::${tool}`
        if (s.name === 'ark_tool_latency_ms_sum') { curSumByKey.set(seriesKey, { plugin, tool, value: s.value }); cumSum += s.value }
        else { curCountByKey.set(seriesKey, { plugin, tool, value: s.value }); cumCount += s.value }
      }
    }

    // Seed newly seen tools from current samples (latency and calls) so they appear immediately in the UI
    const seedSeriesKey = (plugin: string, tool: string) => {
      const key = `${plugin}::${tool}`
      const entry = perToolHistory[key] || { plugin, tool, points: [] }
      const lastPt = entry.points[entry.points.length - 1]
      if (!lastPt || lastPt.t !== snap.time) {
        if (entry.points.length === 0) {
          const sTot = curSumByKey.get(key)?.value || 0
          const cTot = curCountByKey.get(key)?.value || 0
          const avg = cTot > 0 ? (sTot / cTot) : 0
          if (Number.isFinite(avg)) entry.points.push({ t: snap.time, avgMs: avg })
          perToolHistory[key] = entry
        }
      }
    }
    // From latency metrics
    for (const [key, info] of curSumByKey) seedSeriesKey(info.plugin, info.tool)
    for (const [key, info] of curCountByKey) seedSeriesKey(info.plugin, info.tool)
    // From call counters (support both _total and legacy name)
    for (const s of samples) {
      if (s.name === 'ark_tool_calls_total' || s.name === 'ark_tool_calls') {
        const plugin = s.labels.plugin || 'unknown'
        const tool = s.labels.tool || 'unknown'
        seedSeriesKey(plugin, tool)
      }
    }
    // On first snapshot (no prev), seed overall latency with cumulative average so the chart has a point
    if (!prev) {
      const seedAvg = cumCount > 0 ? (cumSum / cumCount) : 0
      if (Number.isFinite(seedAvg)) {
        history.push({ t: snap.time, avgMs: seedAvg })
        saveHistory(history)
      }
    }

    if (prev) {
      let sumDelta = 0
      let countDelta = 0
      const perSumDelta = new Map<string, number>()
      const perCountDelta = new Map<string, number>()
      for (const s of samples) {
        if (s.name !== 'ark_tool_latency_ms_sum' && s.name !== 'ark_tool_latency_ms_count') continue
        const k = keyOf(s)
        const v0 = prev.map.get(k)
        if (typeof v0 !== 'number') continue
        const dv = s.value - v0
        if (!Number.isFinite(dv) || dv < 0) continue
        const plugin = s.labels.plugin || 'unknown'
        const tool = s.labels.tool || 'unknown'
        const seriesKey = `${plugin}::${tool}`
        if (s.name === 'ark_tool_latency_ms_sum') {
          sumDelta += dv
          perSumDelta.set(seriesKey, (perSumDelta.get(seriesKey) || 0) + dv)
        } else {
          countDelta += dv
          perCountDelta.set(seriesKey, (perCountDelta.get(seriesKey) || 0) + dv)
        }
      }
      if (countDelta > 0) {
        const avgMs = sumDelta / countDelta
        if (Number.isFinite(avgMs)) history.push({ t: snap.time, avgMs })
      } else {
        // No calls this interval: push zero so the chart decays to 0
        history.push({ t: snap.time, avgMs: 0 })
      }
      // Trim to max age and cap number of points
      const cutoff = Date.now() - HISTORY_MAX_AGE_MS
      if (history.length > 10000 || history[0]?.t < cutoff) {
        history = history.filter((p) => p.t >= cutoff).slice(-10000)
      }
      saveHistory(history)

      // Per-tool: record points per series where we have deltas; seed empty series with cumulative
      const seriesKeys = new Set<string>([...perSumDelta.keys(), ...perCountDelta.keys(), ...curSumByKey.keys(), ...curCountByKey.keys()])
      for (const key of seriesKeys) {
        const plugin = (curSumByKey.get(key) || curCountByKey.get(key))?.plugin || 'unknown'
        const tool = (curSumByKey.get(key) || curCountByKey.get(key))?.tool || 'unknown'
        const entry = perToolHistory[key] || { plugin, tool, points: [] }
        const sDelta = perSumDelta.get(key) || 0
        const cDelta = perCountDelta.get(key) || 0
        const lastPt = entry.points[entry.points.length - 1]
        if (cDelta > 0) {
          const avg = sDelta / cDelta
          if (Number.isFinite(avg)) {
            if (!lastPt || lastPt.t !== snap.time) entry.points.push({ t: snap.time, avgMs: avg })
            else lastPt.avgMs = avg
            perToolHistory[key] = entry
          }
        } else if (entry.points.length === 0) {
          const sTot = curSumByKey.get(key)?.value || 0
          const cTot = curCountByKey.get(key)?.value || 0
          if (cTot > 0) {
            const avg = sTot / cTot
            if (Number.isFinite(avg)) {
              if (!lastPt || lastPt.t !== snap.time) entry.points.push({ t: snap.time, avgMs: avg })
              else lastPt.avgMs = avg
              perToolHistory[key] = entry
            }
          } else {
            // No history and no cumulative yet; start with zero to allow decay visualization
            if (!lastPt || lastPt.t !== snap.time) entry.points.push({ t: snap.time, avgMs: 0 })
            else lastPt.avgMs = 0
            perToolHistory[key] = entry
          }
        } else {
          // Existing series but no calls this interval: push/update zero
          if (!lastPt || lastPt.t !== snap.time) entry.points.push({ t: snap.time, avgMs: 0 })
          else lastPt.avgMs = 0
          perToolHistory[key] = entry
        }
        // Trim old points for this series as well
        const cutoff = Date.now() - HISTORY_MAX_AGE_MS
        if (perToolHistory[key]) {
          const pts = perToolHistory[key].points
          if (pts.length > 10000 || (pts[0] && pts[0].t < cutoff)) {
            perToolHistory[key].points = pts.filter((p) => p.t >= cutoff).slice(-10000)
          }
        }
      }

      // Aggregate tool call deltas into persisted totals
      for (const s of samples) {
        if (!(s.name === 'ark_tool_calls_total' || s.name === 'ark_tool_calls')) continue
        const plugin = s.labels.plugin || 'unknown'
        const tool = s.labels.tool || 'unknown'
        const seriesKey = `${plugin}::${tool}`
        const k = keyOf(s)
        const v0 = prev.map.get(k)
        if (typeof v0 === 'number') {
          let dv = s.value - v0
          if (!Number.isFinite(dv)) dv = 0
          if (dv < 0) dv = s.value // counter reset; treat current as first increment
          if (dv > 0) {
            const cur = toolCallTotals[seriesKey] || { plugin, tool, total: 0 }
            cur.total += dv
            toolCallTotals[seriesKey] = cur
          }
        }
      }
    }
    last = { samples, curr: snap, prev, latencyHistory: history, perToolLatencyHistory: perToolHistory, toolCallTotals }
    savePerToolHistory(perToolHistory)
    saveToolCallTotals(toolCallTotals)
    for (const cb of listeners) cb(last)
  } catch {
    // ignore
  }
}

function ensurePolling(intervalMs: number) {
  if (timer) return
  const base = getApiBase()
  fetchOnce(base)
  timer = setInterval(() => fetchOnce(base), intervalMs)
}

function maybeStop() {
  if (listeners.size === 0 && timer) {
    clearInterval(timer)
    timer = null
  }
}

export function subscribeMetrics(cb: (p: MetricsPayload) => void, intervalMs = 20000): () => void {
  listeners.add(cb)
  cb(last)
  ensurePolling(intervalMs)
  return () => { listeners.delete(cb); maybeStop() }
}

export function useSharedMetrics(intervalMs = 20000): MetricsPayload {
  const [state, setState] = useState<MetricsPayload>(last)
  useEffect(() => subscribeMetrics(setState, intervalMs), [intervalMs])
  return state
}

export function deltaRate(curr: Snapshot, prev: Snapshot, predicate: (key: string) => boolean): number {
  const dt = Math.max(1, (curr.time - prev.time) / 1000)
  let dsum = 0
  for (const [k, v] of curr.map) {
    if (!predicate(k)) continue
    const v0 = prev.map.get(k) ?? v
    const dv = v - v0
    if (Number.isFinite(dv) && dv >= 0) dsum += dv
  }
  return dsum / dt
}

export type LatencyPeriod = '1m' | '5m' | 'today' | 'week' | 'month'

export function useLatencyHistory(period: LatencyPeriod): LatencyPoint[] {
  const { latencyHistory } = useSharedMetrics()
  const now = Date.now()
  const start = useMemo(() => {
    if (period === '1m') return now - 60_000
    if (period === '5m') return now - 5 * 60_000
    const d = new Date()
    if (period === 'today') {
      d.setHours(0, 0, 0, 0)
      return d.getTime()
    }
    if (period === 'week') {
      const day = d.getDay() // 0=Sun..6=Sat
      const diff = (day + 6) % 7 // days since Monday
      d.setHours(0, 0, 0, 0)
      d.setDate(d.getDate() - diff)
      return d.getTime()
    }
    if (period === 'month') {
      d.setHours(0, 0, 0, 0)
      d.setDate(1)
      return d.getTime()
    }
    return now - 60_000
  }, [period, now])
  return useMemo(() => (latencyHistory || []).filter((p) => p.t >= start), [latencyHistory, start])
}

export function usePerToolLatencyHistory(period: LatencyPeriod) {
  const { perToolLatencyHistory } = useSharedMetrics()
  const now = Date.now()
  const start = useMemo(() => {
    if (period === '1m') return now - 60_000
    if (period === '5m') return now - 5 * 60_000
    const d = new Date()
    if (period === 'today') { d.setHours(0, 0, 0, 0); return d.getTime() }
    if (period === 'week') {
      const day = d.getDay(); const diff = (day + 6) % 7; d.setHours(0, 0, 0, 0); d.setDate(d.getDate() - diff); return d.getTime()
    }
    if (period === 'month') { d.setHours(0, 0, 0, 0); d.setDate(1); return d.getTime() }
    return now - 60_000
  }, [period, now])
  return useMemo(() => {
    const out: Array<{ key: string; plugin: string; tool: string; points: LatencyPoint[] }> = []
    for (const [key, series] of Object.entries(perToolLatencyHistory || {})) {
      const pts = (series.points || []).filter((p) => p.t >= start)
      if (pts.length === 0) continue
      out.push({ key, plugin: series.plugin, tool: series.tool, points: pts })
    }
    return out
  }, [perToolLatencyHistory, start])
}
