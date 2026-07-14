// TypeScript 5.6's lib.dom has no Trusted Types definitions, so declare the
// minimal surface main.ts uses. `trustedTypes` is optional on Window because
// only Chromium engines (WebView2) implement it — WKWebView on macOS leaves
// it undefined and ignores the CSP directive.
interface TrustedHTML {
  toString(): string;
}

interface TrustedTypePolicy {
  createHTML(input: string): TrustedHTML;
}

interface TrustedTypePolicyFactory {
  createPolicy(
    name: string,
    rules: { createHTML?: (input: string) => string },
  ): TrustedTypePolicy;
}

interface Window {
  readonly trustedTypes?: TrustedTypePolicyFactory;
}
