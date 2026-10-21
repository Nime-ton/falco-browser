#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::path::PathBuf;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

// Рекламные и трекерные домены. Список используется дважды:
// 1) в JS (fetch/XHR/script/iframe/img);
// 2) на Windows - как правила DNS движка WebView2 (режет вообще любые запросы к ним).
const AD_HOSTS: &[&str] = &[
    "amazon-adsystem.com", "adnxs.com", "criteo.com", "criteo.net", "taboola.com", "outbrain.com", "mgid.com", "adskeeper.com",
    "exoclick.com", "clickadu.com", "trafficstars.com", "propellerads.com", "popads.net", "a-ads.com", "adsterra.com",
    "highperformanceformat.com", "popcash.net", "onclickads.net", "juicyads.com", "adcash.com", "hilltopads.net",
    "rubiconproject.com", "openx.net", "pubmatic.com", "smartadserver.com", "moatads.com", "adform.net", "adtelligent.com",
    "bidswitch.net", "casalemedia.com", "adsrvr.org",
    "an.yandex.ru", "mc.yandex.ru", "awaps.yandex.net", "yandexadexchange.net", "adfox.ru", "adriver.ru",
    "ad.mail.ru", "top-fwz1.mail.ru", "directadvert.ru", "begun.ru", "marketgid.com", "tns-counter.ru", "yadro.ru",
    "betweendigital.com",
];

// Домены Google: НЕ режем на уровне DNS, иначе YouTube видит сетевые ошибки рекламных запросов
// и наказывает бесконечной буферизацией. Блокируются только в JS и только на сайтах, кроме YouTube.
const GOOGLE_AD_HOSTS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "adservice.google.com", "google-analytics.com",
];

fn js_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "").replace('\r', "")
}

// Скрипт перехода на адрес. save_first = true: перед уходом со страницы она сохраняет
// прокрутку и позицию видео (используется при смене вкладки, чтобы потом их восстановить).
fn nav_script(url: &str, save_first: bool) -> String {
    let save = if save_first { "if (window.__ulSaveForSwitch) window.__ulSaveForSwitch(); " } else { "" };
    format!("{}window.location.href = '{}';", save, js_str(url))
}

fn main() {
    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Falco")
        .build(&event_loop)
        .unwrap();

    let ad_hosts_js = AD_HOSTS.iter().chain(GOOGLE_AD_HOSTS.iter()).map(|h| format!("'{}'", h)).collect::<Vec<_>>().join(",");

    let script = r#"
        (function() {
            try {
                if (window === window.top) window.ipc.postMessage("SYNC_URL|" + window.location.href);

                // 1. БЛОКИРОВЩИК
                const adRegex = /(doubleclick\.net|google-analytics\.com|googlesyndication\.com|adservice\.google\.com|mc\.yandex\.ru|an\.yandex\.ru|amazon-adsystem\.com|appnexus\.com|criteo\.com|taboola\.com|yadro\.ru|top-fwz1\.mail\.ru|propellerads|popads|adk2x|betboom|1xbet|fonbet|melbet|awaps\.|a-ads\.com|mgid\.com|adskeeper|exoclick|clickadu|trafficstars|adfox|adriver|\/pagead\/|\/get_midroll_info)/i;
                const isYT = /(^|\.)youtube(-nocookie)?\.com$/i.test(window.location.hostname);
                // fetch() часто получает объект Request, а не строку - берём из него .url
                function urlOf(x) {
                    try { return typeof x === 'string' ? x : (x && x.url) ? x.url : String(x); } catch (e) { return ''; }
                }
                const AD_HOSTS = [__AD_HOSTS__];
                function hostBlocked(h) {
                    for (let i = 0; i < AD_HOSTS.length; i++) {
                        const d = AD_HOSTS[i];
                        if (h === d || h.endsWith('.' + d)) return true;
                    }
                    return false;
                }
                function isBlocked(x) {
                    if (isYT) return false; // на YouTube сетевые запросы не трогаем: он реагирует на сбои буферизацией
                    const u = urlOf(x);
                    if (!u || u.startsWith('data:') || u.startsWith('blob:')) return false;
                    if (adRegex.test(u)) return true;
                    try { return hostBlocked(new URL(u, window.location.href).hostname.toLowerCase()); } catch (e) { return false; }
                }

                const originalFetch = window.fetch;
                window.fetch = async function(...args) {
                    if (isBlocked(args[0])) return new Response(null, { status: 204 });
                    return originalFetch.apply(this, args);
                };

                const originalXhrOpen = XMLHttpRequest.prototype.open;
                XMLHttpRequest.prototype.open = function(method, url, ...rest) {
                    if (isBlocked(url)) return originalXhrOpen.apply(this, [method, 'data:text/plain,', ...rest]);
                    return originalXhrOpen.apply(this, [method, url, ...rest]);
                };

                window.open = function() { return null; };

                // ---- YouTube: вырезаем рекламу из данных ДО того, как плеер и страница её увидят ----

                function setupYouTube() {
                    // По умолчанию выключено: YouTube грузит родной поток (VP9/AV1) без обходных путей.
                    // Включайте true только на ПК без аппаратного декодера VP9/AV1 (тогда видео тормозит на CPU).
                    const FORCE_H264 = false;
                    if (FORCE_H264) {
                        const badCodec = /av01|vp0?9/i;
                        if (window.MediaSource && MediaSource.isTypeSupported) {
                            const origSupported = MediaSource.isTypeSupported.bind(MediaSource);
                            MediaSource.isTypeSupported = function(type) { return badCodec.test(String(type)) ? false : origSupported(type); };
                        }
                        const origCanPlay = HTMLMediaElement.prototype.canPlayType;
                        HTMLMediaElement.prototype.canPlayType = function(type) { return badCodec.test(String(type)) ? '' : origCanPlay.apply(this, arguments); };
                        // YouTube спрашивает ещё и mediaCapabilities - без этой подмены он выберет VP9 и получит ошибку
                        if (navigator.mediaCapabilities && navigator.mediaCapabilities.decodingInfo) {
                            const origDI = navigator.mediaCapabilities.decodingInfo.bind(navigator.mediaCapabilities);
                            navigator.mediaCapabilities.decodingInfo = function(cfg) {
                                const ct = (cfg && cfg.video && cfg.video.contentType) || '';
                                if (badCodec.test(String(ct))) return Promise.resolve({ supported: false, smooth: false, powerEfficient: false });
                                return origDI(cfg);
                            };
                        }
                    }

                    const PLAYER_AD_KEYS = ['adPlacements', 'playerAds', 'adSlots', 'adBreakHeartbeatParams'];

                    // Быстрая точечная очистка ТОЛЬКО ответа плеера (выполняется за <0.1 мс вместо обхода 40 уровней вложенности)
                    function prunePlayerAds(data) {
                        if (!data || typeof data !== 'object') return data;
                        for (const k of PLAYER_AD_KEYS) {
                            if (k in data) delete data[k];
                        }
                        if (data.playerResponse && typeof data.playerResponse === 'object') {
                            for (const k of PLAYER_AD_KEYS) {
                                if (k in data.playerResponse) delete data.playerResponse[k];
                            }
                        }
                        return data;
                    }

                    // 1) Мгновенная очистка стартовых данных плеера (ytInitialData не трогаем — баннеры в ленте скроет CSS без тормозов)
                    let playerRespValue;
                    try {
                        Object.defineProperty(window, 'ytInitialPlayerResponse', {
                            configurable: true,
                            get() { return playerRespValue; },
                            set(v) { try { playerRespValue = prunePlayerAds(v); } catch (e) { playerRespValue = v; } }
                        });
                    } catch (e) {}

                    // 2) Перехват только JSON-ответов самого плеера (/youtubei/v1/player) без глобального хука на JSON.parse
                    const YT_PLAYER_API = /\/youtubei\/v1\/player/;
                    const origJson = Response.prototype.json;
                    Response.prototype.json = function() {
                        const p = origJson.apply(this, arguments);
                        if (!YT_PLAYER_API.test(this.url || '')) return p;
                        return p.then((d) => { try { return prunePlayerAds(d); } catch (e) { return d; } });
                    };

                    // 3) Запасной турбо-скипер (если реклама вшита в поток)
                    let inAd = false, saved = null;
                    setInterval(() => {
                        const player = document.getElementById('movie_player');
                        if (!player) return;
                        const video = player.querySelector('video');

                        const skip = player.querySelector('.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button, .ytp-ad-skip-button-container button');
                        if (skip) skip.click();

                        const adShowing = player.classList.contains('ad-showing') || player.classList.contains('ad-interrupting');
                        if (adShowing && video) {
                            if (!inAd) { inAd = true; saved = { rate: video.playbackRate, muted: video.muted }; }
                            video.muted = true;
                            video.playbackRate = 16;
                            if (isFinite(video.duration) && video.duration > 0) {
                                video.currentTime = video.duration;
                            }
                        } else if (inAd) {
                            inAd = false;
                            if (video && saved) { video.playbackRate = saved.rate; video.muted = saved.muted; }
                            saved = null;
                        }

                        const wall = document.querySelector('ytd-enforcement-message-view-model');
                        if (wall) {
                            const dlg = wall.closest('tp-yt-paper-dialog') || wall;
                            dlg.remove();
                            const backdrop = document.querySelector('tp-yt-iron-overlay-backdrop');
                            if (backdrop) backdrop.remove();
                            if (video && video.paused) { try { video.play().catch(() => {}); } catch (e) {} }
                        }
                    }, 200);
                }
                if (isYT) setupYouTube();

                // ---- Блокировка рекламы на уровне DOM (script / iframe / img), кроме YouTube ----
                if (!isYT) {
                    const patchSrc = (proto, prop) => {
                        const d = Object.getOwnPropertyDescriptor(proto, prop);
                        if (!d || !d.set) return;
                        Object.defineProperty(proto, prop, {
                            configurable: true, enumerable: d.enumerable, get: d.get,
                            set(v) { if (isBlocked(v)) return; d.set.call(this, v); }
                        });
                    };
                    patchSrc(HTMLScriptElement.prototype, 'src');
                    patchSrc(HTMLIFrameElement.prototype, 'src');
                    patchSrc(HTMLImageElement.prototype, 'src');
                    patchSrc(HTMLEmbedElement.prototype, 'src');

                    const SRC_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED', 'OBJECT', 'LINK', 'SOURCE']);
                    const origSetAttr = Element.prototype.setAttribute;
                    Element.prototype.setAttribute = function(name, value) {
                        try {
                            const n = String(name).toLowerCase();
                            if ((n === 'src' || n === 'href' || n === 'data') && SRC_TAGS.has(this.tagName) && isBlocked(value)) return;
                        } catch (e) {}
                        return origSetAttr.apply(this, arguments);
                    };

                    if (navigator.sendBeacon) {
                        const origBeacon = navigator.sendBeacon.bind(navigator);
                        navigator.sendBeacon = function(url, data) { return isBlocked(url) ? true : origBeacon(url, data); };
                    }

                    // Невидимая ссылка target=_blank, которую скрипт "кликает" сам - классика для pop-under рекламы
                    const origAClick = HTMLAnchorElement.prototype.click;
                    HTMLAnchorElement.prototype.click = function() {
                        if (!this.isConnected && this.target === '_blank' && !this.hasAttribute('download')) return;
                        return origAClick.apply(this, arguments);
                    };

                    // Теги, вставленные парсером страницы (не через JS), ловим постфактум
                    const AD_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED']);
                    const scanNode = (n) => {
                        if (!n || n.nodeType !== 1) return;
                        if (AD_TAGS.has(n.tagName) && isBlocked(n.src)) { n.remove(); return; }
                        if (n.firstElementChild) {
                            n.querySelectorAll('script[src],iframe[src],img[src],embed[src]').forEach((el) => { if (isBlocked(el.src)) el.remove(); });
                        }
                    };
                    new MutationObserver((muts) => {
                        for (const m of muts) for (const n of m.addedNodes) scanNode(n);
                    }).observe(document, { childList: true, subtree: true });
                }

                const css = `
                    .b-adv, .b-banner, #branding, .branding, .ad-container, 
                    [id*="yandex_rtb"], [class*="yandex-rtb"],
                    [id^="ad_"], [class^="ad_"], [class*="-ad-"],
                    div[data-ad], div[data-banner],
                    ins.adsbygoogle, [id*="adfox"], [class*="adfox"], [id^="div-gpt-ad"], [id^="google_ads"],
                    iframe[src*="adfox"], iframe[src*="adriver"],
                    a[href*="1xbet"], a[href*="fonbet"], a[href*="casino"] { 
                        display: none !important; opacity: 0 !important; pointer-events: none !important; width: 0 !important; height: 0 !important;
                    }
                `;
                // Запасной CSS для YouTube: прячет то, что успело отрисоваться
                const ytCss = `
                    ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-display-ad-renderer,
                    ytd-promoted-sparkles-web-renderer, ytd-promoted-video-renderer, ytd-compact-promoted-video-renderer,
                    ytd-banner-promo-renderer, ytd-statement-banner-renderer, ytd-primetime-promo-renderer,
                    ytd-action-companion-ad-renderer, ytd-player-legacy-desktop-watch-ads-renderer,
                    ytd-brand-video-singleton-renderer, ytd-brand-video-shelf-renderer,
                    #masthead-ad, #player-ads, .ytp-ad-overlay-container { display: none !important; }
                    ytd-rich-item-renderer:has(ytd-ad-slot-renderer),
                    ytd-rich-item-renderer:has(ytd-in-feed-ad-layout-renderer) { display: none !important; }
                `;
                function injectCss() {
                    if (!document.head) { requestAnimationFrame(injectCss); return; }
                    if (!isYT && !document.getElementById('ultralight-css')) {
                        const style = document.createElement('style'); style.id = 'ultralight-css'; style.textContent = css; document.head.appendChild(style);
                    }
                    if (isYT && !document.getElementById('ultralight-yt-css')) {
                        const style = document.createElement('style'); style.id = 'ultralight-yt-css'; style.textContent = ytCss; document.head.appendChild(style);
                    }
                }
                injectCss();

                let ui = null; // { container, trigger, tabsRow, input, show, hide }

                // ---- URL-синхронизация ----
                let lastSyncedUrl = window.location.href;

                function syncUrl() {
                    if (window !== window.top) return; // iframes не должны портить список вкладок
                    const href = window.location.href;
                    if (href !== lastSyncedUrl) {
                        lastSyncedUrl = href;
                        window.ipc.postMessage('SYNC_URL|' + href);
                    }
                    if (ui && document.activeElement !== ui.input) ui.input.value = href;
                }
                const onNavigation = syncUrl;

                // Сначала выполняем оригинальный pushState/replaceState, и только потом читаем URL
                const origPush = history.pushState;
                history.pushState = function() { const r = origPush.apply(this, arguments); onNavigation(); return r; };
                const origReplace = history.replaceState;
                history.replaceState = function() { const r = origReplace.apply(this, arguments); onNavigation(); return r; };
                window.addEventListener('popstate', onNavigation);
                window.addEventListener('hashchange', onNavigation);
                window.addEventListener('yt-navigate-finish', onNavigation);

                // 2. ИНТЕРФЕЙС (создаётся один раз, при удалении сайтом просто возвращается в DOM)
                if (window !== window.top) return;

                let tabsState = { list: [], active: 0 };

                function drawTabs() {
                    if (!ui) return;
                    const row = ui.tabsRow;
                    row.replaceChildren(); // НЕ innerHTML: YouTube включает Trusted Types, и innerHTML = '' бросает ошибку

                    tabsState.list.forEach((url, i) => {
                        let tabEl = document.createElement('div');
                        let isActive = (i === tabsState.active);

                        tabEl.style.cssText = `
                            min-width: 120px; max-width: 200px; height: 28px; margin-top: 2px; margin-right: 2px;
                            background: ${isActive ? '#303134' : '#171717'}; color: ${isActive ? '#fff' : '#9aa0a6'};
                            border-radius: 8px 8px 0 0; display: flex; align-items: center; justify-content: space-between;
                            padding: 0 10px; font-size: 13px; cursor: pointer; user-select: none; transition: background 0.2s;
                        `;
                        tabEl.onmouseover = () => { if(!isActive) tabEl.style.background = '#202124'; };
                        tabEl.onmouseout = () => { if(!isActive) tabEl.style.background = '#171717'; };
                        tabEl.onclick = () => window.ipc.postMessage('SWITCH_TAB|' + i);

                        let titleSpan = document.createElement('span');
                        try { titleSpan.textContent = new URL(url).hostname.replace('www.', '') || 'Новая вкладка'; }
                        catch(e) { titleSpan.textContent = 'Загрузка...'; }
                        titleSpan.style.cssText = `overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-grow: 1; margin-right: 8px;`;

                        let closeBtn = document.createElement('span'); closeBtn.textContent = '×';
                        closeBtn.style.cssText = `font-size: 16px; width: 18px; height: 18px; display: flex; align-items: center; justify-content: center; border-radius: 50%; font-weight: bold; cursor: pointer;`;
                        closeBtn.onmouseover = () => closeBtn.style.background = '#5f6368';
                        closeBtn.onmouseout = () => closeBtn.style.background = 'transparent';
                        closeBtn.onclick = (e) => { e.stopPropagation(); window.ipc.postMessage('CLOSE_TAB|' + i); };

                        tabEl.appendChild(titleSpan); tabEl.appendChild(closeBtn); row.appendChild(tabEl);
                    });

                    let newTabBtn = document.createElement('div'); newTabBtn.textContent = '+';
                    newTabBtn.style.cssText = `min-width: 28px; height: 28px; margin-top: 2px; color: #9aa0a6; font-size: 20px; display: flex; align-items: center; justify-content: center; cursor: pointer; border-radius: 50%;`;
                    newTabBtn.onmouseover = () => newTabBtn.style.background = '#303134';
                    newTabBtn.onmouseout = () => newTabBtn.style.background = 'transparent';
                    newTabBtn.onclick = () => window.ipc.postMessage('new_tab');
                    row.appendChild(newTabBtn);
                }

                // Определяем сразу: ответ от Rust может прийти раньше, чем построен интерфейс
                window.renderTabs = function(tabsString, activeIndex) {
                    tabsState = { list: tabsString.split('|||'), active: activeIndex };
                    drawTabs();
                };

                function buildUI() {
                    const container = document.createElement('div'); container.id = 'light-browser-ui';
                    container.style.cssText = `position: fixed; top: -75px; left: 0; width: 100%; height: 75px; background: #202124; z-index: 2147483647; font-family: sans-serif; box-shadow: 0 4px 10px rgba(0,0,0,0.5); display: flex; flex-direction: column; transition: top 0.3s cubic-bezier(0.25, 0.8, 0.25, 1);`;

                    const tabsRow = document.createElement('div'); tabsRow.id = 'light-tabs-row';
                    tabsRow.style.cssText = `display: flex; height: 32px; background: #000; overflow-x: auto; padding: 0 5px;`;

                    const urlRow = document.createElement('div');
                    urlRow.style.cssText = `display: flex; height: 43px; align-items: center; padding: 0 10px; box-sizing: border-box;`;

                    const trigger = document.createElement('div'); trigger.id = 'light-trigger-zone';
                    trigger.style.cssText = `position: fixed; top: 0; left: 0; width: 100%; height: 15px; z-index: 2147483646; background: transparent;`;

                    const input = document.createElement('input'); input.type = 'text'; input.value = window.location.href;
                    input.style.cssText = `flex: 1; min-width: 0; height: 30px; border-radius: 15px; border: none; padding: 0 20px; background: #303134; color: #fff; outline: none; font-size: 14px;`;

                    input.addEventListener('keydown', (e) => {
                        if (e.key === 'Enter') {
                            let query = input.value.trim();
                            if (query.includes('.') && !query.includes(' ')) {
                                if (!query.startsWith('http')) query = 'https://' + query;
                                window.location.href = query;
                            } else { window.location.href = 'https://duckduckgo.com/?q=' + encodeURIComponent(query); }
                        }
                    });
                    input.addEventListener('focus', () => input.select());

                    const show = () => {
                        container.style.top = '0';
                        input.value = window.location.href;
                        window.ipc.postMessage('GET_TABS');
                    };
                    const hide = () => { if (document.activeElement !== input) container.style.top = '-75px'; };

                    trigger.addEventListener('mouseenter', show);
                    container.addEventListener('mouseleave', hide);

                    // Кнопки навигации: назад / вперёд / обновить
                    function navButton(symbol, title, onClick) {
                        const b = document.createElement('div');
                        b.textContent = symbol; b.title = title;
                        b.style.cssText = `width: 30px; height: 30px; margin-right: 4px; flex: none; border-radius: 50%; color: #9aa0a6; font-size: 18px; display: flex; align-items: center; justify-content: center; cursor: pointer; user-select: none;`;
                        b.onmouseover = () => { b.style.background = '#303134'; b.style.color = '#fff'; };
                        b.onmouseout = () => { b.style.background = 'transparent'; b.style.color = '#9aa0a6'; };
                        b.onclick = onClick;
                        return b;
                    }
                    urlRow.appendChild(navButton('\u2190', 'Назад (Alt+←)', () => history.back()));
                    urlRow.appendChild(navButton('\u2192', 'Вперёд (Alt+→)', () => history.forward()));
                    urlRow.appendChild(navButton('\u21BB', 'Обновить (F5)', () => location.reload()));

                    container.appendChild(tabsRow); urlRow.appendChild(input); container.appendChild(urlRow);

                    ui = { container, trigger, tabsRow, input, show, hide };
                    drawTabs();
                    window.ipc.postMessage('GET_TABS');
                }

                // ---- Ручное скрытие элементов: Ctrl+Shift+X (выбрать), Ctrl+Shift+Z (отменить последнее) ----
                // Правила хранятся в localStorage и действуют только на этом сайте (адресе).
                const RULES_KEY = 'ultralight_hidden';
                let userRules = [];
                try { userRules = JSON.parse(localStorage.getItem(RULES_KEY) || '[]'); } catch (e) { userRules = []; }
                if (!Array.isArray(userRules)) userRules = [];
                function saveRules() { try { localStorage.setItem(RULES_KEY, JSON.stringify(userRules)); } catch (e) {} }
                function ensureUserCss() {
                    if (!document.head) return;
                    let st = document.getElementById('ultralight-user-css');
                    if (!st) { st = document.createElement('style'); st.id = 'ultralight-user-css'; document.head.appendChild(st); }
                    const rules = userRules.map((s) => s + ' { display: none !important; }').join('\n');
                    if (st.textContent !== rules) st.textContent = rules;
                }
                (function applyWhenReady() {
                    if (!document.head) { requestAnimationFrame(applyWhenReady); return; }
                    ensureUserCss();
                })();

                function toast(msg) {
                    if (!document.documentElement) return;
                    const t = document.createElement('div');
                    t.textContent = msg;
                    t.style.cssText = 'position:fixed;left:16px;bottom:16px;max-width:60%;padding:10px 14px;background:#202124;color:#fff;font:13px sans-serif;border-radius:8px;z-index:2147483647;box-shadow:0 2px 10px rgba(0,0,0,.5);pointer-events:none;';
                    document.documentElement.appendChild(t);
                    setTimeout(() => t.remove(), 3500);
                }

                // Имена вроде b46fec00c0 или b-post__g0d7f9017fbdajr53w82a сайт генерирует случайно (чтобы обходить блокировщики):
                // на них нельзя опираться в сохраняемом правиле, после перезагрузки они уже другие
                const looksRandom = (s) => s.length >= 8 && /\d/.test(s) && /[A-Za-z]/.test(s);
                function cssPath(el) {
                    const parts = [];
                    while (el && el.nodeType === 1 && el !== document.documentElement && parts.length < 5) {
                        if (el.id && /^[A-Za-z][\w-]{2,}$/.test(el.id) && !looksRandom(el.id)
                            && document.querySelectorAll('#' + CSS.escape(el.id)).length === 1) {
                            parts.unshift('#' + CSS.escape(el.id));
                            break;
                        }
                        let part = el.tagName.toLowerCase();
                        const cls = Array.from(el.classList).filter((c) => /^[A-Za-z_-][\w-]{2,}$/.test(c) && !looksRandom(c)).slice(0, 2);
                        if (cls.length) part += '.' + cls.map((c) => CSS.escape(c)).join('.');
                        const parent = el.parentElement;
                        if (parent) {
                            const same = Array.from(parent.children).filter((c) => c.tagName === el.tagName);
                            if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
                        }
                        parts.unshift(part);
                        el = el.parentElement;
                    }
                    return parts.join(' > ');
                }

                let picking = false, pickEl = null, hl = null;
                const isOurs = (el) => !el || el.id === 'light-browser-ui' || el.id === 'light-trigger-zone' || (ui && ui.container.contains(el));
                function highlight(el) {
                    pickEl = el;
                    if (!hl) {
                        hl = document.createElement('div');
                        hl.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483645;outline:2px solid #ff4d4f;background:rgba(255,77,79,.18);';
                        document.documentElement.appendChild(hl);
                    }
                    const r = el.getBoundingClientRect();
                    hl.style.left = r.left + 'px'; hl.style.top = r.top + 'px';
                    hl.style.width = r.width + 'px'; hl.style.height = r.height + 'px';
                }
                const swallow = (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); };
                // События мыши внутри iframe не доходят до родительской страницы, поэтому на время выбора
                // над всей страницей лежит прозрачный слой. Чтобы узнать, что под ним, слой на миг делаем "прозрачным для мыши".
                let pickLayer = null;
                const onPickMove = (e) => {
                    if (pickLayer) pickLayer.style.pointerEvents = 'none';
                    const el = document.elementFromPoint(e.clientX, e.clientY);
                    if (pickLayer) pickLayer.style.pointerEvents = 'auto';
                    if (el && !isOurs(el)) highlight(el);
                };
                const onPickClick = (e) => {
                    swallow(e);
                    const el = pickEl;
                    stopPick();
                    if (!el) return;
                    if (el === document.body || el === document.documentElement) { toast('Нельзя скрыть всю страницу'); return; }
                    const sel = cssPath(el);
                    if (!sel) return;
                    userRules.push(sel); saveRules(); ensureUserCss();
                    try { console.log('[Falco] скрыт элемент:', sel, String(el.outerHTML).slice(0, 400)); } catch (e2) {}
                    toast('Скрыто <' + el.tagName.toLowerCase() + '>: ' + sel + '   (Ctrl+Shift+Z - отменить)');
                };
                const onPickKey = (e) => {
                    if (e.key === 'Escape') { swallow(e); stopPick(); toast('Отменено'); }
                    else if (e.key === 'ArrowUp' && pickEl && pickEl.parentElement && pickEl.parentElement !== document.documentElement) {
                        swallow(e); highlight(pickEl.parentElement);
                    }
                };
                const PICK_BLOCKED = ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'auxclick', 'contextmenu'];
                function startPick() {
                    if (picking) return;
                    picking = true; pickEl = null;
                    pickLayer = document.createElement('div');
                    pickLayer.style.cssText = 'position:fixed;left:0;top:0;width:100%;height:100%;z-index:2147483644;cursor:crosshair;background:transparent;pointer-events:auto;';
                    document.documentElement.appendChild(pickLayer);
                    // Если фокус остался внутри iframe, клавиши (Esc, стрелка вверх) не дойдут до страницы
                    try { if (document.activeElement && document.activeElement.blur) document.activeElement.blur(); window.focus(); } catch (e) {}
                    window.addEventListener('mousemove', onPickMove, true);
                    window.addEventListener('click', onPickClick, true);
                    PICK_BLOCKED.forEach((t) => window.addEventListener(t, swallow, true));
                    window.addEventListener('keydown', onPickKey, true);
                    toast('Кликните по элементу, который нужно скрыть. Стрелка вверх - выбрать родителя, Esc - отмена');
                }
                function stopPick() {
                    picking = false; pickEl = null;
                    window.removeEventListener('mousemove', onPickMove, true);
                    if (pickLayer) { pickLayer.remove(); pickLayer = null; }
                    window.removeEventListener('click', onPickClick, true);
                    PICK_BLOCKED.forEach((t) => window.removeEventListener(t, swallow, true));
                    window.removeEventListener('keydown', onPickKey, true);
                    if (hl) { hl.remove(); hl = null; }
                }
                function undoRule() {
                    const s = userRules.pop();
                    saveRules(); ensureUserCss();
                    toast(s ? 'Правило удалено: ' + s : 'Для этого сайта нет сохранённых правил');
                }

                // ---- Сохранение прокрутки и позиции видео при переключении вкладок ----
                // Состояние лежит в localStorage сайта, по ключу страницы. Перед сменой вкладки Rust вызывает
                // window.__ulSaveForSwitch(); когда вы возвращаетесь на ту же страницу, позиция восстанавливается
                // один раз. При обычных переходах (ссылка, ввод адреса, F5) ничего не восстанавливается.
                const STATE_KEY = 'ultralight_tabstate';
                const STATE_TTL = 12 * 60 * 60 * 1000;
                const STATE_MAX = 60;
                let scrollDone = true, videoDone = true; // false, пока идёт восстановление
                let switching = false;
                let scrollTimer = null;

                function stateKey() {
                    try {
                        const u = new URL(window.location.href);
                        if (/(^|\.)youtube\.com$/i.test(u.hostname) && u.pathname === '/watch') return u.origin + '/watch?v=' + (u.searchParams.get('v') || '');
                        return u.origin + u.pathname + u.search; // без #фрагмента
                    } catch (e) { return window.location.href; }
                }
                function readStates() {
                    try {
                        const o = JSON.parse(localStorage.getItem(STATE_KEY) || '{}');
                        return (o && typeof o === 'object' && !Array.isArray(o)) ? o : {};
                    } catch (e) { return {}; }
                }
                function writeStates(o) {
                    try {
                        const keys = Object.keys(o);
                        if (keys.length > STATE_MAX) {
                            keys.sort((a, b) => (o[a].ts || 0) - (o[b].ts || 0));
                            keys.slice(0, keys.length - STATE_MAX).forEach((k) => { delete o[k]; });
                        }
                        localStorage.setItem(STATE_KEY, JSON.stringify(o));
                    } catch (e) {}
                }
                const adActive = () => !!document.querySelector('#movie_player.ad-showing, #movie_player.ad-interrupting');
                function mainVideo() {
                    const vids = document.querySelectorAll('video');
                    for (let i = 0; i < vids.length; i++) {
                        const v = vids[i];
                        if (isFinite(v.duration) && v.duration > 60 && v.readyState > 0 && !adActive()) return v;
                    }
                    return null;
                }

                function saveState() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const key = stateKey();
                    const states = readStates();
                    const old = states[key];
                    let x = Math.round(window.scrollX), y = Math.round(window.scrollY), t = 0;
                    const v = mainVideo();
                    if (v && !window.location.pathname.startsWith('/shorts') && v.currentTime > 5 && v.currentTime < v.duration - 10) {
                        t = Math.floor(v.currentTime);
                    }
                    // Пока восстановление не закончено, не затираем ещё не применённые сохранённые значения
                    if (old) {
                        if (!scrollDone) { x = old.x || 0; y = old.y || 0; }
                        if (!videoDone) { t = old.t || 0; }
                    }
                    if (!x && !y && !t) delete states[key];
                    else states[key] = { x: x, y: y, t: t, sw: switching, ts: Date.now() };
                    writeStates(states);
                }

                function startScrollRestore(x, y) {
                    let ticks = 0, stable = 0, stopped = false, timer = null;
                    const INPUT_EVENTS = ['wheel', 'touchstart', 'mousedown', 'keydown'];
                    const stop = () => {
                        if (stopped) return;
                        stopped = true; scrollDone = true;
                        clearInterval(timer);
                        INPUT_EVENTS.forEach((ev) => window.removeEventListener(ev, stop, true));
                    };
                    // Любое действие пользователя отменяет автоматическую прокрутку
                    INPUT_EVENTS.forEach((ev) => window.addEventListener(ev, stop, { capture: true, passive: true }));
                    timer = setInterval(() => {
                        ticks++;
                        const root = document.documentElement;
                        const h = Math.max(root ? root.scrollHeight : 0, document.body ? document.body.scrollHeight : 0);
                        const maxY = h - window.innerHeight;
                        if (maxY >= y - 2) {
                            window.scrollTo(x, y);
                            if (Math.abs(window.scrollY - y) <= 3) stable++; else stable = 0;
                            if (stable >= 3) stop();
                        } else if (maxY > 0) {
                            window.scrollTo(x, maxY); // страница ещё короткая: тянем вниз, чтобы подгрузились ленты
                        }
                        if (ticks >= 50) stop(); // ~7.5 секунд
                    }, 150);
                }

                function startVideoRestore(t) {
                    let finished = false, giveUp = null;
                    const onMeta = (e) => {
                        const v = e.target;
                        if (!(v instanceof HTMLVideoElement)) return;
                        if (!isFinite(v.duration) || v.duration < 60 || adActive()) return;
                        if (t < v.duration - 10) {
                            const p = document.getElementById('movie_player');
                            try { if (p && typeof p.seekTo === 'function') p.seekTo(t, true); else v.currentTime = t; } catch (err) {}
                        }
                        finish();
                    };
                    const finish = () => {
                        if (finished) return;
                        finished = true; videoDone = true;
                        document.removeEventListener('loadedmetadata', onMeta, true);
                        clearTimeout(giveUp);
                    };
                    giveUp = setTimeout(finish, 30000);
                    document.addEventListener('loadedmetadata', onMeta, true); // media-события не всплывают, поэтому capture
                }

                (function restoreOnLoad() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const states = readStates();
                    const st = states[stateKey()];
                    if (!st || !st.sw || Date.now() - (st.ts || 0) > STATE_TTL) return;
                    st.sw = false; writeStates(states); // флаг сброшен: перезагрузка или переход по ссылке не восстанавливают
                    if (st.y > 0 || st.x > 0) { scrollDone = false; startScrollRestore(st.x || 0, st.y || 0); }
                    if (st.t > 5 && !window.location.pathname.startsWith('/shorts')) { videoDone = false; startVideoRestore(st.t); }
                })();

                window.addEventListener('scroll', () => {
                    if (scrollTimer) clearTimeout(scrollTimer);
                    const k = stateKey();
                    scrollTimer = setTimeout(() => { scrollTimer = null; if (stateKey() === k) saveState(); }, 400);
                }, { passive: true });
                window.addEventListener('pagehide', () => saveState());
                document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') saveState(); });
                // Вызывается из Rust прямо перед сменой вкладки
                window.__ulSaveForSwitch = () => { switching = true; saveState(); };

                // Горячие клавиши регистрируются ОДИН раз (раньше каждая пересборка UI добавляла ещё один обработчик)
                window.addEventListener('keydown', (e) => {
                    if (!ui) return;
                    const k = e.key.toLowerCase();
                    // Alt+← / Alt+→ и F5 / Ctrl+R
                    if (e.altKey && !e.ctrlKey) {
                        if (e.key === 'ArrowLeft') { e.preventDefault(); history.back(); }
                        else if (e.key === 'ArrowRight') { e.preventDefault(); history.forward(); }
                        return;
                    }
                    if (e.key === 'F5' || (e.ctrlKey && !e.shiftKey && k === 'r')) { e.preventDefault(); location.reload(); return; }
                    if (e.ctrlKey && e.shiftKey && !e.altKey) {
                        if (k === 'x') { e.preventDefault(); startPick(); return; }
                        if (k === 'z') { e.preventDefault(); undoRule(); return; }
                    }
                    if (!e.ctrlKey) return;
                    if (k === 'l') { e.preventDefault(); ui.show(); ui.input.focus(); }
                    else if (k === 't') { e.preventDefault(); window.ipc.postMessage('new_tab'); }
                    else if (k === 'w') { e.preventDefault(); window.ipc.postMessage('CLOSE_TAB|-1'); }
                    else if (k === 'tab') { e.preventDefault(); window.ipc.postMessage('next_tab'); }
                }, true);

                // Возвращает уже существующие элементы в DOM (а не создаёт новые)
                function attachUI() {
                    const root = document.documentElement;
                    if (!root) return;
                    if (!ui) buildUI();
                    if (!ui.trigger.isConnected) root.appendChild(ui.trigger);
                    if (!ui.container.isConnected) root.appendChild(ui.container);
                }

                // MutationObserver возвращает панель мгновенно, как только сайт её убрал
                let observedRoot = null;
                function watch() {
                    const root = document.documentElement;
                    if (!root) { requestAnimationFrame(watch); return; }
                    attachUI();
                    if (root === observedRoot) return;
                    observedRoot = root;
                    new MutationObserver(() => {
                        if (!ui || !ui.container.isConnected || !ui.trigger.isConnected) attachUI();
                    }).observe(root, { childList: true });
                }
                watch();

                // Запасной сторож: на случай, если сайт заменил сам documentElement
                setInterval(() => {
                    if (!ui || !ui.container.isConnected || !ui.trigger.isConnected || document.documentElement !== observedRoot) watch();
                    injectCss();
                    ensureUserCss();
                }, 1000);

            } catch (err) { console.error("Script error: ", err); }
        })();
    "#.replace("__AD_HOSTS__", &ad_hosts_js);

// Получаем путь C:\Users\<Имя>\AppData\Local\Falco\User Data
    let profile_path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("Falco")
        .join("User Data");

    let mut web_context = WebContext::new(Some(profile_path));

        // перед созданием builder
    let start_url = std::env::args()
        .skip(1)
        .find_map(|a| a.strip_prefix("--app=").map(|s| s.trim_matches('"').to_string()))
        .or_else(|| std::env::args().skip(1).find(|a| a.starts_with("http://") || a.starts_with("https://")))
        .unwrap_or_else(|| "https://duckduckgo.com".to_string());

    let builder = WebViewBuilder::new(&window)
        .with_autoplay(true)
        .with_devtools(false)
        .with_web_context(&mut web_context)
        .with_initialization_script(&script)
        .with_ipc_handler(move |msg: String| {
            let _ = proxy.send_event(msg);
        })
        .with_url(&start_url).unwrap();

    // Windows: домены из AD_HOSTS не резолвятся вообще (script, iframe, img, XHR - всё).
    // Стандартные аргументы wry указаны явно, т.к. свои аргументы их заменяют.
    #[cfg(windows)]
    let builder = {
        let rules = AD_HOSTS
            .iter()
            .map(|h| format!("MAP {h} ~NOTFOUND, MAP *.{h} ~NOTFOUND"))
            .collect::<Vec<_>>()
            .join(", ");
        builder.with_additional_browser_args(format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required --host-resolver-rules=\"{}\"",
            rules
        ))
    };

    let webview = builder.build().unwrap();

    let mut tabs: Vec<String> = vec![start_url.clone()];
    let mut current_tab = 0;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(msg) => {
                if msg.starts_with("SYNC_URL|") {
                    let url = msg.replace("SYNC_URL|", "");
                    tabs[current_tab] = url;
                } else if msg == "GET_TABS" {
                    let tabs_str = tabs.join("|||");
                    let js = format!("if(window.renderTabs) window.renderTabs('{}', {});", js_str(&tabs_str), current_tab);
                    let _ = webview.evaluate_script(&js);
                } else if msg == "new_tab" {
                    tabs.push("https://duckduckgo.com".to_string());
                    current_tab = tabs.len() - 1;
                    let _ = webview.evaluate_script(&nav_script("https://duckduckgo.com", true));
                } else if msg.starts_with("SWITCH_TAB|") {
                    if let Ok(idx) = msg.replace("SWITCH_TAB|", "").parse::<usize>() {
                        if idx < tabs.len() {
                            current_tab = idx;
                            let target_url = &tabs[current_tab];
                            let _ = webview.evaluate_script(&nav_script(target_url, true));
                        }
                    }
                } else if msg.starts_with("CLOSE_TAB|") {
                    let mut idx_to_close = current_tab;
                    let parsed = msg.replace("CLOSE_TAB|", "").parse::<i32>().unwrap_or(-1);
                    if parsed >= 0 { idx_to_close = parsed as usize; }
                    
                    if tabs.len() > 1 && idx_to_close < tabs.len() {
                        let was_active = idx_to_close == current_tab;
                        tabs.remove(idx_to_close);
                        
                        if current_tab > idx_to_close { current_tab -= 1; } 
                        else if current_tab >= tabs.len() { current_tab = tabs.len() - 1; }
                        
                        if was_active {
                            let target_url = &tabs[current_tab];
                            let _ = webview.evaluate_script(&nav_script(target_url, false));
                        } else {
                            let tabs_str = tabs.join("|||");
                            let js = format!("if(window.renderTabs) window.renderTabs('{}', {});", js_str(&tabs_str), current_tab);
                            let _ = webview.evaluate_script(&js);
                        }
                    } else if tabs.len() <= 1 {
                        *control_flow = ControlFlow::Exit;
                    }
                } else if msg == "next_tab" {
                    if tabs.len() > 1 {
                        current_tab = (current_tab + 1) % tabs.len();
                        let target_url = &tabs[current_tab];
                        let _ = webview.evaluate_script(&nav_script(target_url, true));
                    }
                }
            }
            _ => {}
        }
    });
}