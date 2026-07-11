// The tab chassis (GLP-0006 P0.S5b). Adding a tab is ONE entry in `TABS` — a
// (label, component) pair; P1's chat and gwz supplier tabs land here as single
// additions. The selected tab is grip-style shared state (CURRENT_TAB, an atom
// tap in taps.ts) — no React state hook, so the selection is a first-class grip
// value the whole app can read.

import type { ComponentType } from "react";
import { useGrip } from "@owebeeone/grip-react";
import { CURRENT_TAB, CURRENT_TAB_TAP } from "./grips";
import { WorkspacePanel } from "./WorkspacePanel";
import { ChatPanel } from "./ChatPanel";

/** A registered tab: a stable id, a bar label, and the panel to render. */
export interface TabDef {
  readonly id: string;
  readonly label: string;
  readonly component: ComponentType;
}

/** Placeholder tab — proves the chassis; P1 replaces it with real supplier tabs. */
function AboutPanel() {
  return (
    <div className="panel">
      <header>
        <h1>About</h1>
      </header>
      <section>
        <div className="current">
          This is the gryth workspace demo on the audited glade substrate.
        </div>
        <div className="current" style={{ marginTop: "0.8rem" }}>
          The tabs above are the chassis: adding one is a single entry in{" "}
          <code>tabs.tsx</code> <code>TABS</code> — a label plus a component. The
          GLP-0006 supplier tabs (chat, gwz, files, terminal) each land here as
          one-line additions.
        </div>
      </section>
    </div>
  );
}

/** The tab registry — the ONLY place a tab is declared. */
export const TABS: readonly TabDef[] = [
  { id: "workspace", label: "Workspace", component: WorkspacePanel },
  { id: "chat", label: "Chat", component: ChatPanel },
  { id: "about", label: "About", component: AboutPanel },
];

/** The default (first) tab — kept in sync with CURRENT_TAB's default. */
export const DEFAULT_TAB = TABS[0]!.id;

/** The tab bar: a button per registered tab, driving CURRENT_TAB. */
function TabBar() {
  const tab = useGrip(CURRENT_TAB) ?? DEFAULT_TAB;
  const tabTap = useGrip(CURRENT_TAB_TAP);
  return (
    <nav className="tabs">
      {TABS.map((t) => (
        <button
          key={t.id}
          className={t.id === tab ? "tab active" : "tab"}
          onClick={() => tabTap?.set(t.id)}
          disabled={t.id === tab}
        >
          {t.label}
        </button>
      ))}
    </nav>
  );
}

/** The chassis: the tab bar plus the active tab's panel. */
export function Tabs() {
  const tab = useGrip(CURRENT_TAB) ?? DEFAULT_TAB;
  const Active = (TABS.find((t) => t.id === tab) ?? TABS[0]!).component;
  return (
    <>
      <TabBar />
      <Active />
    </>
  );
}
