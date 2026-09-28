import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import ptBR from "./locales/pt-BR.json";

// Detecta o idioma padrão salvo no localStorage ou utiliza o do sistema operacional
const savedLanguage = localStorage.getItem("sonante_lang");
const systemLanguage = navigator.language.startsWith("pt") ? "pt-BR" : "en";
const defaultLanguage = savedLanguage || systemLanguage;

i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    "pt-BR": { translation: ptBR },
  },
  lng: defaultLanguage,
  fallbackLng: "en",
  interpolation: {
    escapeValue: false, // React já protege contra XSS
  },
});

export default i18n;
