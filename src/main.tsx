import { frontendStartup } from './utils/frontendStartup';
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import {appearance} from "./services/appearance";
import { NowPlayingWindow } from "./components/NowPlayingWindow";
import "./index.css"; // <-- Esta linha é obrigatória para carregar o Tailwind!
import "./i18n";

const stopAppearance=appearance.start();
window.addEventListener("pagehide",stopAppearance,{once:true});

const isNowPlayingWindow =
  new URLSearchParams(window.location.search).get("window") === "now-playing";

if (!isNowPlayingWindow) frontendStartup.mark("renderer_entry");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isNowPlayingWindow ? <NowPlayingWindow /> : <App />}
  </React.StrictMode>
);
