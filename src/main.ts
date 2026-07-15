import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  age,
  canRefreshSignin,
  card,
  failedRefresh,
  pendingRefresh,
  updateNotice,
  type UpdateInfo,
  type UpdatePhase,
  type View,
} from "./render";

let current: View[] = [];
let updateInfo: UpdateInfo | null = null;
let updatePhase: UpdatePhase = "idle";
let updateError: string | undefined;

// Under `require-trusted-types-for 'script'` (see the CSP in
// src-tauri/tauri.conf.json) the webview rejects plain-string assignment to
// injection sinks like innerHTML. This policy is deliberately pass-through —
// escaping already happened via esc() in render.ts — its value is that
// render() below becomes the page's only sanctioned HTML sink: any other
// innerHTML write throws a TypeError instead of parsing markup. WKWebView
// has no trustedTypes and ignores the directive, hence the fallback.
const htmlPolicy = window.trustedTypes?.createPolicy("panel-render", {
  createHTML: (s: string) => s,
});
// TrustedHTML isn't assignable to lib.dom's string-typed innerHTML setter,
// so the cast routes it through; the browser accepts either.
const asRenderedHtml = (s: string): string =>
  htmlPolicy ? (htmlPolicy.createHTML(s) as unknown as string) : s;

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
  document.querySelector<HTMLElement>("#app")!.innerHTML = asRenderedHtml(
    visible.map(card).join("") +
      `<footer>updated ${age(updated) || "—"}${updateNotice(updateInfo, updatePhase, updateError)}</footer>`,
  );
}

// Card markup is fully rebuilt via innerHTML on every render, so individual
// .refresh-btn elements come and go — one delegated listener on the
// never-replaced #app container handles clicks for whichever button is
// current. The pendingRefresh/failedRefresh sets (see refreshButtonHtml)
// carry the button state across re-renders; the direct DOM mutations here
// just give instant feedback before the next render.
document.querySelector<HTMLElement>("#app")!.addEventListener("click", (e) => {
  const notes = (e.target as HTMLElement).closest<HTMLButtonElement>(".update-notes");
  if (notes) {
    invoke("open_release_notes").catch((err) => console.error("open_release_notes failed", err));
    return;
  }
  const upd = (e.target as HTMLElement).closest<HTMLButtonElement>(".update-btn");
  if (upd && !upd.disabled && updatePhase !== "busy") {
    updatePhase = "busy";
    if (current.length) render(current);
    invoke("apply_update").catch((err) => {
      updatePhase = "error";
      updateError = String(err);
      if (current.length) render(current);
    });
    return;
  }
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
listen<UpdateInfo | null>("update", (e) => {
  updateInfo = e.payload;
  updatePhase = "idle";
  updateError = undefined;
  if (current.length) render(current);
});
invoke<UpdateInfo | null>("update_status")
  .then((u) => {
    updateInfo = u;
    if (current.length) render(current);
  })
  .catch(() => {});
listen("panel-shown", () => {
  invoke<View[]>("panel_opened").then(render).catch((e) => console.error("panel_opened failed", e));
});
invoke<View[]>("panel_opened").then(render).catch((e) => console.error("panel_opened failed", e));
setInterval(() => current.length && render(current), 30_000);
