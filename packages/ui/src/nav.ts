import { readonly, ref } from "vue";

export const tabs = ["connections", "state", "actions", "journal"] as const;
export type Tab = (typeof tabs)[number];

export interface Route {
  tab: Tab;
  /** `#/state/vh.outputs`: the state path to show. */
  path?: string;
  /** `#/journal/42`: the journal entry whose `why` chain is open. */
  seq?: number;
}

/**
 * Screens live in the hash (`#/state`), so UI routes never shadow the HTTP API
 * at `/state`, `/journal` and so on, and the server's SPA fallback stays simple.
 */
export function parseHash(hash: string): Route {
  const body = hash.replace(/^#\/?/, "");
  const slash = body.indexOf("/");
  const head = slash === -1 ? body : body.slice(0, slash);
  const rest = slash === -1 ? "" : safeDecode(body.slice(slash + 1));
  const tab = tabs.find((t) => t === head) ?? "connections";
  if (tab === "state" && rest !== "") return { tab, path: rest };
  if (tab === "journal" && /^[1-9]\d*$/.test(rest)) return { tab, seq: Number(rest) };
  return { tab };
}

function safeDecode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

export function hrefOf(route: Route): string {
  if (route.tab === "state" && route.path) return `#/state/${encodeURIComponent(route.path)}`;
  if (route.tab === "journal" && route.seq !== undefined) return `#/journal/${route.seq}`;
  return `#/${route.tab}`;
}

const current = ref<Route>(parseHash(location.hash));
addEventListener("hashchange", () => (current.value = parseHash(location.hash)));

export function useRoute() {
  return readonly(current);
}

export function navigate(route: Route) {
  location.hash = hrefOf(route);
}
