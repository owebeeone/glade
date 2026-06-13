import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: { dedupe: ["react", "react-dom"] },
  // grip-react is consumed as built ESM; don't pre-bundle it
  optimizeDeps: { exclude: ["@owebeeone/grip-react"] },
  // the glade client/grip-share sources + taut corpus live outside this dir
  server: { fs: { strict: false } },
});
