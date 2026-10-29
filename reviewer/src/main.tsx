import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { AuthProvider } from "./components/Account";
import "./styles/style.css";

// Personal access tokens pasted before sign-in replaced them. Remove in a
// later release.
localStorage.removeItem("deltoids.gh.token");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AuthProvider>
      <App />
    </AuthProvider>
  </StrictMode>,
);
