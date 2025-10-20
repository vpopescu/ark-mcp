import React, { useMemo } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { useSharedMetrics } from "@/lib/metrics-bus"

export function AvgLatencyCard() {
  const { samples } = useSharedMetrics()
  const avg = useMemo(() => {
    let sum = 0
    let count = 0
    for (const s of samples) {
      if (s.name === 'ark_mcp_latency_ms_sum') sum += s.value
      else if (s.name === 'ark_mcp_latency_ms_count') count += s.value
    }
    const v = count > 0 ? sum / count : 0
    return Number.isFinite(v) ? Math.round(v) : 0
  }, [samples])
  return (
    <Card className="ark-numeric-stat">
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Average tool latency</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="text-3xl font-semibold tracking-tight ark-numeric-state-value">{avg ? `${avg} ms` : '—'}</div>
      </CardContent>
    </Card>
  )
}

export default AvgLatencyCard
