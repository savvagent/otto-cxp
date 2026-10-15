import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";
import path from "path";
import { SERVER_ONLY_PATHS } from "./src/pwa";

export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      registerType: "autoUpdate",
      workbox: {
        // Client-side routes get the cached app shell; the server's own paths
        // (the sign-in round trip above all) always go to the network.
        navigateFallback: "index.html",
        navigateFallbackDenylist: SERVER_ONLY_PATHS,
      },
      manifest: {
        name: "CXP - Customer Experience Platform",
        short_name: "CXP",
        description: "Customer Experience Platform",
        theme_color: "#0f172a",
        background_color: "#0f172a",
        display: "standalone",
        start_url: "/",
        icons: [
          {
            src: "/icon-192.png",
            sizes: "192x192",
            type: "image/png",
          },
          {
            src: "/icon-512.png",
            sizes: "512x512",
            type: "image/png",
          },
        ],
      },
    }),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    // Everything the server answers itself, so `pnpm dev:web` signs in and
    // calls the API on the dev server's origin, as the built console does.
    proxy: {
      "/api": "http://localhost:8080",
      "/auth": "http://localhost:8080",
      "/mcp": "http://localhost:8080",
      "/.well-known": "http://localhost:8080",
    },
  },
});
