import React from "react"
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { getApiBase } from "@/lib/config"

type Duration = number | null // milliseconds

function joinUrl(base: string, path: string): string {
	if (!path) return base
	if (/^https?:\/\//i.test(path)) return path
	const b = base.replace(/\/?$/, "")
	const p = path.startsWith("/") ? path : `/${path}`
	return `${b}${p}`
}

function readSettings() {
	try {
		const raw = localStorage.getItem("ark.ui.settings")
		if (!raw) return {}
		return JSON.parse(raw) as any
	} catch {
		return {}
	}
}

function getMgmtPaths(): { livez: string; readyz: string } {
	const s: any = readSettings()
	const livez = s?.ark?.management_server?.livez?.path
	const readyz = s?.ark?.management_server?.readyz?.path
	return {
		livez: typeof livez === "string" && livez.trim() ? livez.trim() : "/livez",
		readyz: typeof readyz === "string" && readyz.trim() ? readyz.trim() : "/readyz",
	}
}

async function measureGet(url: string): Promise<Duration> {
	try {
		const t0 = performance.now()
		const res = await fetch(url, { method: "GET", cache: "no-store" })
		// We only care about time to first byte/headers; but await text() to finish consistently
		await res.text().catch(() => undefined)
		const t1 = performance.now()
		return Math.max(0, Math.round(t1 - t0))
	} catch {
		return null
	}
}

export function HealthReadinessCard() {
	const [liveMs, setLiveMs] = React.useState<Duration>(null)
	const [readyMs, setReadyMs] = React.useState<Duration>(null)
	const [lastUpdated, setLastUpdated] = React.useState<number | null>(null)
	const [tick, setTick] = React.useState<number>(0) // 1s heartbeat for footer timer

	const base = React.useMemo(() => getApiBase(), [])
	const { livez, readyz } = React.useMemo(() => getMgmtPaths(), [])
	const liveUrl = React.useMemo(() => joinUrl(base, livez), [base, livez])
	const readyUrl = React.useMemo(() => joinUrl(base, readyz), [base, readyz])

	const runChecks = React.useCallback(async () => {
		const [lv, rz] = await Promise.all([
			measureGet(liveUrl),
			measureGet(readyUrl),
		])
		setLiveMs(lv)
		setReadyMs(rz)
		setLastUpdated(Date.now())
	}, [liveUrl, readyUrl])

	React.useEffect(() => {
		// initial
		runChecks()
		// every 30s
		const id = setInterval(runChecks, 30_000)
		return () => clearInterval(id)
	}, [runChecks])

	React.useEffect(() => {
		const id = setInterval(() => setTick((n) => n + 1), 1_000)
		return () => clearInterval(id)
	}, [])

	const footer = React.useMemo(() => {
		if (!lastUpdated) return "last check —"
		const secs = Math.max(0, Math.floor((Date.now() - lastUpdated) / 1000))
		return `last check ${secs} second${secs === 1 ? "" : "s"} ago`
	}, [lastUpdated, tick])

function fmt(v: Duration): React.ReactNode {
	if (v == null) return <span aria-label="unknown">—</span>
	return (
		<>
			<span className="ark-dashboard-probe-ms">{v}</span>
			<span className="ark-metrics-card-units"> ms</span>
		</>
	)
}

	return (
		<Card className="ark-numeric-stat">
			<CardHeader>
				<CardTitle className="ark-metrics-card-title">Health and readiness response</CardTitle>
			</CardHeader>
			<CardContent>
				<div>
						<div className="text-3xl font-semibold tracking-tight ark-numeric-state-value-half">{fmt(liveMs)} liveness </div>
						<hr/>
						<div className="text-3xl font-semibold tracking-tight ark-numeric-state-value-half"> {fmt(readyMs)} readiness</div>
				</div>

        
			</CardContent>
			<CardFooter>
				{footer}
			</CardFooter>
		</Card>
	)
}

export default HealthReadinessCard

