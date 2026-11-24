import { LocalDiagnostics } from './LocalDiagnostics';
import React, { useEffect, useId, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { X, Sparkles, Cpu, HardDrive, Server, ShieldCheck, Heart, ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";
import { KeyboardShortcutsHelp } from "./KeyboardShortcutsHelp";

type AboutTab = "about" | "shortcuts";
interface Props {
  onClose: () => void;
  initialTab?: AboutTab;
}

export const AboutModal: React.FC<Props> = ({ onClose, initialTab = "about" }) => {
  const { t } = useTranslation();
  const [channelError, setChannelError] = useState(false);
  const [sponsorError, setSponsorError] = useState(false);
  const [activeTab, setActiveTab] = useState<AboutTab>(initialTab);
  const id = useId();
  const aboutTabId = id + "-about-tab";
  const shortcutsTabId = id + "-shortcuts-tab";
  const aboutPanelId = id + "-about-panel";
  const shortcutsPanelId = id + "-shortcuts-panel";
  const aboutTabRef = useRef<HTMLButtonElement>(null);
  const shortcutsTabRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const previousFocus = document.activeElement;
    returnFocusRef.current = previousFocus instanceof HTMLElement ? previousFocus : null;
    (initialTab === "shortcuts" ? shortcutsTabRef : aboutTabRef).current?.focus({ preventScroll: true });
    return () => {
      const target = returnFocusRef.current;
      if (target?.isConnected) target.focus({ preventScroll: true });
    };
  }, []);

  const handleTabKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const tabs: AboutTab[] = ["about", "shortcuts"];
    let nextTab: AboutTab | undefined;
    if (event.key === "ArrowRight") nextTab = tabs[(tabs.indexOf(activeTab) + 1) % tabs.length];
    else if (event.key === "ArrowLeft") nextTab = tabs[(tabs.indexOf(activeTab) + tabs.length - 1) % tabs.length];
    else if (event.key === "Home") nextTab = tabs[0];
    else if (event.key === "End") nextTab = tabs[tabs.length - 1];
    if (!nextTab) return;
    event.preventDefault();
    event.stopPropagation();
    setActiveTab(nextTab);
    (nextTab === "shortcuts" ? shortcutsTabRef : aboutTabRef).current?.focus({ preventScroll: true });
  };

  const handleDialogKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;

    const dialog = event.currentTarget;
    const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(
      'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter(element => !element.closest("[hidden], [inert]"));
    if (focusable.length === 0) {
      event.preventDefault();
      dialog.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const focusIsInside = dialog.contains(document.activeElement);
    if (event.shiftKey && (document.activeElement === first || !focusIsInside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !focusIsInside)) {
      event.preventDefault();
      first.focus();
    }
  };

  const openChannel = async (url: string) => {
    setChannelError(false);
    try { await openUrl(url); } catch { setChannelError(true); }
  };
  const openSponsor = async () => {
    setSponsorError(false);
    try { await openUrl("https://github.com/sponsors/tiagoalam"); } catch { setSponsorError(true); }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-xs select-none p-4">
      <div role="dialog" aria-modal="true" aria-label={t("sidebar.about")} tabIndex={-1} onKeyDown={handleDialogKeyDown}
        className="bg-[var(--theme-surface-panel,#181818)] border border-[color:var(--theme-border-soft,#2B2B2B)] rounded-2xl w-full max-w-lg max-h-[90vh] flex flex-col shadow-2xl overflow-hidden animate-in fade-in duration-150">
        {/* Cabeçalho */}
        <div className="shrink-0 p-6 border-b border-[color:var(--theme-border-soft,#262626)] flex items-center justify-between bg-[var(--theme-surface-panel,#1D1D1D)]">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-[#E5A00D]/10 border border-[color:var(--theme-border-accent,#E5A00D)]/30 flex items-center justify-center text-[color:var(--theme-fg-accent,#E5A00D)]">
              <Sparkles size={22} />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h2 className="text-lg font-black tracking-wider text-[color:var(--theme-fg-accent,#E5A00D)]">SONANTE</h2>
                <span className="text-[10px] font-mono font-bold bg-[#E5A00D] text-black px-1.5 py-0.5 rounded-sm">
                  {t("about.version", { version: "0.7.2" })}
                </span>
              </div>
              <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)]">{t("about.tagline")}</p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("about.close")}
            className="p-1.5 text-[color:var(--theme-fg-muted,#888888)] hover:text-[color:var(--theme-fg-primary,#ffffff)] rounded-lg hover:bg-[var(--theme-surface-raised,#252525)] transition-colors cursor-pointer focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--theme-fg-accent,#E5A00D)]"
          >
            <X size={18} />
          </button>
        </div>

        <div role="tablist" aria-label={t("about.tabsLabel")} aria-orientation="horizontal" onKeyDown={handleTabKeyDown}
          className="flex shrink-0 gap-1 overflow-x-auto border-b border-[color:var(--theme-border-soft,#262626)] px-4">
          <button ref={aboutTabRef} type="button" role="tab" id={aboutTabId} aria-controls={aboutPanelId}
            aria-selected={activeTab === "about"} tabIndex={activeTab === "about" ? 0 : -1}
            onClick={() => setActiveTab("about")}
            className={activeTab === "about"
              ? "shrink-0 whitespace-nowrap border-b-2 border-[color:var(--theme-border-accent,#E5A00D)] px-3 py-3 text-sm font-semibold text-[color:var(--theme-fg-accent,#E5A00D)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
              : "shrink-0 whitespace-nowrap border-b-2 border-transparent px-3 py-3 text-sm font-medium text-[color:var(--theme-fg-secondary,#bbbbbb)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"}>
            {t("about.tabAbout")}
          </button>
          <button ref={shortcutsTabRef} type="button" role="tab" id={shortcutsTabId} aria-controls={shortcutsPanelId}
            aria-selected={activeTab === "shortcuts"} tabIndex={activeTab === "shortcuts" ? 0 : -1}
            onClick={() => setActiveTab("shortcuts")}
            className={activeTab === "shortcuts"
              ? "shrink-0 whitespace-nowrap border-b-2 border-[color:var(--theme-border-accent,#E5A00D)] px-3 py-3 text-sm font-semibold text-[color:var(--theme-fg-accent,#E5A00D)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"
              : "shrink-0 whitespace-nowrap border-b-2 border-transparent px-3 py-3 text-sm font-medium text-[color:var(--theme-fg-secondary,#bbbbbb)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]"}>
            {t("about.tabShortcuts")}
          </button>
        </div>

        <div className="flex min-h-0 flex-1 overflow-hidden">
        <section id={aboutPanelId} role="tabpanel" aria-labelledby={aboutTabId} tabIndex={activeTab === "about" ? 0 : -1}
          hidden={activeTab !== "about"}
          className={activeTab === "about" ? "min-h-0 flex-1 overflow-y-auto p-6 space-y-4 text-xs" : "hidden"}>
          <div className="space-y-3">
            <h3 className="text-[11px] font-bold text-[color:var(--theme-fg-secondary,#CCCCCC)] uppercase tracking-wider">
              {t("about.techTitle")}
            </h3>

            <div className="grid grid-cols-1 gap-2.5">
              <div className="p-3 rounded-xl bg-[var(--theme-surface-base,#131313)] border border-[color:var(--theme-border-soft,#222222)] flex items-start space-x-3">
                <ShieldCheck size={18} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)] block">{t("about.audioEngineTitle")}</span>
                  <span className="text-[color:var(--theme-fg-muted,#888888)] text-[11px] leading-relaxed">
                    {t("about.audioEngineDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[var(--theme-surface-base,#131313)] border border-[color:var(--theme-border-soft,#222222)] flex items-start space-x-3">
                <HardDrive size={18} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)] block">{t("about.localTitle")}</span>
                  <span className="text-[color:var(--theme-fg-muted,#888888)] text-[11px] leading-relaxed">
                    {t("about.localDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[var(--theme-surface-base,#131313)] border border-[color:var(--theme-border-soft,#222222)] flex items-start space-x-3">
                <Server size={18} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)] block">{t("about.plexTitle")}</span>
                  <span className="text-[color:var(--theme-fg-muted,#888888)] text-[11px] leading-relaxed">
                    {t("about.plexDesc")}
                  </span>
                </div>
              </div>

              <div className="p-3 rounded-xl bg-[var(--theme-surface-base,#131313)] border border-[color:var(--theme-border-soft,#222222)] flex items-start space-x-3">
                <Cpu size={18} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0 mt-0.5" />
                <div>
                  <span className="font-bold text-[color:var(--theme-fg-primary,#ffffff)] block">{t("about.devTitle")}</span>
                  <span className="text-[color:var(--theme-fg-muted,#888888)] text-[11px] leading-relaxed">
                    {t("about.devDesc")}
                  </span>
                </div>
              </div>
            </div>
          </div>
          <section className="p-4 rounded-xl bg-[var(--theme-surface-base,#131313)] border border-[color:var(--theme-border-soft,#222222)] space-y-3" aria-labelledby="about-testers-title">
            <h3 id="about-testers-title" className="flex items-center gap-2 text-[11px] font-bold text-[color:var(--theme-fg-secondary,#CCCCCC)] uppercase tracking-wider">
              <Heart size={15} className="text-[color:var(--theme-fg-accent,#E5A00D)] shrink-0" />
              {t("about.testersTitle")}
            </h3>
            <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)] leading-relaxed">{t("about.testersDescription")}</p>
            <div className="flex flex-wrap gap-2">
              {[
                { name: "Eduardo Monks", url: "https://www.youtube.com/@emmonks" },
                { name: "Digão Reviews", url: "https://www.youtube.com/channel/UCeQ57U46cZQBMVDnKM3SbEg" },
              ].map(({ name, url }) => (
                <button key={url} type="button" onClick={() => void openChannel(url)}
                  className="inline-flex items-center gap-2 rounded-lg border border-[color:var(--theme-border-soft,#2B2B2B)] px-3 py-2 text-[color:var(--theme-fg-accent,#E5A00D)] hover:bg-[var(--theme-surface-raised,#252525)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--theme-fg-accent,#E5A00D)] transition-colors cursor-pointer">
                  {name}<ExternalLink size={12} aria-hidden="true" />
                </button>
              ))}
            </div>
            {channelError && <p role="alert" className="text-[11px] text-red-300">{t("about.channelOpenError")}</p>}
          </section>
          <LocalDiagnostics />
          <p className="text-[11px] text-[color:var(--theme-fg-muted,#888888)] leading-relaxed">
            {t("about.license", { license: "GPL-3.0-only" })}
          </p>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pt-1">
            <button type="button" onClick={() => void openSponsor()} aria-label={t("about.supportDevelopment")}
              className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-[11px] text-[color:var(--theme-fg-muted,#888888)] hover:bg-[var(--theme-surface-raised,#252525)] hover:text-[color:var(--theme-fg-primary,#ffffff)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--theme-fg-accent,#E5A00D)] transition-colors cursor-pointer">
              <Heart size={13} aria-hidden="true" />
              <span>{t("about.supportDevelopment")}</span>
              <ExternalLink size={11} aria-hidden="true" />
            </button>
            {sponsorError && <p role="alert" className="text-[11px] text-[color:var(--theme-fg-muted,#888888)]">{t("about.sponsorOpenError")}</p>}
          </div>
        </section>
        <section id={shortcutsPanelId} role="tabpanel" aria-labelledby={shortcutsTabId} tabIndex={activeTab === "shortcuts" ? 0 : -1}
          hidden={activeTab !== "shortcuts"}
          className={activeTab === "shortcuts" ? "min-h-0 flex-1 overflow-y-auto p-6" : "hidden"}>
          <KeyboardShortcutsHelp />
        </section>
        </div>

        {/* Rodapé */}
        <div className="shrink-0 px-6 py-4 border-t border-[color:var(--theme-border-soft,#262626)] bg-[var(--theme-surface-base,#161616)] flex items-center justify-between text-[11px] text-[color:var(--theme-fg-muted,#666666)]">
          <div className="flex items-center space-x-1">
            <span>{t("about.builtWith")}</span>
            <Heart size={12} className="text-[color:var(--theme-fg-accent,#E5A00D)] fill-[#E5A00D]" />
            <span>{t("about.forEnthusiasts")}</span>
          </div>
          <span className="font-mono">Linux • MPD • ALSA</span>
        </div>
      </div>
    </div>
  );
};

export default AboutModal;
