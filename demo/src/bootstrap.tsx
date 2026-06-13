import "./index.css";
import ReactDOM from "react-dom/client";
import { GripProvider } from "@owebeeone/grip-react";
import { grok, main } from "./runtime";
import { registerAllTaps } from "./taps";
import { startGladeSync } from "./glade";
import App from "./App";

// 1. register the share-declared taps, 2. start syncing to the local node,
// 3. mount the UI (dumb projections over the grips).
registerAllTaps();

const NODE_URL = "ws://127.0.0.1:9099";
startGladeSync(NODE_URL).catch((e) => console.error("glade sync:", (e as Error).message));

ReactDOM.createRoot(document.getElementById("root")!).render(
  <GripProvider grok={grok} context={main}>
    <App />
  </GripProvider>,
);
