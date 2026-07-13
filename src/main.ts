import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type RateWindow = {
  label: string;
  used_percent: number;
  resets_at: string | null;
  exhaust_eta: string | null;
};
type Quota = { plan: string | null; windows: RateWindow[]; fetched_at: string };
type DayUsage = {
  date: string;
  input_tokens: number;
  output_tokens: number;
  est_cost_usd: number | null;
};
type View = {
  kind: "claude" | "codex" | "grok" | "deepseek";
  health: "green" | "amber" | "red" | "unavailable";
  remaining_percent: number | null;
  quota: Quota | null;
  error: string | null;
  error_kind: string | null;
  usage: { days: DayUsage[] } | null;
  updated_at: string | null;
  // false means the provider is disabled in config — the shell's tray icon
  // and this panel both omit it entirely rather than showing an empty/gray
  // placeholder card.
  enabled: boolean;
};

const NAMES: Record<View["kind"], string> = {
  claude: "Claude",
  codex: "Codex",
  grok: "Grok",
  deepseek: "DeepSeek",
};
const CLI: Record<View["kind"], string> = {
  claude: "Claude Code",
  codex: "Codex",
  grok: "Grok",
  deepseek: "DeepSeek",
};

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

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

// `exhaustEta` is an ISO date string from our own backend (see
// RateWindow.exhaust_eta in model.rs) — parsed via `new Date()` and only the
// resulting numeric hour/minute digits are interpolated, so no HTML-escaping
// is needed here.
function etaLabel(exhaustEta: string | null): string {
  if (!exhaustEta) return "";
  const d = new Date(exhaustEta);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `runs out ~${hh}:${mm}`;
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
      // DeepSeek has no local sign-in CLI to "open" — its auth is a static
      // API key, so the expired-token hint points at the env var instead.
      return v.kind === "deepseek"
        ? "API key invalid — check DEEPSEEK_API_KEY"
        : `Sign-in expired — open ${CLI[v.kind]} to refresh`;
    case "network":
      return `Offline — retrying (last data ${age(v.updated_at) || "never"})`;
    case "schema_changed":
      return "Endpoint changed — needs an update";
    default:
      return v.error ?? "";
  }
}

const WEEKDAY_INITIALS = ["S", "M", "T", "W", "T", "F", "S"];
const SHORT_MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

// Backend day labels are plain "YYYY-MM-DD" local-day strings (see
// DayUsage.date) — split explicitly and built via Date.UTC rather than
// `new Date(str)` so weekday/label derivation can't shift across midnight
// depending on the host machine's timezone offset.
function parseDayUsage(dateStr: string): Date {
  const [y, m, d] = dateStr.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d));
}

function weekdayInitial(dateStr: string): string {
  return WEEKDAY_INITIALS[parseDayUsage(dateStr).getUTCDay()];
}

function fmtDayLabel(dateStr: string): string {
  const d = parseDayUsage(dateStr);
  return `${SHORT_MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}`;
}

function fmtCost(usd: number | null): string {
  return usd != null ? `$${usd.toFixed(2)} est.` : "—";
}

// "Jul 10 · 187.9M in · 145.5K out · $88.45 est." — every part is a number
// or date reformatted from backend fields, so (like etaLabel above) no
// esc() is needed: no raw API strings ever reach this string.
function dayTooltip(d: DayUsage): string {
  return `${fmtDayLabel(d.date)} · ${fmtTokens(d.input_tokens)} in · ${fmtTokens(d.output_tokens)} out · ${fmtCost(d.est_cost_usd)}`;
}

// 7-day usage capsule chart: bar height is proportional to (input + output)
// tokens normalized against the week's max, rendered as pill-shaped
// capsules (rx = min(barW/2, barH/2), so short bars read as coins and tall
// bars read as full stadiums). All bars use the provider's identity accent
// at a fixed opacity ladder — 0.45 for a normal day, 0.15 for a zero-usage
// day's 4px stub, 1.0 for today (always the last slot) — so today reads as
// the visual anchor of the row without needing a different hue. Today also
// gets a small value label (fmtTokens of its input+output total) in a fixed
// headroom row above the tallest possible bar, so it never collides with
// any day's capsule regardless of which day is actually tallest. Each bar
// still carries a native <title> tooltip for hover detail, and weekday
// initials sit in their own row below the baseline.
function usageChart(days: DayUsage[], kind: View["kind"]): string {
  const width = 316;
  const height = 74;
  const baseline = 54;
  const maxBarH = 40;
  const minBarH = 8;
  const zeroBarH = 4;
  const totals = days.map((d) => d.input_tokens + d.output_tokens);
  const weekMax = Math.max(...totals, 1);
  const n = days.length;
  const slot = width / n;
  // ~16px at the standard 7-slot/316-wide layout; clamped so a different
  // day count (not expected in practice, but kept generic) can't overlap
  // neighboring slots.
  const barW = Math.min(16, slot - 6);

  const bars = days
    .map((d, i) => {
      const total = totals[i];
      const isZero = total === 0;
      const isToday = i === n - 1;
      const barH = isZero ? zeroBarH : Math.max(minBarH, (total / weekMax) * maxBarH);
      const rx = Math.min(barW / 2, barH / 2);
      const x = i * slot + (slot - barW) / 2;
      const y = baseline - barH;
      const cx = x + barW / 2;
      const barClass = isToday
        ? "chart-bar chart-bar-today"
        : isZero
          ? "chart-bar chart-bar-zero"
          : "chart-bar";
      const dayClass = isToday ? "chart-day chart-day-today" : "chart-day";
      // Today's value label is horizontally centered on its capsule by
      // default; if that would push the label past the viewBox's right
      // edge, it anchors to the right edge instead of centering.
      const label = isToday
        ? (() => {
            const anchorEnd = cx > width - 18;
            const labelX = anchorEnd ? width - 2 : cx;
            const anchor = anchorEnd ? "end" : "middle";
            return `<text class="chart-value" x="${labelX.toFixed(1)}" y="8" text-anchor="${anchor}">${fmtTokens(total)}</text>`;
          })()
        : "";
      return `<g><title>${dayTooltip(d)}</title><rect class="${barClass}" x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${barW.toFixed(1)}" height="${barH.toFixed(1)}" rx="${rx.toFixed(1)}"></rect>${label}<text class="${dayClass}" x="${cx.toFixed(1)}" y="${baseline + 12}">${weekdayInitial(d.date)}</text></g>`;
    })
    .join("");

  return `<svg class="chart" viewBox="0 0 ${width} ${height}" preserveAspectRatio="none" role="img" aria-label="${NAMES[kind]} 7-day usage">${bars}</svg>`;
}

// Ring gauge: a small donut showing the provider's binding remaining-percent.
// `pct` is null when the provider is unavailable (no reading) — the ring then
// renders as an empty gray track with a "—" label instead of a number.
function ring(pct: number | null, health: View["health"]): string {
  const size = 56;
  const stroke = 5;
  const r = (size - stroke) / 2;
  const circumference = 2 * Math.PI * r;
  const known = pct != null;
  const clamped = known ? Math.max(0, Math.min(100, pct)) : 0;
  // The stroke arc is floored at a small minimum so 0% remaining still shows
  // a visible sliver of its health color instead of vanishing entirely; the
  // label below keeps showing the true rounded value, unaffected by this.
  const arcPct = known ? Math.max(clamped, 2.5) : 0;
  const offset = known ? circumference * (1 - arcPct / 100) : circumference;
  const label = known ? String(Math.round(clamped)) : "—";
  return `
    <div class="ring health-${health}">
      <svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}" aria-hidden="true">
        <circle class="ring-track" cx="${size / 2}" cy="${size / 2}" r="${r}"></circle>
        <circle class="ring-fill" cx="${size / 2}" cy="${size / 2}" r="${r}"
          stroke-dasharray="${circumference.toFixed(2)}"
          stroke-dashoffset="${offset.toFixed(2)}"></circle>
      </svg>
      <span class="ring-num">${label}</span>
    </div>`;
}

// One compact line per rate window: "5h · 97% left · resets in 2h 14m".
// Parts are filtered so a window without a resets_at still reads cleanly.
function windowLine(w: RateWindow): string {
  const remaining = Math.round(100 - w.used_percent);
  const parts = [
    esc(w.label),
    `${remaining}% left`,
    countdown(w.resets_at),
    etaLabel(w.exhaust_eta),
  ].filter(Boolean);
  return `<div class="win-line">${parts.join(" · ")}</div>`;
}

// Sign-in refresh state lives OUTSIDE the DOM: card markup is rebuilt via
// innerHTML on every render (state events, panel-shown, the 30s footer
// tick), so a disabled attribute set only on the live button node would be
// silently wiped mid-flight — re-enabling the button while the CLI is still
// running and allowing duplicate spawns. refreshButtonHtml consults these
// sets so every re-render faithfully reproduces pending/failed state until
// the invoke settles or the provider recovers.
const pendingRefresh = new Set<string>();
const failedRefresh = new Set<string>();

// Only Claude and Grok have a local CLI that can refresh an expired token by
// re-running with a trivial prompt (see refresh_signin in commands.rs).
// Codex has no local refresh CLI, so its expired-token error stays text-only.
function canRefreshSignin(v: View): boolean {
  return v.error_kind === "token_expired" && (v.kind === "claude" || v.kind === "grok");
}

// v.kind is a closed enum ("claude" | "codex" | "grok" | "deepseek"), so
// it's safe to inline into the data attribute without escaping.
function refreshButtonHtml(v: View): string {
  if (!canRefreshSignin(v)) return "";
  if (pendingRefresh.has(v.kind)) {
    return `<button type="button" class="refresh-btn" data-kind="${v.kind}" disabled>Refreshing…</button>`;
  }
  const label = failedRefresh.has(v.kind)
    ? "Refresh failed — run the CLI manually"
    : "Refresh sign-in";
  return `<button type="button" class="refresh-btn" data-kind="${v.kind}">${label}</button>`;
}

function card(v: View): string {
  const windowsHtml = v.error_kind
    ? `<div class="win-line win-error"><span class="err-icon" aria-hidden="true">⚠</span>${esc(errorCopy(v))}</div>${refreshButtonHtml(v)}`
    : (v.quota?.windows ?? []).map(windowLine).join("");
  const today = v.usage?.days.at(-1);
  const bottomRow =
    v.usage && today
      ? `<div class="card-bottom">
           <span class="usage-line">Today ${fmtTokens(today.input_tokens)} in · ${fmtTokens(today.output_tokens)} out · ${fmtCost(today.est_cost_usd)}</span>
           ${usageChart(v.usage.days, v.kind)}
         </div>`
      : "";
  const plan = v.quota?.plan ? `<span class="plan">${esc(v.quota.plan)}</span>` : "";
  return `
    <section class="card health-${v.health}" data-kind="${v.kind}">
      <div class="card-top">
        ${ring(v.remaining_percent, v.health)}
        <div class="identity">
          <div class="identity-row">
            <span class="dot"></span>
            <h2>${NAMES[v.kind]}</h2>
            ${plan}
          </div>
          <div class="windows">${windowsHtml}</div>
        </div>
      </div>
      ${bottomRow}
    </section>`;
}

function render(views: View[]) {
  current = views;
  // A provider that no longer shows an expired token has no refresh button —
  // drop its stale failure flag so a future re-expiry starts clean. Pending
  // entries are left alone: they are cleared when their invoke settles.
  for (const v of views) {
    if (!canRefreshSignin(v)) failedRefresh.delete(v.kind);
  }
  // Disabled providers disappear entirely — no card, no contribution to the
  // "updated" footer timestamp — rather than showing an empty/gray card.
  const visible = views.filter((v) => v.enabled);
  const updated = visible.find((v) => v.updated_at)?.updated_at ?? null;
  document.querySelector<HTMLElement>("#app")!.innerHTML =
    visible.map(card).join("") + `<footer>updated ${age(updated) || "—"}</footer>`;
}

// Card markup is fully rebuilt via innerHTML on every render, so individual
// .refresh-btn elements come and go — one delegated listener on the
// never-replaced #app container handles clicks for whichever button is
// current. The pendingRefresh/failedRefresh sets (see refreshButtonHtml)
// carry the button state across re-renders; the direct DOM mutations here
// just give instant feedback before the next render.
document.querySelector<HTMLElement>("#app")!.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".refresh-btn");
  if (!btn || btn.disabled) return;
  const kind = btn.dataset.kind;
  if (!kind || pendingRefresh.has(kind)) return;
  pendingRefresh.add(kind);
  failedRefresh.delete(kind);
  btn.disabled = true;
  btn.textContent = "Refreshing…";
  invoke("refresh_signin", { kind })
    .then(() => {
      // Success: the backend re-poll emits a "state" event that re-renders
      // the card without the error (and thus without the button).
      pendingRefresh.delete(kind);
    })
    .catch((err) => {
      console.error("refresh_signin failed", err);
      pendingRefresh.delete(kind);
      failedRefresh.add(kind);
      if (current.length) render(current);
    });
});

listen<View[]>("state", (e) => render(e.payload));
listen("panel-shown", () => {
  invoke<View[]>("panel_opened").then(render).catch((e) => console.error("panel_opened failed", e));
});
invoke<View[]>("panel_opened").then(render).catch((e) => console.error("panel_opened failed", e));
setInterval(() => current.length && render(current), 30_000);
