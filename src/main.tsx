import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// The non-macOS pet fallback runs inside a transparent Tauri window.
const isPetMode =
  new URLSearchParams(window.location.search).get("mode") === "pet";

if (isPetMode) {
  document.documentElement.style.background = "transparent";
  document.body.style.background = "transparent";
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
