import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent, ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import './App.css';

type TabInfo = {
  label: string;
  url: string;
  title: string;
  active: boolean;
  mini: boolean;
  live: boolean;
  playing: boolean;
  position: number;
  duration: number;
  muted: boolean;
  videos: number;
  feed: boolean;
  rate: number;
};

type Site = { name: string; url: string };

/** Shape the Rust settings struct serialises to (camelCase). */
type Settings = {
  bossKey?: string;
  sites?: Site[];
  sidebarWidth?: number;
  portraitFolds?: boolean;
  miniRatio?: 'port' | 'land';
  miniCinema?: boolean;
  fontScale?: number;
  foldedGroups?: string[];
  closeToTray?: boolean;
};

const DEFAULT_SITES: Site[] = [
  { name: '哔哩哔哩', url: 'https://www.bilibili.com' },
  { name: '抖音', url: 'https://www.douyin.com' },
  { name: '快手', url: 'https://www.kuaishou.com' },
  { name: '西瓜视频', url: 'https://www.ixigua.com' },
  { name: 'AcFun', url: 'https://www.acfun.cn' },
  { name: 'CCTV 直播', url: 'https://tv.cctv.com/live/' },
  { name: 'YouTube', url: 'https://www.youtube.com' },
  { name: 'Twitch', url: 'https://www.twitch.tv' },
];

/** `browser.rs` leaves this much room for the toolbar and the right rail. */
const TOP = 40;
const RAIL = 54;
const PANEL = 232;
const FOLDED = 56;
/** `SIDEBAR_MIN` / `SIDEBAR_MAX` in `browser.rs`, the span it clamps a width into. */
const SIDE_MIN = 52;
const SIDE_MAX = 420;
const LANDSCAPE = { width: 1180, height: 740 };
const PORTRAIT = { width: 480, height: 900 };
/** The two frames `mini_shape` in `browser.rs` answers with; the strip is `MINI_RAIL_W`. */
const MINI_LANDSCAPE = { width: 720, height: 405 };
const MINI_PORTRAIT = { width: 400, height: 760 };
const MINI_STRIP = 43;

function host(url: string) {
  return url.replace(/^https?:\/\//, '').split('/')[0];
}

/** `www.bilibili.com/video/x` and `live.bilibili.com` both belong to 哔哩哔哩. */
function siteKey(url: string) {
  const parts = host(url).split('.');
  return parts.length > 2 ? parts.slice(-2).join('.') : parts.join('.');
}

/** A single letter for the folded rail, so www.bilibili.com still reads as B. */
function badge(name: string) {
  const stripped = name.replace(/^https?:\/\/(?:www\.)?/, '');
  return (stripped.charAt(0) || '?').toUpperCase();
}

const MODIFIER_KEYS = ['Control', 'Alt', 'Shift', 'Meta'];

/** Turns a captured keydown into the string the Rust parser reads back. */
function comboFrom(e: KeyboardEvent): string | undefined {
  if (MODIFIER_KEYS.includes(e.key)) return undefined;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push('Ctrl');
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  if (e.metaKey) mods.push('Super');
  // A bare key would steal it from every other program; function keys are the
  // accepted exception.
  if (mods.length === 0 && !/^F\d{1,2}$/.test(e.code)) return undefined;
  return [...mods, e.code.replace(/^(Key|Digit)/, '')].join('+');
}

function comboLabel(combo: string) {
  return combo.replace('Super', 'Win');
}

function clockAt(sec: number) {
  const s = Math.max(0, Math.round(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  const two = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${two(h)}:${two(m)}:${two(r)}` : `${two(m)}:${two(r)}`;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    const message = typeof e === 'string' ? e : (e as Error)?.message ?? String(e);
    showError(message);
    return undefined;
  }
}

let showError: (message: string) => void = () => {};

function Icon({ name }: { name: string }) {
  const paths: Record<string, ReactNode> = {
    popout: (
      <>
        <path d="M10 6H4.5A1.5 1.5 0 0 0 3 7.5v8A1.5 1.5 0 0 0 4.5 17h8a1.5 1.5 0 0 0 1.5-1.5V11" />
        <path d="M12 3h5v5M17 3l-7 7" />
      </>
    ),
    inbox: (
      <>
        <path d="M12 4v8M8.5 9 12 12.5 15.5 9" />
        <path d="M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
      </>
    ),
    close: <path d="M5 5l14 14M19 5 5 19" />,
    /// The three window keys, drawn rather than typed: a text ─, ▢ and ✕ come
    /// from three different fallback fonts at three different weights, which is
    /// what made them read as mismatched colours in the title row.
    wmin: <path d="M5.5 13.5h13" />,
    wmax: <rect x="5.5" y="5.5" width="13" height="13" rx="2" />,
    wclose: <path d="M6.5 6.5 17.5 17.5M17.5 6.5 6.5 17.5" />,
    /// 主界面收进托盘：一整扇窗，和右下角那一格——托盘就在屏幕那一角。箭头说明
    /// 窗是走进那一格里，不是被关掉。
    tuck: (
      <>
        <rect x="3" y="4" width="12" height="9.5" rx="2" />
        <rect x="14.5" y="14.5" width="6.5" height="6.5" rx="1.6" />
        <path d="M12.6 11.4 17 15.2" />
        <path d="M17.6 12.6v3h-3" />
      </>
    ),
    /// The small window's way out. Inset and lighter than the shell's ✕ so it
    /// reads as "put this away" rather than as a destructive delete.
    exit: <path d="M7 7l10 10M17 7 7 17" />,
    pin: (
      <>
        <path d="M9 4h6l-1 5 3 3H7l3-3z" />
        <path d="M12 12v8" />
      </>
    ),
    fold: (
      <>
        <path d="M14 6 9 12l5 6" />
        <path d="M19 4v16" />
      </>
    ),
    expand: (
      <>
        <path d="M10 6l5 6-5 6" />
        <path d="M5 4v16" />
      </>
    ),
    land: (
      <>
        <rect x="2.6" y="6.6" width="18.8" height="10.8" rx="3" />
        <path d="M7.2 12h5.2M10.4 9.7 12.7 12l-2.3 2.3" />
      </>
    ),
    port: (
      <>
        <rect x="6.6" y="2.6" width="10.8" height="18.8" rx="3" />
        <path d="M12 16.8v-5.2M9.7 13.6 12 11.3l2.3 2.3" />
      </>
    ),
    gear: (
      <>
        <circle cx="12" cy="12" r="3.2" />
        <path d="M19.5 12.5a1.5 1.5 0 0 0 .1 1.9l.1.1a1.8 1.8 0 1 1-2.5 2.5l-.1-.1a1.5 1.5 0 0 0-1.9-.1 1.5 1.5 0 0 0-.9 1.4v.2a1.8 1.8 0 1 1-3.6 0v-.1a1.5 1.5 0 0 0-1-1.4 1.5 1.5 0 0 0-1.9.1l-.1.1a1.8 1.8 0 1 1-2.5-2.5l.1-.1a1.5 1.5 0 0 0 .1-1.9 1.5 1.5 0 0 0-1.4-.9H5.2a1.8 1.8 0 1 1 0-3.6h.1a1.5 1.5 0 0 0 1.4-1 1.5 1.5 0 0 0-.1-1.9l-.1-.1a1.8 1.8 0 1 1 2.5-2.5l.1.1a1.5 1.5 0 0 0 1.9.1h.1a1.5 1.5 0 0 0 .9-1.4V5.2a1.8 1.8 0 1 1 3.6 0v.1a1.5 1.5 0 0 0 .9 1.4 1.5 1.5 0 0 0 1.9-.1l.1-.1a1.8 1.8 0 1 1 2.5 2.5l-.1.1a1.5 1.5 0 0 0-.1 1.9v.1a1.5 1.5 0 0 0 1.4.9h.2a1.8 1.8 0 1 1 0 3.6h-.1a1.5 1.5 0 0 0-1.4.9z" />
      </>
    ),
    plus: <path d="M12 5v14M5 12h14" />,
    trash: (
      <>
        <path d="M4 7h16M10 7V4.5h4V7" />
        <path d="m6 7 1 13h10l1-13" />
      </>
    ),
    back: <path d="M14 6l-6 6 6 6" />,
    fwd: <path d="M10 6l6 6-6 6" />,
    reload: (
      <>
        <path d="M20 12a8 8 0 1 1-2.4-5.7" />
        <path d="M20 4v4h-4" />
      </>
    ),
    chevron: <path d="M9 6l6 6-6 6" />,
    play: <path d="M8 5.5v13l11-6.5z" />,
    pause: <path d="M8.6 5.5h2.4v13H8.6zM13.4 5.5h2.4v13h-2.4z" />,
    up: <path d="M6 14.5 12 8.5l6 6" />,
    down: <path d="M6 9.5l6 6 6-6" />,
    skip: (
      <>
        <path d="M7 5.5 16 12l-9 6.5z" />
        <path d="M18.6 5.5v13" />
      </>
    ),
    backskip: (
      <>
        <path d="M17 5.5 8 12l9 6.5z" />
        <path d="M5.4 5.5v13" />
      </>
    ),
    sound: (
      <>
        <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5z" />
        <path d="M16.4 9.4a4 4 0 0 1 0 5.2" />
        <path d="M18.9 7a7.5 7.5 0 0 1 0 10" />
      </>
    ),
    mute: (
      <>
        <path d="M4 9.5v5h3.5L13 19V5L7.5 9.5z" />
        <path d="M16.5 9.5l5 5M21.5 9.5l-5 5" />
      </>
    ),
  };
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      {paths[name]}
    </svg>
  );
}

/** The app icon drawn: konjac-jelly ghost, play triangle for a mouth. */
function Mark() {
  return (
    <svg className="mark" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <rect x="0" y="0" width="20" height="20" rx="3.9" fill="var(--jelly-ground)" />
      <path
        d="M10 2.3C6.3 2.3 4.1 5.4 4.1 9.6v5.8c0 1.6 2.4 1.6 2.4 0 0 1.6 2.4 1.6 2.4 0 0 1.6 2.4 1.6 2.4 0 0 1.6 2.4 1.6 2.4 0 0 1.6 2.4 1.6 2.4 0V9.6C16.1 5.4 13.7 2.3 10 2.3Z"
        fill="var(--jelly)"
      />
      <circle cx="6.2" cy="6.3" r=".5" fill="var(--jelly-dot)" />
      <circle cx="13.6" cy="7.2" r=".42" fill="var(--jelly-dot)" />
      <circle cx="10.4" cy="4.6" r=".36" fill="var(--jelly-dot)" />
      <circle cx="5.9" cy="12.4" r=".42" fill="var(--jelly-dot)" />
      <circle cx="14.4" cy="12.9" r=".5" fill="var(--jelly-dot)" />
      <ellipse cx="7.5" cy="9.4" rx="1.1" ry="1.45" fill="var(--ink)" />
      <ellipse cx="12.5" cy="9.4" rx="1.1" ry="1.45" fill="var(--ink)" />
      <path d="M9 11.9 12.6 13.9 9 15.9Z" fill="var(--jelly-play)" stroke="var(--jelly-play)" strokeWidth="1.1" strokeLinejoin="round" />
    </svg>
  );
}

/** Titles longer than the row scroll past instead of truncating to "…". */
function Ticker({ text }: { text: string }) {
  const box = useRef<HTMLSpanElement>(null);
  const [long, setLong] = useState(false);
  useEffect(() => {
    const el = box.current;
    if (el) setLong(el.scrollWidth > el.clientWidth + 2);
  }, [text]);
  if (!long) {
    return (
      <span className="tick" ref={box}>
        {text}
      </span>
    );
  }
  return (
    <span className="tick cut" ref={box}>
      <span className="run">
        <i>{text}</i>
        <i>{text}</i>
      </span>
    </span>
  );
}

function useTabs(every = 1200) {
  const [tabs, setTabs] = useState<TabInfo[]>([]);
  const apply = useCallback((t: TabInfo[]) => setTabs(t), []);
  useEffect(() => {
    invoke<TabInfo[]>('list_tabs').then(apply).catch(() => {});
    const un = listen<TabInfo[]>('browser://tabs', (e) => apply(e.payload));
    const timer = setInterval(() => {
      invoke<TabInfo[]>('list_tabs').then(apply).catch(() => {});
    }, every);
    return () => {
      un.then((f) => f());
      clearInterval(timer);
    };
  }, [apply, every]);
  return tabs;
}

/**
 * The strip down the right of the small window.
 *
 * It is a webview of our own, standing outside the page rather than over it, so
 * none of these buttons can ever cover 点赞 or the site's own progress bar. Each
 * one names something for Rust to hand to the bridge in the page next door; the
 * lit states come back from that page, so what is showing is what the player is
 * actually doing.
 */
function MiniRail() {
  const tabs = useTabs(400);
  const [pinned, setPinned] = useState(true);
  const [note, setNote] = useState('');
  const picture = usePicture();
  const mini = tabs.find((t) => t.mini);
  const title = mini?.title || (mini ? host(mini.url) : '小屏');
  const playing = !!mini?.playing;
  const land = picture.w >= picture.h;
  const hold = (e: MouseEvent) => e.stopPropagation();
  // The strip has no error row of its own, so a refused call would leave the
  // button looking simply dead. Each call reports here instead: the strip washes
  // itself in the warning colour, the reason goes into the hover text, and where
  // there is room for a caption the text shows there too.
  const ask = (cmd: string, args?: Record<string, unknown>) =>
    invoke(cmd, args).catch((e) =>
      setNote(typeof e === 'string' ? e : (e as Error)?.message ?? String(e)),
    );
  const ctl = (what: string) => ask('mini_ctl', { what });
  // 主界面收没收进托盘只有一处真相：Rust。竖栏这个键和标题行那个键都来这里查，
  // 所以两边永远亮得一样。
  const [tucked, setTucked] = useState(false);
  const readTucked = useCallback(() => {
    invoke<boolean>('main_tucked')
      .then((v) => setTucked(!!v))
      .catch(() => {});
  }, []);
  useEffect(() => {
    readTucked();
    const t = setInterval(readTucked, 700);
    return () => clearInterval(t);
  }, [readTucked]);
  const pct = mini && mini.duration > 0 ? Math.min(100, (mini.position / mini.duration) * 100) : 0;

  useEffect(() => {
    if (!note) return;
    const t = setTimeout(() => setNote(''), 6000);
    return () => clearTimeout(t);
  }, [note]);

  return (
    <div
      className={note ? 'vstrip bad' : 'vstrip'}
      title={note || undefined}
      onMouseDown={() => getCurrentWindow().startDragging().catch(() => {})}
    >
      <button className="vstrip-btn x" title="收回：页面退回主窗口，视频暂停" onMouseDown={hold} onClick={() => ask('attach_back')}>
        <Icon name="exit" />
        <i>收回</i>
      </button>
      <button
        className={pinned ? 'vstrip-btn on' : 'vstrip-btn'}
        title={pinned ? '取消置顶：小窗不再压在其他窗口上面' : '置顶：小窗压在其他窗口上面'}
        onMouseDown={hold}
        onClick={() => {
          const next = !pinned;
          setPinned(next);
          ask('mini_topmost', { on: next });
        }}
      >
        <Icon name="pin" />
        <i>置顶</i>
      </button>
      <button
        className="vstrip-btn shape"
        title={land ? '切换成竖屏 9:16：抖音、快手这类上下滑的用这个' : '切换成横屏 16:9：B 站、ACFun 这类整页视频用这个'}
        onMouseDown={hold}
        onClick={() => ask('mini_size', land ? MINI_PORTRAIT : MINI_LANDSCAPE)}
      >
        <Icon name={land ? 'port' : 'land'} />
        <i>{land ? '竖屏' : '横屏'}</i>
      </button>
      <button
        className={tucked ? 'vstrip-btn on' : 'vstrip-btn'}
        title={
          tucked
            ? '唤回主界面：站点库和标签都回来，小窗不停'
            : '收起主界面：桌面只留这个小窗，主界面藏进右下角托盘（声音不停）'
        }
        onMouseDown={hold}
        onClick={() => ask('window_control', { what: 'tuck' }).then(readTucked)}
      >
        <Icon name="tuck" />
        <i>{tucked ? '唤回' : '收起'}</i>
      </button>
      <span className="vstrip-split" />
      <button className="vstrip-btn" title={`${playing ? '暂停' : '播放'}（点一下画面也能开关）`} onMouseDown={hold} onClick={() => ctl('toggle')}>
        <Icon name={playing ? 'pause' : 'play'} />
        <i>{playing ? '暂停' : '播放'}</i>
      </button>
      <button className="vstrip-btn" title="倍速：点一下换一档；按住画面可临时再快一倍" onMouseDown={hold} onClick={() => ctl('rate')}>
        <span className="rate">{mini?.rate || 1}×</span>
        <i>倍速</i>
      </button>
      <button
        className={mini?.muted ? 'vstrip-btn on' : 'vstrip-btn'}
        title={`${mini?.muted ? '取消静音' : '静音'}（在按钮上滚滚轮调音量，主界面侧栏是同一个开关）`}
        onMouseDown={hold}
        onWheel={(e) => ctl(e.deltaY < 0 ? 'louder' : 'softer')}
        onClick={() => mini && ask('set_mute', { label: mini.label, on: !mini.muted })}
      >
        <Icon name={mini?.muted ? 'mute' : 'sound'} />
        <i>静音</i>
      </button>
      <span className="vstrip-split" />
      {/* 上/下一集（短视频站是上/下一条）走同一行：两个半尺寸的键，中间不留缝。 */}
      <span className="vstrip-pair">
        <button
          className="vstrip-btn half"
          title={mini?.feed ? '上一条（键盘 ↑ 也一样）' : '上一集：找页面上的上一集按钮并点它'}
          onMouseDown={hold}
          onClick={() => ctl(mini?.feed ? 'up' : 'prev')}
        >
          <Icon name={mini?.feed ? 'up' : 'backskip'} />
        </button>
        <button
          className="vstrip-btn half"
          title={mini?.feed ? '下一条（键盘 ↓ 也一样）' : '下一集：找页面上的下一集按钮并点它'}
          onMouseDown={hold}
          onClick={() => ctl(mini?.feed ? 'down' : 'next')}
        >
          <Icon name={mini?.feed ? 'down' : 'skip'} />
        </button>
      </span>
      <span className="vstrip-foot">
        <span className={playing ? 'vstrip-mark on' : mini?.live ? 'vstrip-mark' : 'vstrip-mark load'} title={title}>
          <Mark />
        </span>
        <span className="vstrip-time">
          {mini && mini.duration > 0 ? (
            <>
              {clockAt(mini.duration)}
              <wbr />
              {Math.round(pct)}%
            </>
          ) : (
            ''
          )}
        </span>
        <span className="vstrip-track">{pct > 0 && <i style={{ width: `${pct}%` }} />}</span>
        <span className={note ? 'vstrip-cap bad' : 'vstrip-cap'}>{note || '小窗中'}</span>
      </span>
    </div>
  );
}

/**
 * How big the picture is right now, read off the window the strip is standing
 * in: the strip's own width is the only thing taken from the frame, so what is
 * left over is the video's. The button that flips the frame needs this because
 * the same flip can come from the main window, from a drag of the edge, or from
 * here, and only the window knows which one it is in.
 */
function usePicture() {
  const [box, setBox] = useState({ w: MINI_PORTRAIT.width, h: MINI_PORTRAIT.height });
  useEffect(() => {
    const win = getCurrentWindow();
    let live = true;
    const read = () => {
      Promise.all([win.innerSize(), win.scaleFactor()])
        .then(([s, k]) => live && setBox({ w: s.width / k - MINI_STRIP, h: s.height / k }))
        .catch(() => {});
    };
    read();
    const un = win.onResized(read);
    return () => {
      live = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);
  return box;
}

function TabRow({ tab, full }: { tab: TabInfo; full: boolean }) {
  const hasRail = tab.duration > 0;
  const pct = hasRail ? Math.min(100, (tab.position / tab.duration) * 100) : 0;
  const cls = ['tab', tab.active ? 'on' : '', tab.mini ? 'mini' : ''].filter(Boolean).join(' ');
  const focus = () => call('activate_tab', { label: tab.label });
  const name = tab.title || host(tab.url);

  if (!full) {
    return (
      <div className={cls} onClick={focus} role="button" tabIndex={0} onKeyDown={(e) => e.key === 'Enter' && focus()} title={name}>
        <span className={tab.playing ? 'beacon on' : 'beacon'} aria-hidden="true" />
        <span className="letter">{badge(name)}</span>
      </div>
    );
  }

  return (
    <div className={cls} onClick={focus} role="button" tabIndex={0} onKeyDown={(e) => e.key === 'Enter' && focus()}>
      <div className="row1">
        <span className={tab.playing ? 'beacon on' : 'beacon'} aria-hidden="true" />
        <Ticker text={name} />
        <span className="acts" onClick={(e) => e.stopPropagation()}>
          {tab.mini ? (
            <button className="ic" title="还原到主窗口" onClick={() => call('attach_back')}>
              <Icon name="inbox" />
            </button>
          ) : (
            <button className="ic" title="发到置顶小窗" onClick={() => call('detach_mini', { label: tab.label })}>
              <Icon name="popout" />
            </button>
          )}
          <button
            className={tab.muted ? 'ic on' : 'ic'}
            title={tab.muted ? '取消静音（与小窗同一个开关）' : '静音此页'}
            onClick={() => call('set_mute', { label: tab.label, on: !tab.muted })}
          >
            <Icon name={tab.muted ? 'mute' : 'sound'} />
          </button>
          <button className="ic" title="关闭" onClick={() => call('close_tab', { label: tab.label })}>
            <Icon name="close" />
          </button>
        </span>
      </div>
      <div className="progress">
        <span className="track">{hasRail && <span className="fill" style={{ width: `${pct}%` }} />}</span>
        <span className="stamp">
          {hasRail
            ? `${clockAt(tab.position)} / ${clockAt(tab.duration)}`
            : tab.playing
              ? '播放中'
              : tab.videos > 0
                ? `${tab.videos} 个播放器`
                : '未识别到视频'}
        </span>
      </div>
    </div>
  );
}

function Row({ label, on, onClick, hint }: { label: string; on: boolean; onClick: () => void; hint?: string }) {
  return (
    <button className={on ? 'row on' : 'row'} onClick={onClick} title={hint}>
      <span>{label}</span>
      <span className="dot" aria-hidden="true" />
    </button>
  );
}

function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="sec">
      <h2>{title}</h2>
      {children}
      {note && <p className="note">{note}</p>}
    </section>
  );
}

function SettingsSheet({
  settings,
  save,
  tabs,
  onCapture,
  capturing,
  onWidth,
  onOpen,
  log,
  onClose,
}: {
  settings: Settings;
  save: (patch: Settings) => void;
  tabs: TabInfo[];
  onCapture: () => void;
  capturing: boolean;
  onWidth: (width: number) => void;
  onOpen: (url: string) => void;
  log: string[];
  onClose: () => void;
}) {
  const [showLog, setShowLog] = useState(false);
  // The library is written on every change, so what is on screen is what is saved.
  const rows = settings.sites?.length ? settings.sites : DEFAULT_SITES;
  const [picked, setPicked] = useState<number[]>([]);
  const [edit, setEdit] = useState<{ at: number | null; name: string; url: string } | null>(null);
  const [ask, setAsk] = useState<number[] | null>(null);

  const write = (next: Site[]) => save({ sites: next.map((s) => ({ name: s.name.trim(), url: s.url.trim() })) });
  const drop = (indexes: number[]) => {
    write(rows.filter((_, i) => !indexes.includes(i)));
    setPicked([]);
    setAsk(null);
  };
  const saveDraft = () => {
    if (!edit) return;
    const name = edit.name.trim();
    let url = edit.url.trim();
    if (!name || !url) return;
    if (!/^https?:\/\//i.test(url)) url = 'https://' + url.replace(/^\/+/, '');
    write(edit.at === null ? [...rows, { name, url }] : rows.map((r, i) => (i === edit.at ? { name, url } : r)));
    setEdit(null);
    setPicked([]);
  };

  const width = Math.round(settings.sidebarWidth ?? PANEL);
  const font = Math.round((settings.fontScale ?? 1) * 100);

  return (
    <>
    <div className="sheet">
      <div className="sheet-head">
        <button className="soft" onClick={onClose}>
          <Icon name="back" />
          返回播放
        </button>
        <h1>设置</h1>
      </div>
      <div className="sheet-body">
        <div className="col">
          <Section title="基本设置">
            <label className="slider">
              <span>
                界面字体<i>{font}%</i>
              </span>
              <input
                type="range"
                min={85}
                max={130}
                step={5}
                value={font}
                onChange={(e) => save({ fontScale: Number(e.target.value) / 100 })}
              />
            </label>
            <label className="slider">
              <span>
                侧栏宽度<i>{Math.max(PANEL, width)}px</i>
              </span>
              <input
                type="range"
                min={PANEL}
                max={340}
                step={4}
                value={Math.max(PANEL, width)}
                onChange={(e) => onWidth(Number(e.target.value))}
              />
            </label>
            <Row
              label="竖屏时收起侧栏"
              on={settings.portraitFolds !== false}
              onClick={() => save({ portraitFolds: settings.portraitFolds === false })}
              hint="切到竖屏窗口时自动折叠左侧列表，横屏时自动展开"
            />
          </Section>

          <Section
            title="置顶小窗"
            note="小窗里只留播放器：点一下画面出现控制条（进度、暂停、倍速、音量、下一集），按住画面不放临时快进一倍、松开恢复，右键回到主界面。抖音、快手这类上下滑的短视频站，控制条上的「下一集」会换成上下翻页按钮，小窗里直接按键盘 ↓ 翻下一条、↑ 回上一条。"
          >
            <div className="seg">
              <button className={settings.miniRatio === 'port' ? 'on' : ''} onClick={() => save({ miniRatio: 'port' })}>
                <Icon name="port" />
                竖屏 9:16
              </button>
              <button className={settings.miniRatio === 'land' ? 'on' : ''} onClick={() => save({ miniRatio: 'land' })}>
                <Icon name="land" />
                横屏 16:9
              </button>
            </div>
            <Row
              label="小窗只保留播放器"
              on={settings.miniCinema !== false}
              onClick={() => save({ miniCinema: settings.miniCinema === false })}
              hint="关闭后小窗显示站点原样的页面"
            />
          </Section>

          <Section
            title="藏在右下角"
            note="点 ✕ 或按 Alt+F4 时，窗口收进屏幕右下角的托盘图标里，声音继续放：左键点图标唤回，右键有「全部暂停并静音」和「退出」。老板键不一样，它会先暂停再静音，是彻底藏起来。第一次收进去时，Windows 可能把图标放进右下角那个 ^ 展开层里，点 ^ 就能看到它。"
          >
            <div className="seg">
              <button
                className={settings.closeToTray !== false ? 'on' : ''}
                onClick={() => save({ closeToTray: true })}
              >
                关闭时缩到右下角
              </button>
              <button
                className={settings.closeToTray === false ? 'on' : ''}
                onClick={() => save({ closeToTray: false })}
              >
                关闭时直接退出
              </button>
            </div>
          </Section>

          <Section title="快捷键">
            <button className={capturing ? 'row on' : 'row'} onClick={onCapture}>
              <span>老板键</span>
              <span className="kbd">{comboLabel(settings.bossKey ?? 'Ctrl+Shift+H')}</span>
            </button>
            <p className="note">点击后直接按下新的组合键，Esc 取消。改完立刻生效并写入 settings.json，重启后仍然用它。</p>
            {settings.bossKey && !/Ctrl|Alt|Super/.test(settings.bossKey) && (
              <p className="note warn">当前快捷键只用了 Shift：日常输入大写字母就会触发隐藏，建议至少带上 Ctrl 或 Alt。</p>
            )}
          </Section>

          <Section title="诊断">
            <button className="soft" onClick={() => setShowLog(!showLog)}>
              {showLog ? '收起日志' : '展开运行日志'}
            </button>
            {showLog && <pre className="logbox">{log.slice(-30).join('\n')}</pre>}
          </Section>
        </div>

        <div className="col">
          <Section title="网页管理" note="当前打开的页面。关闭其中一个不影响其他页面的播放。">
            <div className="manage">
              {tabs.length === 0 && <p className="empty">现在没有打开的页面。</p>}
              {tabs.map((t) => (
                <div className="mrow" key={t.label}>
                  <span className={t.playing ? 'beacon on' : 'beacon'} aria-hidden="true" />
                  <span className="mname">{t.title || host(t.url)}</span>
                  <span className="mhost">{host(t.url)}</span>
                  <button className="ic" title="发到置顶小窗" onClick={() => call('detach_mini', { label: t.label })}>
                    <Icon name="popout" />
                  </button>
                  <button className="ic" title="关闭" onClick={() => call('close_tab', { label: t.label })}>
                    <Icon name="close" />
                  </button>
                </div>
              ))}
            </div>
            {tabs.length > 0 && (
              <button className="link" onClick={() => call('close_all')}>
                全部关闭
              </button>
            )}
          </Section>

          <Section
            title="账号与登录"
            note="登录状态只保存在这台电脑的魔芋播放器目录里，和浏览器、其他站点都不相通。退出软件时会写入磁盘，下次打开仍然是登录状态；掉线或换账号，就在页面里重新登录一次。"
          >
            <div className="setacts">
              {rows.slice(0, 6).map((s) => (
                <button
                  className="soft"
                  key={s.url}
                  title={`打开 ${s.name}，在页面里登录`}
                  onClick={() => onOpen(s.url)}
                >
                  {s.name}
                </button>
              ))}
            </div>
          </Section>

          <Section title="常用站点" note="这一份列表同时出现在顶部菜单的「站点」里；清空后自动回到内置默认。">
            <div className="srows">
              {rows.map((s, i) => (
                <div className="srow" key={`${s.url}-${i}`}>
                  <input
                    type="checkbox"
                    className="spick"
                    checked={picked.includes(i)}
                    title="选中以便批量删除"
                    onChange={(e) => setPicked(e.target.checked ? [...picked, i] : picked.filter((j) => j !== i))}
                  />
                  <button
                    className="sinfo"
                    title="点击编辑名称和链接"
                    onClick={() => setEdit({ at: i, name: s.name, url: s.url })}
                  >
                    <span className="sname">{s.name}</span>
                    <span className="shost">{s.url}</span>
                  </button>
                  <button className="ic" title="打开这个站点" onClick={() => onOpen(s.url)}>
                    <Icon name="popout" />
                  </button>
                  <button className="ic" title="删除这个站点" onClick={() => setAsk([i])}>
                    <Icon name="trash" />
                  </button>
                </div>
              ))}
            </div>
            <div className="setacts">
              <button className="soft" onClick={() => setEdit({ at: null, name: '', url: '' })}>
                <Icon name="plus" />
                添加站点
              </button>
              <button className="soft" disabled={!picked.length} onClick={() => setAsk(picked)}>
                <Icon name="trash" />
                {picked.length ? `删除所选 ${picked.length} 个` : '批量删除'}
              </button>
              <button className="link" onClick={() => save({ sites: [] })}>
                恢复默认
              </button>
            </div>
          </Section>
        </div>
      </div>
    </div>

      {edit && (
        <div className="veil" onClick={() => setEdit(null)}>
          <div className="modal mini" onClick={(e) => e.stopPropagation()}>
            <h1>{edit.at === null ? '添加站点' : '编辑站点'}</h1>
            <label className="field">
              <span>站点名称</span>
              <input
                autoFocus
                value={edit.name}
                placeholder="例如：B站"
                spellCheck={false}
                onChange={(e) => setEdit({ ...edit, name: e.target.value })}
              />
            </label>
            <label className="field">
              <span>站点链接</span>
              <input
                value={edit.url}
                placeholder="https://example.com"
                spellCheck={false}
                onChange={(e) => setEdit({ ...edit, url: e.target.value })}
                onKeyDown={(e) => e.key === 'Enter' && saveDraft()}
              />
            </label>
            <p className="note">两项都填好才能保存；链接漏了开头的 https:// 会自动补上。保存后立刻出现在侧栏和「站点」菜单里。</p>
            <div className="dlgfoot">
              <button className="soft" onClick={() => setEdit(null)}>
                取消
              </button>
              <button className="primary" disabled={!(edit.name.trim() && edit.url.trim())} onClick={saveDraft}>
                保存
              </button>
            </div>
          </div>
        </div>
      )}

      {ask && (
        <div className="veil" onClick={() => setAsk(null)}>
          <div className="modal mini" onClick={(e) => e.stopPropagation()}>
            <h1>是否确认删除</h1>
            <p className="asktext">
              {ask.map((i) => rows[i]?.name).filter(Boolean).join('、') || '选中的站点'}
            </p>
            <p className="note">删除只把站点从这份列表里去掉，不会退出登录，也不会关掉已经打开的页面。</p>
            <div className="dlgfoot">
              <button className="soft" onClick={() => setAsk(null)}>
                取消
              </button>
              <button className="primary warn" onClick={() => drop(ask)}>
                确认删除
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

function Guide({ what, onClose }: { what: 'guide' | 'about'; onClose: () => void }) {
  return (
    <div className="veil" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h1>{what === 'guide' ? '使用说明' : '关于魔芋播放器'}</h1>
        {what === 'guide' ? (
          <ul className="list">
            <li>
              <b>打开页面</b>
              <span>第一行的「站点」菜单，或空界面中间的站点按钮；也可以在地址栏粘贴链接、输入关键词后回车。</span>
            </li>
            <li>
              <b>页面里的链接</b>
              <span>点视频就在当前页打开，左侧列表按站点归并；点站点名一行的箭头可以收起或展开它的页面。</span>
            </li>
            <li>
              <b>置顶小窗</b>
              <span>识别到视频时右侧会闪动提示。右侧两个按钮是「以小窗竖屏打开」「以小窗横屏打开」；小窗里点画面出控制条——进度可拖、暂停、倍速（点按切换，按住画面临时快进一倍）、音量条、「下一集」，右键回到主界面。抖音、快手这类滑动翻页的短视频站，「下一集」位置换成上下翻页按钮，小窗里按 ↓ 下一条、↑ 上一条；B站、AcFun 这类长视频站仍是跳下一集。</span>
            </li>
            <li>
              <b>关闭小窗</b>
              <span>点右下角收纳图标或小窗上的还原按钮，页面退回主窗口并自动暂停，不会关掉网页。</span>
            </li>
            <li>
              <b>藏在右下角</b>
              <span>点 ✕ 只把窗口收进任务栏右侧的托盘图标，声音继续放：左键点图标唤回，右键菜单里有「全部暂停并静音」和「退出」。想彻底退出就用右键的「退出」或菜单「文件 → 退出魔芋播放器」。</span>
            </li>
            <li>
              <b>老板键</b>
              <span>默认 Ctrl+Shift+H，可在设置里改成任意组合键。隐藏时会同时静音并暂停所有播放。</span>
            </li>
            <li>
              <b>竖屏 / 横屏</b>
              <span>菜单「视图」里的两项改的是主窗口大小；侧栏底部「设置」里的小窗比例决定小窗默认形状。</span>
            </li>
          </ul>
        ) : (
          <div className="about">
            <p>魔芋播放器：把多个视频网站放在一个窗口里看，账号登录状态保存在本机，可以置顶小窗和一键隐藏。</p>
            <p className="fine">只做聚合与播放，不破解会员、不下载、不绕过任何网站限制；也不做软件更新。</p>
          </div>
        )}
        <button className="primary" onClick={onClose}>
          知道了
        </button>
      </div>
    </div>
  );
}

/// The title row's menus. Ids are the ones Rust logs and `menu()` already
/// answers to, so the title row, the keyboard and the old native bar all drive
/// the same set of actions.
type Menu = { id: string; name: string; items: [string, string][] };

const MENUS: Menu[] = [
  {
    id: 'file',
    name: '文件',
    items: [
      ['file.new', '打开新页面'],
      ['file.close', '关闭当前页面'],
      ['file.all', '全部关闭'],
      ['file.quit', '退出魔芋播放器'],
    ],
  },
  {
    id: 'edit',
    name: '编辑',
    items: [
      ['edit.back', '后退'],
      ['edit.forward', '前进'],
      ['edit.reload', '刷新当前页面'],
      ['edit.pause', '暂停当前页面的播放'],
      ['edit.mute', '静音当前页面'],
    ],
  },
  {
    id: 'view',
    name: '视图',
    items: [
      ['view.fold', '收起 / 展开左侧列表'],
      ['view.mini.port', '以小窗竖屏打开（9:16）'],
      ['view.mini.land', '以小窗横屏打开（16:9）'],
      ['view.mini', '以小窗默认比例打开'],
      ['view.attach', '把小窗收回主窗口并暂停'],
      ['view.portrait', '主窗口改竖屏（短视频）'],
      ['view.landscape', '主窗口改横屏'],
      ['view.settings', '设置'],
    ],
  },
  { id: 'site', name: '站点', items: [] },
  {
    id: 'help',
    name: '帮助',
    items: [
      ['help.guide', '使用说明'],
      ['help.diag', '诊断日志'],
      ['help.about', '关于魔芋播放器'],
    ],
  },
];

/** The rows one menu shows. 站点 is built from the live library, addressed by
 * index so Rust opens the same entry the settings page lists. */
function menuItems(m: Menu, sites: Site[]): [string, string][] {
  if (m.id !== 'site') return m.items;
  return [...sites.map((s, i) => [`site.${i}`, s.name] as [string, string]), ['site.manage', '管理常用站点…']];
}

function Shell() {
  const tabs = useTabs();
  const [url, setUrl] = useState('');
  const [log, setLog] = useState<string[]>([]);
  const [error, setError] = useState('');
  const [settings, setSettings] = useState<Settings>({});
  const [view, setView] = useState<'pages' | 'settings'>('pages');
  const [modal, setModal] = useState<'guide' | 'about' | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [narrow, setNarrow] = useState(false);
  const addr = useRef<HTMLInputElement>(null);
  // Remembers that the tall frame folded the list, so restoring landscape
  // unfolds again — and a list the user folded by hand stays folded.
  const foldedByShape = useRef(false);

  const sites = settings.sites?.length ? settings.sites : DEFAULT_SITES;
  const folded = (settings.sidebarWidth ?? PANEL) <= FOLDED + (PANEL - FOLDED) / 2;
  // The pages are laid out at exactly this width by `set_sidebar_width`, so the
  // shell's own column has to be drawn at it too — not at the fixed PANEL the
  // slider started from, which is what left a strip of shell beside a page.
  const side = Math.round(Math.min(SIDE_MAX, Math.max(SIDE_MIN, settings.sidebarWidth ?? PANEL)));
  const activeTab = tabs.find((t) => t.active);
  const miniTab = tabs.find((t) => t.mini);

  const save = useCallback((patch: Settings) => {
    call<Settings>('save_settings', { patch }).then((next) => {
      if (next) setSettings(next);
    });
  }, []);

  useEffect(() => {
    invoke<Settings>('get_settings').then(setSettings).catch(() => {});
  }, []);

  useEffect(() => {
    document.documentElement.style.setProperty('--fs', String(settings.fontScale ?? 1));
  }, [settings.fontScale]);

  // The native 站点 submenu and the toolbar row both come from this list.
  useEffect(() => {
    call('set_site_menu', { sites });
  }, [sites]);

  // Only the shell's own full-window sheets need the pages taken out of the
  // way: the site pages are separate windows stacked above the shell, so no
  // amount of z-index in this document lifts HTML over a playing video. The
  // title-row menus are system popups, drawn above everything by Windows
  // itself, so opening one no longer costs the user a vanished page.
  useEffect(() => {
    call('pages_visible', { on: view !== 'settings' && !modal });
  }, [view, modal]);

  useEffect(() => {
    const sync = () => setNarrow(window.innerWidth < 860);
    sync();
    addEventListener('resize', sync);
    return () => removeEventListener('resize', sync);
  }, []);

  useEffect(() => {
    const un = listen<string>('browser://menu', (e) => menuRef.current(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        setCapturing(false);
        return;
      }
      const next = comboFrom(e);
      if (!next) return;
      call<string>('set_boss_key', { combo: next }).then((applied) => {
        if (applied) setSettings((s) => ({ ...s, bossKey: applied }));
        setCapturing(false);
      });
    };
    addEventListener('keydown', onKey, true);
    return () => removeEventListener('keydown', onKey, true);
  }, [capturing]);

  useEffect(() => {
    showError = setError;
    return () => {
      showError = () => {};
    };
  }, []);

  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(''), 6000);
    return () => clearTimeout(t);
  }, [error]);

  const refreshLog = useCallback(() => {
    invoke<string[]>('app_log').then(setLog).catch(() => {});
  }, []);

  // Only 设置 shows the log, so the tail is only fetched while that page is
  // open. Polling it from every screen put a command through the main process
  // for the whole session to feed a panel that is almost always closed.
  useEffect(() => {
    if (view !== 'settings') return;
    refreshLog();
    const t = setInterval(refreshLog, 2000);
    return () => clearInterval(t);
  }, [refreshLog, view]);

  const open = (target: string) => {
    const value = target.trim();
    if (!value) return;
    setView('pages');
    call('open_tab', { url: value }).then(() => {
      if (target === url) setUrl('');
      refreshLog();
    });
  };

  /** Width is a stored preference, so folding survives a restart. */
  const applyWidth = useCallback(
    (width: number) => {
      const nextFolded = width <= FOLDED + (PANEL - FOLDED) / 2;
      if (nextFolded && view === 'settings') setView('pages');
      save({ sidebarWidth: width });
    },
    [save, view],
  );

  const fold = () => applyWidth(folded ? PANEL : FOLDED);

  const shape = (portrait: boolean) => {
    if (portrait) {
      call('set_main_size', PORTRAIT);
      if (settings.portraitFolds !== false && !folded) {
        foldedByShape.current = true;
        applyWidth(FOLDED);
      }
    } else {
      call('set_main_size', LANDSCAPE);
      if (foldedByShape.current) {
        foldedByShape.current = false;
        applyWidth(PANEL);
      }
    }
  };

  const toggleMini = (shape?: 'port' | 'land') => {
    if (miniTab) call('attach_back');
    else if (activeTab) call('detach_mini', { label: activeTab.label, shape: shape ?? null });
  };

  /** The rail's two shape buttons: they open the page in a mini window of that
   * ratio, and resize the mini window when one is already out. */
  const miniShape = (which: 'port' | 'land') => {
    if (miniTab) call('mini_size', which === 'port' ? MINI_PORTRAIT : MINI_LANDSCAPE);
    else toggleMini(which);
  };

  /** One route for every title-row action, so the log reads the same however
   * the action was reached. `null` means the combined 菜单 button, which lists
   * all five groups with a heading each. */
  const popup = (e: React.MouseEvent<HTMLButtonElement>, id: string | null) => {
    const r = e.currentTarget.getBoundingClientRect();
    const items = id
      ? menuItems(MENUS.find((m) => m.id === id)!, sites).map(([action, label]) => ({ id: action, label }))
      : MENUS.flatMap((m) =>
          menuItems(m, sites).map(([action, label], n) => ({
            id: action,
            label,
            heading: n === 0 ? m.name : null,
          })),
        );
    call('popup_menu', { items, x: r.left, y: r.bottom });
  };

  const menu = (id: string) => {
    // Rust already opened the page; coming back here only brings it in view.
    if (id.startsWith('site.')) return void setView('pages');
    if (id === 'file.new') return void addr.current?.focus();
    if (id === 'file.close') return void (activeTab && call('close_tab', { label: activeTab.label }));
    if (id === 'file.all') return void call('close_all');
    if (id === 'file.quit') return void call('quit');
    if (id === 'edit.back') return void call('page_nav', { dir: 'back' });
    if (id === 'edit.forward') return void call('page_nav', { dir: 'forward' });
    if (id === 'edit.reload') return void call('page_nav', { dir: 'reload' });
    if (id === 'edit.pause') return void call('page_nav', { dir: 'pause' });
    if (id === 'edit.mute') return void call('page_nav', { dir: 'mute' });
    if (id === 'view.portrait') return void shape(true);
    if (id === 'view.landscape') return void shape(false);
    if (id === 'view.fold') return void fold();
    if (id === 'view.mini') return void toggleMini();
    if (id === 'view.mini.port') return void miniShape('port');
    if (id === 'view.mini.land') return void miniShape('land');
    if (id === 'view.attach') return void call('attach_back');
    if (id === 'view.settings' || id === 'site.manage') return void setView('settings');
    if (id === 'help.guide') return void setModal('guide');
    if (id === 'help.about') return void setModal('about');
    if (id === 'help.diag') return void setView('settings');
  };

  const menuRef = useRef(menu);
  menuRef.current = menu;

  const groups = useMemo(() => {
    const byKey = new Map<string, { key: string; name: string; tabs: TabInfo[] }>();
    for (const t of tabs) {
      const key = siteKey(t.url);
      const known = sites.find((s) => siteKey(s.url) === key);
      const found = byKey.get(key);
      if (found) found.tabs.push(t);
      else byKey.set(key, { key, name: known?.name ?? host(t.url), tabs: [t] });
    }
    return [...byKey.values()];
  }, [tabs, sites]);

  const foldedSet = new Set(settings.foldedGroups ?? []);
  const toggleGroup = (key: string) => {
    const next = foldedSet.has(key) ? [...foldedSet].filter((k) => k !== key) : [...foldedSet, key];
    save({ foldedGroups: next });
  };

  const prompt = !!activeTab && activeTab.videos > 0 && !miniTab;

  return (
    <div
      className={narrow ? 'app narrow' : 'app'}
      style={
        {
          '--side': `${side}px`,
          '--top': `${TOP}px`,
          '--rail': `${RAIL}px`,
        } as CSSProperties
      }
    >
      <header className="top" data-tauri-drag-region>
        <span className="brand" data-tauri-drag-region>
          <Mark />
          <span className="bname">魔芋播放器</span>
        </span>
        <nav className="menus">
          {narrow ? (
            <button className="mb" title="打开全部菜单" onClick={(e) => popup(e, null)}>
              菜单
            </button>
          ) : (
            MENUS.map((m) => (
              <button className="mb" key={m.id} title={`打开${m.name}菜单`} onClick={(e) => popup(e, m.id)}>
                {m.name}
              </button>
            ))
          )}
        </nav>
        {!narrow && (
          <span className="nav">
            <button className="nb" title="后退" onClick={() => call('page_nav', { dir: 'back' })}>
              <Icon name="back" />
            </button>
            <button className="nb" title="前进" onClick={() => call('page_nav', { dir: 'forward' })}>
              <Icon name="fwd" />
            </button>
            <button className="nb" title="刷新" onClick={() => call('page_nav', { dir: 'reload' })}>
              <Icon name="reload" />
            </button>
          </span>
        )}
        <input
          ref={addr}
          className="addr"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && open(url)}
          placeholder="粘贴地址，或输入关键词"
          spellCheck={false}
        />
        {!narrow && (
          <button className="go" onClick={() => open(url)} disabled={!url.trim()}>
            打开
          </button>
        )}
        <span className="wbtns" data-tauri-drag-region>
          {miniTab && (
            <button
              className="wb"
              title="收进托盘：主界面藏到右下角，只留置顶小窗继续播（小窗右侧竖栏是同一个键）"
              onClick={() => call('window_control', { what: 'tuck' })}
            >
              <Icon name="tuck" />
            </button>
          )}
          <button className="wb" title="最小化" onClick={() => call('window_control', { what: 'min' })}>
            <Icon name="wmin" />
          </button>
          <button className="wb" title="最大化 / 还原" onClick={() => call('window_control', { what: 'max' })}>
            <Icon name="wmax" />
          </button>
          <button
            className="wb quit"
            title={settings.closeToTray === false ? '关闭魔芋播放器' : '收进右下角托盘：窗口消失，声音继续'}
            onClick={() => call('window_control', { what: 'close' })}
          >
            <Icon name="wclose" />
          </button>
        </span>
      </header>

      {error && <div className="err">{error}</div>}

      {view === 'settings' ? (
        <SettingsSheet
          settings={settings}
          save={save}
          tabs={tabs}
          log={log}
          onCapture={() => setCapturing(true)}
          capturing={capturing}
          onWidth={applyWidth}
          onOpen={open}
          onClose={() => setView('pages')}
        />
      ) : (
        <div className="body">
          <aside className={folded ? 'side folded' : 'side'}>
            <div className="slist">
              {tabs.length === 0 && !folded && (
                <p className="lead">
                  还没有打开的页面。
                  <br />
                  用上面菜单「站点」，或地址栏里粘贴一个链接。
                </p>
              )}
              {folded
                ? tabs.map((t) => <TabRow key={t.label} tab={t} full={false} />)
                : groups.map((g) => {
                    const openGroup = !foldedSet.has(g.key);
                    const playing = g.tabs.some((t) => t.playing);
                    return (
                      <div className="group" key={g.key}>
                        <div
                          className={openGroup ? 'ghead open' : 'ghead'}
                          role="button"
                          tabIndex={0}
                          onClick={() => toggleGroup(g.key)}
                          onKeyDown={(e) => e.key === 'Enter' && toggleGroup(g.key)}
                          title={openGroup ? `收起 ${g.name} 的 ${g.tabs.length} 个页面` : `展开 ${g.name} 的页面`}
                        >
                          <span className="tw" aria-hidden="true">
                            <Icon name="chevron" />
                          </span>
                          <span className="letter">{badge(g.name)}</span>
                          <span className="gname">{g.name}</span>
                          <span className="gcount">{g.tabs.length}</span>
                          {playing && <span className="beacon on" aria-hidden="true" />}
                        </div>
                        {openGroup && g.tabs.map((t) => <TabRow key={t.label} tab={t} full />)}
                      </div>
                    );
                  })}
            </div>
            <div className="sfoot">
              <button className="sbtn" onClick={fold} title="收起 / 展开这个页面列表">
                <Icon name={folded ? 'expand' : 'fold'} />
                {!folded && <span>收起列表</span>}
              </button>
              <button className="sbtn" onClick={() => setView('settings')} title="设置：常用站点、字体大小、老板键、藏在右下角、运行日志">
                <Icon name="gear" />
                {!folded && <span>设置</span>}
              </button>
            </div>
          </aside>

          <main className="stage">
            {tabs.length === 0 && (
              <div className="none">
                <span className="ring" aria-hidden="true">
                  <Mark />
                </span>
                <p className="big">暂未识别到视频</p>
                <p className="sub">点下面任意一个站点就会在这里打开，也可以在地址栏里粘贴任意视频页面的链接。</p>
                <div className="launch">
                  {sites.map((s) => (
                    <button className="chip" key={s.url} onClick={() => open(s.url)}>
                      {s.name}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {tabs.length > 0 && (!activeTab || activeTab.label === miniTab?.label) && (
              <div className="none">
                <span className="ring float" aria-hidden="true">
                  <Mark />
                </span>
                <p className="big">这一页正在小窗里播放</p>
                <button className="back" onClick={() => call('attach_back')} disabled={!miniTab}>
                  收回主窗口并暂停
                </button>
              </div>
            )}
          </main>

          <aside className="rail">
            {prompt && (
              <button className="ask" title="把当前页发到置顶小窗" onClick={() => toggleMini()}>
                是否开启小窗观看
              </button>
            )}
            <button
              className="rbtn"
              disabled={!miniTab && !activeTab}
              title={miniTab ? '把小窗改成竖屏 9:16' : '以小窗竖屏打开当前页（短视频）'}
              onClick={() => miniShape('port')}
            >
              <Icon name="port" />
              <span className="rl">竖屏</span>
            </button>
            <button
              className="rbtn"
              disabled={!miniTab && !activeTab}
              title={miniTab ? '把小窗改成横屏 16:9' : '以小窗横屏打开当前页（长视频）'}
              onClick={() => miniShape('land')}
            >
              <Icon name="land" />
              <span className="rl">横屏</span>
            </button>
            <button
              className={miniTab ? 'rbtn on' : 'rbtn'}
              disabled={!miniTab}
              title="把小窗里的页面收回主窗口并暂停"
              onClick={() => call('attach_back')}
            >
              <Icon name="inbox" />
              <span className="rl">收回</span>
            </button>
            <span className="rlabel">{miniTab ? '小窗中' : activeTab ? siteKey(activeTab.url).split('.')[0] : '空闲'}</span>
          </aside>
        </div>
      )}

      {modal && <Guide what={modal} onClose={() => setModal(null)} />}
    </div>
  );
}

export default function App() {
  return new URLSearchParams(window.location.search).get('bar') ? <MiniRail /> : <Shell />;
}
