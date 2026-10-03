import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { NowPlayingWindow } from "./components/NowPlayingWindow";
import "./index.css"; // <-- Esta linha é obrigatória para carregar o Tailwind!
import "./i18n";

const isNowPlayingWindow =
  new URLSearchParams(window.location.search).get("window") === "now-playing";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isNowPlayingWindow ? <NowPlayingWindow /> : <App />}
  </React.StrictMode>
);
