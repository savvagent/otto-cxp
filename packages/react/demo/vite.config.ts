import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig({
  root: __dirname,
  plugins: [react()],
  resolve: {
    alias: {
      "@cxp/react": path.resolve(__dirname, "../src"),
      "@cxp/sdk": path.resolve(__dirname, "../../../sdk/typescript/src"),
    },
  },
  server: {
    proxy: {
      "/mcp": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
    },
  },
});
