import React, { useMemo } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { useSharedMetrics, deltaRate } from "@/lib/metrics-bus"

export function ThroughputCard() {
  const { curr, prev } = useSharedMetrics()
  const rate = useMemo(() => {
    if (!curr || !prev) return 0
    const r = deltaRate(curr, prev, (k) => k.startsWith('ark_mcp_calls_total{'))
    return Number.isFinite(r) ? r : 0
  }, [curr, prev])
  return (
    <Card className="ark-numeric-stat">
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Average tool throughput</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="text-3xl font-semibold tracking-tigh ark-numeric-state-value">{`${rate.toFixed(1)} req/s`}</div>
      </CardContent>
    </Card>
  )
}

export default ThroughputCard
