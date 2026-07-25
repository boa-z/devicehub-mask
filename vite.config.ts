import { defineConfig } from "vite";
import react from "@vitejs/plugin-react-swc";

export default defineConfig({
  plugins: [react()],
  build: {
    manifest: true,
  },
  server: {
    host: "127.0.0.1",
    port: 5173,
  },
});
