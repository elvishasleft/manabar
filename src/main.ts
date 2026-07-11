import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type RateWindow = { label: string; used_percent: number; resets_at: string | null };
type Quota = { plan: string | null; windows: RateWindow[]; fetched_at: string };
type DayUsage = {
  date: string;
  input_tokens: number;
  output_tokens: number;
  est_cost_usd: number | null;
};
type View = {
  kind: "claude" | "codex" | "grok";
  health: "green" | "amber" | "red" | "unavailable";
  remaining_percent: number | null;
  quota: Quota | null;
  error: string | null;
  error_kind: string | null;
  usage: { days: DayUsage[] } | null;
  updated_at: string | null;
};

const NAMES: Record<View["kind"], string> = { claude: "Claude", codex: "Codex", grok: "Grok" };
const CLI: Record<View["kind"], string> = { claude: "Claude Code", codex: "Codex", grok: "Grok" };

let current: View[] = [];

const fmtTokens = (n: number) =>
  new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 }).format(n);

function countdown(resetsAt: string | null): string {
  if (!resetsAt) return "";
  const secs = Math.floor((new Date(resetsAt).getTime() - Date.now()) / 1000);
  if (secs <= 0) return "resets now";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${Math.max(m, 1)}m`;
}

function age(iso: string | null): string {
  if (!iso) return "";
  const mins = Math.floor((Date.now() - new Date(iso).getTime()) / 60000);
  return mins < 1 ? "just now" : mins < 60 ? `${mins}m ago` : `${Math.floor(mins / 60)}h ago`;
}

function errorCopy(v: View): string {
  switch (v.error_kind) {
    case "no_credentials":
      return "Not installed / not signed in";
    case "token_expired":
      return `Sign-in expired — open ${CLI[v.kind]} to refresh`;
    case "network":
      return `Offline — retrying (last data ${age(v.updated_at) || "never"})`;
    case "schema_changed":
      return "Endpoint changed — needs an update";
    default:
      return v.error ?? "";
  }
}

function sparkline(days: DayUsage[]): string {
  const vals = days.map((d) => d.input_tokens + d.output_tokens);
  const max = Math.max(...vals, 1);
  const pts = vals
    .map((v, i) => `${(i / (vals.length - 1)) * 100},${28 - (v / max) * 26}`)
    .join(" ");
  return `<svg class="spark" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true"><polyline points="${pts}" fill="none" /></svg>`;
}

function card(v: View): string {
  const windows = (v.quota?.windows ?? [])
    .map(
      (w) => `
      <div class="window">
        <div class="window-head"><span>${w.label}</span><span>${Math.round(100 - w.used_percent)}% left · ${countdown(w.resets_at)}</span></div>
        <div class="bar"><div class="bar-fill health-${v.health}" style="width:${Math.min(w.used_percent, 100)}%"></div></div>
      </div>`
    )
    .join("");
  const today = v.usage?.days.at(-1);
  const cost = today?.est_cost_usd != null ? `$${today.est_cost_usd.toFixed(2)} est.` : "—";
  const usageBlock =
    v.usage && today
      ? `<div class="usage">
           <div class="usage-today">Today: ${fmtTokens(today.input_tokens)} in · ${fmtTokens(today.output_tokens)} out · ${cost}</div>
           ${sparkline(v.usage.days)}
         </div>`
      : "";
  const status = v.error_kind ? `<div class="status error">${errorCopy(v)}</div>` : "";
  return `
    <section class="card health-${v.health}" data-kind="${v.kind}">
      <header>
        <h2>${NAMES[v.kind]}</h2>
        <span class="plan">${v.quota?.plan ?? ""}</span>
      </header>
      ${status}${windows}${usageBlock}
    </section>`;
}

function render(views: View[]) {
  current = views;
  const updated = views.find((v) => v.updated_at)?.updated_at ?? null;
  document.querySelector<HTMLElement>("#app")!.innerHTML =
    views.map(card).join("") + `<footer>updated ${age(updated) || "—"}</footer>`;
}

listen<View[]>("state", (e) => render(e.payload));
invoke<View[]>("panel_opened").then(render);
setInterval(() => current.length && render(current), 30_000);
