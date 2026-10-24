import react from "@vitejs/plugin-react";
import { loadEnv } from "vite";
import { configDefaults, defineConfig } from "vitest/config";

import pkg from "./package.json";

export default defineConfig(({ mode }) => {
  const fileEnv = loadEnv(mode, process.cwd(), "BRAND_");
  const serviceName = (
    process.env.BRAND_SERVICE_NAME ??
    fileEnv.BRAND_SERVICE_NAME ??
    ""
  ).trim();

  return {
    plugins: [react()],
    define: {
      "import.meta.env.VITE_APP_VERSION": JSON.stringify(pkg.version),
      "import.meta.env.VITE_BRAND_SERVICE_NAME": JSON.stringify(serviceName),
    },
    clearScreen: false,
    test: {
      exclude: [...configDefaults.exclude, ".claude/**"],
      css: true,
      environment: "jsdom",
      environmentOptions: {
        jsdom: {
          url: "http://localhost/",
        },
      },
      setupFiles: ["./vitest.setup.ts"],
      globals: false,
    },
    server: {
      host: "127.0.0.1",
      port: 1420,
      strictPort: true,
    },
  };
});
