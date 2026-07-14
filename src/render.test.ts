import { describe, expect, test } from "vitest";
import { card, esc, type View } from "./render";

// Every field an attacker-influenced API response could reach: quota.plan,
// window labels, and raw error strings. The fixtures below push script
// payloads through each and assert card() emits only escaped text.

function view(overrides: Partial<View>): View {
  return {
    kind: "claude",
    health: "green",
    remaining_percent: 80,
    quota: null,
    error: null,
    error_kind: null,
    usage: null,
    updated_at: null,
    enabled: true,
    ...overrides,
  };
}

describe("esc", () => {
  test("escapes all five HTML metacharacters", () => {
    expect(esc(`&<>"'`)).toBe("&amp;&lt;&gt;&quot;&#39;");
  });

  test("leaves benign text unchanged", () => {
    expect(esc("Max 20x")).toBe("Max 20x");
  });
});

describe("card escapes API-derived strings", () => {
  test("plan with an img onerror payload renders as text, not markup", () => {
    const html = card(
      view({
        quota: {
          plan: "<img src=x onerror=alert(1)>",
          windows: [],
          fetched_at: "2026-07-14T00:00:00Z",
        },
      }),
    );
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });

  test("window label with quotes and angle brackets is escaped", () => {
    const html = card(
      view({
        quota: {
          plan: null,
          windows: [
            {
              label: '"><script>alert(1)</script>',
              used_percent: 3,
              resets_at: null,
              exhaust_eta: null,
            },
          ],
          fetched_at: "2026-07-14T00:00:00Z",
        },
      }),
    );
    expect(html).not.toContain("<script");
    expect(html).toContain("&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;");
  });

  test("raw provider error string is escaped", () => {
    // An unknown error_kind falls through errorCopy() to the raw v.error
    // string — the only card path where an arbitrary API string is shown.
    const html = card(
      view({
        error_kind: "unexpected",
        error: "<svg onload=alert(1)>",
      }),
    );
    expect(html).not.toContain("<svg onload");
    expect(html).toContain("&lt;svg onload=alert(1)&gt;");
  });

  test("benign plan string survives verbatim", () => {
    const html = card(
      view({
        quota: { plan: "Max 20x", windows: [], fetched_at: "2026-07-14T00:00:00Z" },
      }),
    );
    expect(html).toContain('<span class="plan">Max 20x</span>');
  });
});
