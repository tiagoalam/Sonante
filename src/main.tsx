import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css"; // <-- Esta linha é obrigatória para carregar o Tailwind!

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
