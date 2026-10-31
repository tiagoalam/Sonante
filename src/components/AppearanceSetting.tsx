import {useId, useSyncExternalStore} from 'react';
import {useTranslation} from 'react-i18next';
import {appearance} from '../services/appearance';
import {APPEARANCES, appearancePalettes, type Appearance} from '../utils/appearance';

/** Local, synthetic library preview. It never loads artwork or a media source. */
function AppearancePreview({mode}: {mode: Appearance}) {
  const c = appearancePalettes[mode];
  const covers = ['#466d76', '#976c69', '#776987', '#767c51', '#667d9b', '#a07845'];
  return <svg viewBox="0 0 320 192" aria-hidden="true" focusable="false" className="block w-full rounded-lg">
    <rect width="320" height="192" rx="8" fill={c.base}/>
    <path d="M8 0H76V158H0V8Q0 0 8 0" fill={c.panel}/>
    <text x="12" y="21" fill={c.gold} fontSize="12" fontWeight="700">Sonante</text>
    {[43, 61, 79, 108, 126].map((y, i) => <g key={y}>
      {i === 0 && <rect x="7" y={y - 10} width="62" height="16" rx="4" fill={c.gold} opacity=".17"/>}
      <rect x="13" y={y - 5} width="5" height="5" rx="1" fill={i === 0 ? c.gold : c.muted}/>
      <rect x="24" y={y - 4} width={i % 2 ? 30 : 37} height="3" rx="1.5" fill={i === 0 ? c.gold : c.muted}/>
    </g>)}
    <rect x="91" y="16" width="85" height="6" rx="3" fill={c.text}/>
    <rect x="91" y="29" width="48" height="3" rx="1.5" fill={c.muted}/>
    {covers.map((color, i) => {
      const x = 91 + (i % 3) * 73, y = 43 + Math.floor(i / 3) * 57;
      return <g key={color}>
        <rect x={x} y={y} width="61" height="39" rx="4" fill={color}/>
        <circle cx={x + 30} cy={y + 19} r="13" fill={c.panel} opacity=".28"/>
        <path d={`M${x} ${y + 35}L${x + 20} ${y + 16}L${x + 42} ${y + 35}Z`} fill="#ffffff" opacity=".2"/>
        <rect x={x} y={y + 44} width="43" height="3" rx="1.5" fill={c.muted}/>
      </g>;
    })}
    <path d="M0 158H320V184Q320 192 312 192H8Q0 192 0 184Z" fill={c.panel}/>
    <rect x="10" y="164" width="22" height="22" rx="3" fill={covers[0]}/>
    <rect x="39" y="167" width="44" height="3" rx="1.5" fill={c.text}/>
    <rect x="39" y="175" width="30" height="3" rx="1.5" fill={c.muted}/>
    <circle cx="162" cy="169" r="8" fill={c.gold}/>
    <path d="M160 165L166 169L160 173Z" fill={c.panel}/>
    <path d="M143 166L138 169L143 172ZM181 166L186 169L181 172Z" fill={c.text}/>
    <rect x="118" y="182" width="88" height="2" rx="1" fill={c.card}/>
    <rect x="118" y="182" width="34" height="2" rx="1" fill={c.gold}/>
    <rect x="266" y="173" width="41" height="3" rx="1.5" fill={c.muted}/>
  </svg>;
}

export function AppearanceSetting() {
  const {t} = useTranslation();
  const id = useId();
  const value = useSyncExternalStore(appearance.subscribe, appearance.getSnapshot, () => 'dark' as const);
  return <fieldset className="min-w-0 space-y-3" aria-describedby={`${id}-help`}>
    <legend className="text-[11px] font-bold text-[color:var(--theme-fg-accent,#E5A00D)] uppercase tracking-wider">{t('settings.appearance')}</legend>
    <div className="flex flex-col gap-2">
      {APPEARANCES.map(mode => <label key={mode} className="relative min-w-0 cursor-pointer">
        <input type="radio" name={id} value={mode} checked={value === mode}
          onChange={() => appearance.set(mode)} className="peer sr-only"/>
        <span className="flex items-center gap-4 rounded-xl border-2 border-[color:var(--theme-border-soft,#383838)] p-2 transition-colors peer-checked:border-[color:var(--theme-control-accent,#E5A00D)] peer-focus-visible:outline-2 peer-focus-visible:outline-offset-4 peer-focus-visible:outline-[color:var(--theme-control-accent,#E5A00D)]">
          <span className="block w-28 shrink-0 sm:w-32"><AppearancePreview mode={mode}/></span>
          <span className="flex min-w-0 flex-1 items-center justify-between gap-2 px-1 text-sm font-semibold text-[color:var(--theme-fg-primary,#ffffff)]">
            <span className="min-w-0">
              <span className="block">{t(`settings.appearance${mode[0].toUpperCase()}${mode.slice(1)}`)}</span>
              <span className="mt-1 block text-xs font-normal leading-relaxed text-[color:var(--theme-fg-muted,#888888)]">{t(`settings.appearance${mode[0].toUpperCase()}${mode.slice(1)}Description`)}</span>
            </span>
            <span aria-hidden="true" className="flex h-4 w-4 items-center justify-center rounded-full border border-[color:var(--theme-border-soft,#777777)]">
              {value === mode && <span className="h-2 w-2 rounded-full bg-[var(--theme-control-accent,#E5A00D)]"/>}
            </span>
          </span>
        </span>
      </label>)}
    </div>
    <p id={`${id}-help`} className="text-xs text-[color:var(--theme-fg-muted,#888888)]">{t('settings.appearanceHelp')}</p>
  </fieldset>;
}
