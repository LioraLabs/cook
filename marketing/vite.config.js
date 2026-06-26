import { defineConfig } from "vite";

export default defineConfig({
  assetsInclude: ["**/*.vert", "**/*.frag"],
  server: {
    allowedHosts: true,
  },
});
