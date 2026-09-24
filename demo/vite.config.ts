import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The port of the glade-node the page connects to: GLADE_NODE_PORT, which
// run_demo.py sets to the port its node reports, or 9099.
const nodePort = process.env.GLADE_NODE_PORT ?? "9099";
if (!/^[0-9]+$/.test(nodePort)) {
  throw new Error(`GLADE_NODE_PORT must be a port number, not ${JSON.stringify(nodePort)}`);
}

export default defineConfig({
  define: { __GLADE_NODE_PORT__: JSON.stringify(nodePort) },
  plugins: [react()],
  resolve: { dedupe: ["react", "react-dom"] },
  // grip-react is consumed as built ESM; don't pre-bundle it
  optimizeDeps: { exclude: ["@owebeeone/grip-react"] },
  // the glade client/grip-share sources + taut corpus live outside this dir
  server: { fs: { strict: false } },
});
