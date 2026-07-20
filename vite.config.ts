import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "chrome120", // WebView2 evergreen
    rolldownOptions: {
      input: {
        overlay: resolve(__dirname, "overlay.html"),
        settings: resolve(__dirname, "settings.html"),
        history: resolve(__dirname, "history.html"),
      },
    },
  },
});
