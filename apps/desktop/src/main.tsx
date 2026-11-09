import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";
import "./tray.css";
import { useHoard } from "./store";

// Dev-only hook for driving the UI from automation (CDP). Stripped from
// production builds because import.meta.env.DEV is statically false there.
if (import.meta.env.DEV) {
  (window as unknown as { hoard: typeof useHoard }).hoard = useHoard;
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
