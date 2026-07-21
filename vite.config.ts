import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Never watch the Rust build output — vite's watcher dies with EBUSY on
    // the locked dictum_lib.dll under Windows.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome120", // WebView2 evergreen
    rolldownOptions: {
      input: {
        overlay: resolve(__dirname, "overlay.html"),
        main: resolve(__dirname, "main.html"),
      },
    },
  },
});
