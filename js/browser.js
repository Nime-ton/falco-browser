(function() {
            try {
                const PERF = __PERF__;
                const NATIVE = __NATIVE__;
                let searchPrefix = '__SEARCH_URL__';
                let pinTopBar = __PIN_BAR__;

                // ---- Защищённый канал связи с Rust ----
                const IPC_T = '__IPC_TOKEN__';
                let rawPost = null;
                try {
                    if (window.chrome && window.chrome.webview && typeof window.chrome.webview.postMessage === 'function') {
                        rawPost = window.chrome.webview.postMessage.bind(window.chrome.webview);
                    } else if (window.ipc && typeof window.ipc.postMessage === 'function') {
                        rawPost = window.ipc.postMessage.bind(window.ipc);
                    }
                } catch (e) {}
                const ipcPost = (m) => { if (rawPost) { try { rawPost(IPC_T + ':' + m); } catch (e) {} } };
                const lockGlobal = (name, fn) => {
                    try { Object.defineProperty(window, name, { value: fn, writable: false, configurable: false, enumerable: false }); } catch (e) {}
                };
                const rApply = Reflect.apply;
                const rAttachShadow = Element.prototype.attachShadow;
                const rJsonParse = JSON.parse;
                const perfT0 = performance.now();
                // id этого документа: Rust принимает SYNC_URL/TITLE только от документа, объявившегося через LOAD_URL
                const DOC_ID = Math.random().toString(36).slice(2, 12) + Date.now().toString(36);
                if (window === window.top) ipcPost('LOAD_URL|' + DOC_ID + '|' + window.location.href);

                // 1. БЛОКИРОВЩИК
                const adRegex = /(doubleclick\.net|google-analytics\.com|googlesyndication\.com|adservice\.google\.com|mc\.yandex\.ru|an\.yandex\.ru|amazon-adsystem\.com|appnexus\.com|criteo\.com|taboola\.com|yadro\.ru|top-fwz1\.mail\.ru|propellerads|popads|adk2x|betboom|1xbet|fonbet|melbet|awaps\.|a-ads\.com|mgid\.com|adskeeper|exoclick|clickadu|trafficstars|adfox|adriver|\/pagead\/|\/get_midroll_info)/i;
                const isYT = /(^|\.)youtube(-nocookie)?\.com$/i.test(window.location.hostname);
                function urlOf(x) {
                    try { return typeof x === 'string' ? x : (x && x.url) ? x.url : String(x); } catch (e) { return ''; }
                }
                let AD_SET = null;
                function hostBlocked(h) {
                    if (!AD_SET) AD_SET = new Set('__AD_HOSTS__'.split(','));
                    while (h) {
                        if (AD_SET.has(h)) return true;
                        const i = h.indexOf('.');
                        if (i < 0) return false;
                        h = h.slice(i + 1);
                    }
                    return false;
                }
                function isBlocked(x) {
                    if (isYT) return false;
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

                // ---- Новые окна сайта (target="_blank" и window.open) открываем новой вкладкой этого браузера ----
                const perfNow = performance.now.bind(performance);
                const ulLastOpen = { u: '', t: -1e9 };
                let lastTrustedGestureTime = -1e9;
                let lastGestureInteractive = false;

                function isInteractiveTarget(el) {
                    if (!el || el.nodeType !== 1) return false;
                    if (el.closest && el.closest('a, area, button, input, select, textarea, summary, [role="button"], [role="link"], [tabindex]')) return true;
                    try { if (window.getComputedStyle(el).cursor === 'pointer') return true; } catch (e) {}
                    return false;
                }

                const recordGesture = (e) => {
                    if (!e || !e.isTrusted) return;
                    if (e.type === 'keydown') {
                        if (e.key !== 'Enter' && e.key !== ' ') return;
                        lastGestureInteractive = isInteractiveTarget(e.target);
                    } else {
                        if (e.type === 'auxclick' && e.button !== 1) return;
                        if (e.type === 'click' && e.button !== 0) return;
                        lastGestureInteractive = isInteractiveTarget(e.target);
                    }
                    lastTrustedGestureTime = perfNow();
                };
                window.addEventListener('click', recordGesture, true);
                window.addEventListener('auxclick', recordGesture, true);
                window.addEventListener('keydown', recordGesture, true);

                function userGesture(e) {
                    if (e && e.isTrusted) return true;
                    return lastGestureInteractive && (perfNow() - lastTrustedGestureTime) < 150;
                }

                function openInTab(u, background) {
                    try {
                        const abs = new URL(u, window.location.href).href;
                        if (!/^https?:\/\//i.test(abs) || !rawPost) return false;
                        const now = perfNow();
                        lastTrustedGestureTime = -1e9;
                        lastGestureInteractive = false;
                        if ((abs === ulLastOpen.u && now - ulLastOpen.t < 500) || now - ulLastOpen.t < 120) return true;
                        ulLastOpen.u = abs; ulLastOpen.t = now;
                        ipcPost((background ? 'OPEN_TAB_BG|' : 'OPEN_TAB|') + abs);
                        return true;
                    } catch (e) { return false; }
                }

                // window.open с размерами окна (так открываются окна входа Google/Apple/GitHub и 3-D Secure)
                // идёт в настоящее окно-попап (Rust: popups.rs), чтобы работали window.opener, postMessage и window.close().
                // Всё остальное - вкладка, как раньше.
                const nativeOpen = window.open;
                const popupLike = (f) => typeof f === 'string' && /(^|[\s,])(popup|width|height)\b/i.test(f);
                window.open = function(u, name, features) {
                    if (!userGesture(null)) return null;
                    if (popupLike(features) && typeof nativeOpen === 'function') {
                        let target = '';
                        try {
                            target = (u === undefined || u === null || u === '') ? 'about:blank' : new URL(String(u), window.location.href).href;
                        } catch (e) { return null; }
                        if (target !== 'about:blank' && !/^https?:\/\//i.test(target)) return null;
                        lastTrustedGestureTime = -1e9;
                        lastGestureInteractive = false;
                        try { return nativeOpen.call(window, target === 'about:blank' ? '' : target, name, features); } catch (e) { return null; }
                    }
                    if (u) openInTab(String(u), false);
                    return null;
                };

                const origElemClick = HTMLElement.prototype.click;
                HTMLElement.prototype.click = function() {
                    const tag = this.tagName;
                    if (tag === 'A' || tag === 'AREA') {
                        const tgt = (this.target || '').toLowerCase();
                        if ((!this.isConnected || tgt === '_blank') && !this.hasAttribute('download') && !userGesture(null)) return;
                    }
                    return origElemClick.apply(this, arguments);
                };

                const origFormSubmit = HTMLFormElement.prototype.submit;
                HTMLFormElement.prototype.submit = function() {
                    if (String(this.getAttribute('target') || '').toLowerCase() === '_blank' && !userGesture(null)) return;
                    return origFormSubmit.apply(this, arguments);
                };
                document.addEventListener('submit', (e) => {
                    const f = e.target;
                    if (f && f.getAttribute && String(f.getAttribute('target') || '').toLowerCase() === '_blank' && !userGesture(e)) {
                        e.preventDefault();
                        e.stopImmediatePropagation();
                    }
                }, true);

                function onLinkClick(e) {
                    if (e.defaultPrevented) return;
                    const mid = e.type === 'auxclick' && e.button === 1;
                    const left = e.type === 'click' && e.button === 0;
                    if (!mid && !left) return;
                    const a = e.target && e.target.closest ? e.target.closest('a[href], area[href]') : null;
                    if (!a || a.hasAttribute('download')) return;
                    const blank = (a.getAttribute('target') || '').toLowerCase() === '_blank';
                    let mode = null;
                    if (mid || (left && (e.ctrlKey || e.metaKey))) mode = 'bg';
                    else if (left && !e.altKey && (e.shiftKey || blank)) mode = 'fg';
                    if (!mode) return;
                    if (!userGesture(e)) {
                        e.preventDefault();
                        e.stopImmediatePropagation();
                        return;
                    }
                    const href = typeof a.href === 'string' ? a.href : a.getAttribute('href');
                    if (openInTab(href, mode === 'bg')) e.preventDefault();
                }
                document.addEventListener('click', onLinkClick, true);
                document.addEventListener('auxclick', onLinkClick, true);

                // ---- YouTube ----
                function setupYouTube() {
                    const FORCE_H264 = __YT_H264__;
                    if (FORCE_H264) {
                        const badCodec = /av01|vp0?9/i;
                        if (window.MediaSource && MediaSource.isTypeSupported) {
                            const origSupported = MediaSource.isTypeSupported.bind(MediaSource);
                            MediaSource.isTypeSupported = function(type) { return badCodec.test(String(type)) ? false : origSupported(type); };
                        }
                        const origCanPlay = HTMLMediaElement.prototype.canPlayType;
                        HTMLMediaElement.prototype.canPlayType = function(type) { return badCodec.test(String(type)) ? '' : origCanPlay.apply(this, arguments); };
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

                    let playerRespValue;
                    try {
                        Object.defineProperty(window, 'ytInitialPlayerResponse', {
                            configurable: true,
                            get() { return playerRespValue; },
                            set(v) { try { playerRespValue = prunePlayerAds(v); } catch (e) { playerRespValue = v; } }
                        });
                    } catch (e) {}

                    const YT_PLAYER_API = /\/youtubei\/v1\/player/;
                    const origJson = Response.prototype.json;
                    Response.prototype.json = function() {
                        const p = origJson.apply(this, arguments);
                        if (!YT_PLAYER_API.test(this.url || '')) return p;
                        return p.then((d) => { try { return prunePlayerAds(d); } catch (e) { return d; } });
                    };

                    let inAd = false, saved = null, adTimer = null, watchedPlayer = null, playerObs = null;
                    const isAdClass = (pl) => pl.classList.contains('ad-showing') || pl.classList.contains('ad-interrupting');
                    function adStep() {
                        const player = document.getElementById('movie_player');
                        if (!player) return;
                        const video = player.querySelector('video');

                        const skip = player.querySelector('.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button, .ytp-ad-skip-button-container button');
                        if (skip) skip.click();

                        if (isAdClass(player) && video) {
                            if (!inAd) { inAd = true; saved = { rate: video.playbackRate, muted: video.muted }; }
                            video.muted = true;
                            video.playbackRate = 16;
                            if (isFinite(video.duration) && video.duration > 0) {
                                video.currentTime = video.duration;
                            }
                            if (!adTimer) adTimer = setInterval(adStep, 200);
                        } else {
                            if (inAd) {
                                inAd = false;
                                if (video && saved) { video.playbackRate = saved.rate; video.muted = saved.muted; }
                                saved = null;
                            }
                            if (adTimer) { clearInterval(adTimer); adTimer = null; }
                        }
                    }
                    function removeWall() {
                        const wall = document.querySelector('ytd-enforcement-message-view-model');
                        if (!wall) return;
                        const dlg = wall.closest('tp-yt-paper-dialog') || wall;
                        dlg.remove();
                        const backdrop = document.querySelector('tp-yt-iron-overlay-backdrop');
                        if (backdrop) backdrop.remove();
                        const v = document.querySelector('#movie_player video');
                        if (v && v.paused) { try { v.play().catch(() => {}); } catch (e) {} }
                    }
                    function ytWatch() {
                        if (document.hidden) return;
                        const player = document.getElementById('movie_player');
                        if (player && player !== watchedPlayer) {
                            watchedPlayer = player;
                            if (playerObs) playerObs.disconnect();
                            playerObs = new MutationObserver(() => { if (inAd || isAdClass(player)) adStep(); });
                            playerObs.observe(player, { attributes: true, attributeFilter: ['class'] });
                            adStep();
                        }
                        removeWall();
                    }
                    setInterval(ytWatch, 1000);
                    window.addEventListener('yt-navigate-finish', ytWatch);
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
                        if (typeof name === 'string' && (name.length === 3 || name.length === 4) && SRC_TAGS.has(this.tagName)) {
                            try {
                                const n = name.toLowerCase();
                                if ((n === 'src' || n === 'href' || n === 'data') && isBlocked(value)) return;
                            } catch (e) {}
                        }
                        return origSetAttr.apply(this, arguments);
                    };

                    if (navigator.sendBeacon) {
                        const origBeacon = navigator.sendBeacon.bind(navigator);
                        navigator.sendBeacon = function(url, data) { return isBlocked(url) ? true : origBeacon(url, data); };
                    }

                    const origAClick = HTMLAnchorElement.prototype.click;
                    HTMLAnchorElement.prototype.click = function() {
                        if (!this.isConnected && this.target === '_blank' && !this.hasAttribute('download')) return;
                        return origAClick.apply(this, arguments);
                    };

                    const AD_TAGS = new Set(['SCRIPT', 'IFRAME', 'IMG', 'EMBED']);
                    const scanNode = (n) => {
                        if (!n || n.nodeType !== 1) return;
                        if (AD_TAGS.has(n.tagName) && isBlocked(n.src)) { n.remove(); return; }
                        if (n.firstElementChild) {
                            n.querySelectorAll('script[src],iframe[src],img[src],embed[src]').forEach((el) => { if (isBlocked(el.src)) el.remove(); });
                        }
                    };
                    if (!(NATIVE && window === window.top)) {
                        new MutationObserver((muts) => {
                            for (const m of muts) for (const n of m.addedNodes) scanNode(n);
                        }).observe(document, { childList: true, subtree: true });
                    }
                }

                // Стили подсветки найденных слов на странице (CSS Custom Highlight API - без изменения DOM)
                const highlightRules = `
                    ::highlight(ul-find-all) { background-color: rgba(253, 214, 99, 0.42); color: inherit; }
                    ::highlight(ul-find-cur) { background-color: #ff9800; color: #000; }
                `;
                const css = highlightRules + `
                    @view-transition { navigation: auto; }
                    ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
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
                const ytCss = highlightRules + `
                    @view-transition { navigation: auto; }
                    ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
                    ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-display-ad-renderer,
                    ytd-promoted-sparkles-web-renderer, ytd-promoted-video-renderer, ytd-compact-promoted-video-renderer,
                    ytd-banner-promo-renderer, ytd-statement-banner-renderer, ytd-primetime-promo-renderer,
                    ytd-action-companion-ad-renderer, ytd-player-legacy-desktop-watch-ads-renderer,
                    ytd-brand-video-singleton-renderer, ytd-brand-video-shelf-renderer,
                    #masthead-ad, #player-ads, .ytp-ad-overlay-container { display: none !important; }
                    ytd-rich-item-renderer:has(ytd-ad-slot-renderer),
                    ytd-rich-item-renderer:has(ytd-in-feed-ad-layout-renderer) { display: none !important; }
                `;
                // CSS кладём в constructable stylesheet: на него не действует CSP страницы (style-src),
                // а <style> с textContent на сайтах со строгим CSP молча блокируется.
                const ulSheets = {}, ulTexts = {};
                function ulSetCss(key, text) {
                    try {
                        let sh = ulSheets[key];
                        if (!sh) { sh = new CSSStyleSheet(); ulSheets[key] = sh; }
                        // keepAlive зовёт это при каждой мутации <html>/<head> - не пересчитываем стили зря
                        if (ulTexts[key] !== text) { sh.replaceSync(text); ulTexts[key] = text; }
                        if (!document.adoptedStyleSheets.includes(sh)) {
                            document.adoptedStyleSheets = [...document.adoptedStyleSheets, sh];
                        }
                        return true;
                    } catch (e) { return false; }
                }
                // Если страница перезаписала document.adoptedStyleSheets - возвращаем наши листы
                function ulReattach() {
                    try {
                        for (const k in ulSheets) {
                            if (!document.adoptedStyleSheets.includes(ulSheets[k])) {
                                document.adoptedStyleSheets = [...document.adoptedStyleSheets, ulSheets[k]];
                            }
                        }
                    } catch (e) {}
                }
                document.addEventListener('DOMContentLoaded', ulReattach);
                window.addEventListener('load', ulReattach);

                // @view-transition оставляем обычным <style>, как было раньше (плавные переходы между страницами)
                const vtCss = `
                    @view-transition { navigation: auto; }
                    ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
                `;
                let cssOk = null;
                function injectCss() {
                    const id = isYT ? 'ultralight-yt-css' : 'ultralight-css';
                    if (cssOk === null) cssOk = ulSetCss(id, isYT ? ytCss : css);
                    if (!document.head) { requestAnimationFrame(injectCss); return; }
                    if (!document.getElementById(id)) {
                        const style = document.createElement('style'); style.id = id;
                        style.textContent = cssOk ? vtCss : (isYT ? ytCss : css);
                        document.head.appendChild(style);
                    }
                }
                injectCss();

                let COSM = null;
                let cosmCss = null, cosmEl = null;
                function applyCosmetic() {
                    if (isYT || window !== window.top) return;
                    if (cosmCss === null) {
                        if (typeof window.__ulCosmRaw !== 'function') return;
                        let raw = '{"g":[],"h":{}}';
                        try { raw = window.__ulCosmRaw(); } catch (e) {}
                        try { delete window.__ulCosmRaw; } catch (e) {}
                        try { COSM = JSON.parse(raw); } catch (e) { COSM = { g: [], h: {} }; }
                        const sels = COSM.g.slice();
                        let h = window.location.hostname.toLowerCase();
                        while (h) {
                            if (COSM.h[h]) for (let i = 0; i < COSM.h[h].length; i++) sels.push(COSM.h[h][i]);
                            const i = h.indexOf('.');
                            if (i < 0) break;
                            h = h.slice(i + 1);
                        }
                        const ok = [];
                        for (let i = 0; i < sels.length; i++) {
                            try { if (!window.CSS || CSS.supports('selector(' + sels[i] + ')')) ok.push(sels[i]); } catch (e) {}
                        }
                        const parts = [];
                        for (let i = 0; i < ok.length; i += 100) parts.push(ok.slice(i, i + 100).join(',') + '{display:none!important}');
                        cosmCss = parts.join('\n');
                    }
                    if (!cosmCss) return;
                    if (ulSetCss('cosm', cosmCss)) return;
                    const root = document.head || document.documentElement;
                    if (!root) { requestAnimationFrame(applyCosmetic); return; }
                    if (!cosmEl || !cosmEl.isConnected) {
                        cosmEl = document.createElement('style');
                        cosmEl.textContent = cosmCss;
                        root.appendChild(cosmEl);
                    }
                }
                applyCosmetic();

                let ui = null;

                // ---- Отступ страницы и фиксированных шапок сайтов при закреплённой верхней панели ----
                const PIN_CSS = `
                    html {
                        padding-top: 75px !important;
                        scroll-padding-top: 75px !important;
                        box-sizing: border-box !important;
                    }
                    ytd-masthead, #masthead-container.ytd-app, tp-yt-app-header#header {
                        top: 75px !important;
                    }
                    ytd-app {
                        --ytd-masthead-height: 56px;
                    }
                `;

                let fixedAdjustTimer = 0;
                function adjustFixedHeaders() {
                    if (!document.body) return;
                    const inFs = !!document.fullscreenElement;
                    const active = pinTopBar && !inFs;
                    const candidates = document.querySelectorAll('header, nav, [role="banner"], [role="navigation"], #header, #navbar, .header, .navbar, .sticky-top, .fixed-top');
                    for (let i = 0; i < candidates.length; i++) {
                        const el = candidates[i];
                        if (!el || el === (ui && ui.container) || el === (ui && ui.trigger)) continue;
                        if (el.tagName === 'YTD-MASTHEAD' || el.id === 'masthead-container') continue;
                        try {
                            const cs = window.getComputedStyle(el);
                            if (cs.position === 'fixed' || cs.position === 'sticky') {
                                if (active) {
                                    const topPx = parseFloat(cs.top);
                                    if (!el.dataset.ulOrigTop && topPx >= 0 && topPx < 75) {
                                        el.dataset.ulOrigTop = el.style.top || '';
                                        el.style.setProperty('top', (topPx + 75) + 'px', 'important');
                                    }
                                } else if (el.dataset.ulOrigTop !== undefined) {
                                    const orig = el.dataset.ulOrigTop;
                                    delete el.dataset.ulOrigTop;
                                    if (orig) el.style.top = orig;
                                    else el.style.removeProperty('top');
                                }
                            }
                        } catch (e) {}
                    }
                }

                function applyPinBarLayout() {
                    const inFs = !!document.fullscreenElement;
                    const active = pinTopBar && !inFs;
                    const rules = active ? PIN_CSS : '';
                    if (!ulSetCss('pinbar', rules) && document.head) {
                        let st = document.getElementById('ultralight-pinbar-css');
                        if (active) {
                            if (!st) {
                                st = document.createElement('style');
                                st.id = 'ultralight-pinbar-css';
                                document.head.appendChild(st);
                            }
                            if (st.textContent !== rules) st.textContent = rules;
                        } else if (st) {
                            st.remove();
                        }
                    }
                    if (ui) {
                        if (active) {
                            ui.container.style.top = '0';
                            ui.trigger.style.display = 'none';
                        } else {
                            ui.trigger.style.display = inFs ? 'none' : 'block';
                            if (inFs || !ui.container.matches(':hover')) {
                                ui.hide();
                            }
                        }
                    }
                    clearTimeout(fixedAdjustTimer);
                    fixedAdjustTimer = setTimeout(adjustFixedHeaders, 60);
                }

                document.addEventListener('fullscreenchange', applyPinBarLayout);
                document.addEventListener('DOMContentLoaded', applyPinBarLayout);
                window.addEventListener('load', applyPinBarLayout);

                // ---- URL-синхронизация ----
                let lastSyncedUrl = window.location.href;
                let lastTitle = null, titleTimer = null, titleEl = null, titleObs = null;

                function syncUrl() {
                    if (window !== window.top) return;
                    const href = window.location.href;
                    if (href !== lastSyncedUrl) {
                        lastSyncedUrl = href;
                        ipcPost('SYNC_URL|' + DOC_ID + '|' + href);
                        lastTitle = null; scheduleTitle();
                    }
                    if (ui && !ui.inputFocused()) ui.input.value = href;
                }
                const onNavigation = syncUrl;

                const origPush = history.pushState;
                history.pushState = function() { const r = origPush.apply(this, arguments); onNavigation(); return r; };
                const origReplace = history.replaceState;
                history.replaceState = function() { const r = origReplace.apply(this, arguments); onNavigation(); return r; };
                window.addEventListener('popstate', onNavigation);
                window.addEventListener('hashchange', onNavigation);
                window.addEventListener('yt-navigate-finish', onNavigation);
                // Возврат из BackForwardCache: init-скрипт заново не выполняется, поэтому объявляемся сами
                window.addEventListener('pageshow', (e) => {
                    if (!e.persisted || window !== window.top) return;
                    lastSyncedUrl = window.location.href;
                    ipcPost('LOAD_URL|' + DOC_ID + '|' + lastSyncedUrl);
                    lastTitle = null; scheduleTitle();
                });

                // 2. ИНТЕРФЕЙС
                if (window !== window.top) return;

                // ---- Изолированное масштабирование ТОЛЬКО контента страницы (document.body) ----
                // Верхняя панель вкладок (#light-browser-ui) и плашка уведомлений висят на <html> (documentElement),
                // поэтому они остаются 100% чёткими и не меняют свой размер при зуме страницы!
                const ZOOM_KEY = 'ultralight_page_zoom';
                let pageZoom = 1.0;
                try {
                    const savedZ = parseFloat(localStorage.getItem(ZOOM_KEY) || '1');
                    if (savedZ >= 0.3 && savedZ <= 3.0) pageZoom = savedZ;
                } catch (e) {}

                function applyPageZoom(showNotice) {
                    if (document.body) {
                        document.body.style.zoom = pageZoom === 1.0 ? '' : String(pageZoom);
                    }
                    try {
                        if (pageZoom === 1.0) localStorage.removeItem(ZOOM_KEY);
                        else localStorage.setItem(ZOOM_KEY, String(pageZoom));
                    } catch (e) {}
                    if (showNotice && window.__ulDlNotice) {
                        window.__ulDlNotice('info', 'Масштаб: ' + Math.round(pageZoom * 100) + '%', '');
                    }
                }
                function stepPageZoom(dir) {
                    if (dir === 0) pageZoom = 1.0;
                    else pageZoom = Math.round(Math.min(3.0, Math.max(0.3, pageZoom + dir * 0.1)) * 10) / 10;
                    applyPageZoom(true);
                }
                document.addEventListener('DOMContentLoaded', () => { if (pageZoom !== 1.0) applyPageZoom(false); });

                let keepBarUntil = 0;
                try {
                    const swTs = Number(sessionStorage.getItem('__ul_sw_bar') || '0');
                    if (swTs && (Date.now() - swTs) < 4000) {
                        keepBarUntil = Date.now() + 1100;
                    }
                    sessionStorage.removeItem('__ul_sw_bar');
                } catch (e) {}

                let tabsState = { list: [], active: 0, pinned: 0 };
                let tabEls = [];
                let drawnCount = -1, tabClosing = false;
                let dragState = false, pendingDraw = false, dragJustEnded = false;
                const dropInd = document.createElement('div');
                dropInd.style.cssText = 'position:absolute;top:2px;width:2px;height:28px;background:#8ab4f8;border-radius:1px;display:none;pointer-events:none;';

                function optimisticSelectTab(idx) {
                    tabsState.active = idx;
                    for (let j = 0; j < tabEls.length; j++) {
                        const active = (j === idx);
                        tabEls[j].style.background = active ? '#303134' : '#171717';
                        tabEls[j].style.color = active ? '#fff' : '#9aa0a6';
                    }
                    if (ui && ui.startProgress) ui.startProgress();
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                }

                lockGlobal('__ulNavSmooth', function(url) {
                    try { if (window.__ulSaveForSwitch) window.__ulSaveForSwitch(); } catch (e) {}
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                    if (ui && ui.startProgress) ui.startProgress();
                    if (document.body) {
                        document.body.style.transition = 'opacity 0.09s ease-out';
                        document.body.style.opacity = '0.35';
                    }
                    window.location.href = url;
                });
                lockGlobal('__ulNavDirect', function(url) {
                    try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
                    if (ui && ui.startProgress) ui.startProgress();
                    window.location.href = url;
                });

                function startTabDrag(e0, from, el) {
                    const startX = e0.clientX;
                    let dragging = false, target = from;
                    const insertPos = (x) => {
                        for (let j = 0; j < tabEls.length; j++) {
                            const r = tabEls[j].getBoundingClientRect();
                            if (x < r.left + r.width / 2) return j;
                        }
                        return tabEls.length;
                    };
                    const onMove = (e) => {
                        if (!dragging) {
                            if (Math.abs(e.clientX - startX) < 6) return;
                            dragging = true; dragState = true;
                            el.style.opacity = '0.45';
                            try { el.setPointerCapture(e0.pointerId); } catch (err) {}
                        }
                        const k0 = insertPos(e.clientX);
                        target = k0 > from ? k0 - 1 : k0;
                        const pn = tabsState.pinned | 0;
                        target = from < pn ? Math.min(target, pn - 1) : Math.max(target, pn);
                        const k = target > from ? target + 1 : target;
                        const rowR = ui.tabsRow.getBoundingClientRect();
                        const edge = k < tabEls.length ? tabEls[k].getBoundingClientRect().left - 1 : tabEls[tabEls.length - 1].getBoundingClientRect().right + 1;
                        dropInd.style.left = (edge - rowR.left + ui.tabsRow.scrollLeft) + 'px';
                        dropInd.style.display = target === from ? 'none' : 'block';
                    };
                    const end = (e) => {
                        window.removeEventListener('pointermove', onMove, true);
                        window.removeEventListener('pointerup', end, true);
                        window.removeEventListener('pointercancel', end, true);
                        try { el.releasePointerCapture(e0.pointerId); } catch (err) {}
                        if (!dragging) return;
                        dragState = false;
                        dragJustEnded = true; setTimeout(() => { dragJustEnded = false; }, 80);
                        dropInd.style.display = 'none'; el.style.opacity = '';
                        if (e.type === 'pointerup' && target !== from) ipcPost('MOVE_TAB|' + from + '|' + target);
                        if (pendingDraw) { pendingDraw = false; drawTabs(); }
                    };
                    window.addEventListener('pointermove', onMove, true);
                    window.addEventListener('pointerup', end, true);
                    window.addEventListener('pointercancel', end, true);
                }

                function tabIn(el) {
                    const full = el.style.cssText;
                    el.style.cssText = full + 'min-width:0;max-width:0;padding:0;margin-right:0;opacity:0;overflow:hidden;';
                    requestAnimationFrame(() => requestAnimationFrame(() => { el.style.cssText = full; }));
                }
                function tabOut(el, done) {
                    if (tabClosing) return;
                    tabClosing = true;
                    setTimeout(() => { tabClosing = false; }, 700);
                    el.style.cssText = el.style.cssText + 'min-width:0;max-width:0;padding:0;margin-right:0;opacity:0;overflow:hidden;pointer-events:none;';
                    setTimeout(done, 170);
                }
                // Единственную вкладку не "схлопываем": Rust заменит её стартовой страницей
                function closeTab(i, el) {
                    if (!el || tabsState.list.length <= 1) { ipcPost('CLOSE_TAB|' + i); return; }
                    tabOut(el, () => ipcPost('CLOSE_TAB|' + i));
                }
                function drawTabs() {
                    if (!ui) return;
                    if (dragState) { pendingDraw = true; return; }
                    const row = ui.tabsRow;
                    row.replaceChildren();
                    tabEls = [];
                    tabClosing = false;
                    const prevCount = drawnCount;
                    const grew = prevCount > 0 && tabsState.list.length > prevCount;
                    drawnCount = tabsState.list.length;

                    tabsState.list.forEach((label, i) => {
                        let tabEl = document.createElement('div');
                        let isActive = (i === tabsState.active);
                        const isPinned = i < (tabsState.pinned | 0);

                        tabEl.style.cssText = `
                            min-width: ${isPinned ? 36 : 120}px; max-width: ${isPinned ? 36 : 200}px; height: 28px; margin-top: 2px; margin-right: ${(isPinned && i === (tabsState.pinned | 0) - 1) ? 8 : 2}px;
                            background: ${isActive ? '#303134' : '#171717'}; color: ${isActive ? '#fff' : '#9aa0a6'};
                            border-radius: 8px 8px 0 0; display: flex; align-items: center; justify-content: ${isPinned ? 'center' : 'space-between'};
                            padding: 0 ${isPinned ? 0 : 10}px; font-size: 13px; cursor: pointer; user-select: none;
                            touch-action: none; flex: none;
                        `;
                        tabEl.className = 'ul-tab';
                        tabEl.title = label || 'Новая вкладка';
                        tabEl.onmouseover = () => {
                            if (i !== tabsState.active) {
                                tabEl.style.background = '#202124';
                                ipcPost('WARM_TAB|' + i);
                            }
                        };
                        tabEl.onmouseout = () => { if (i !== tabsState.active) tabEl.style.background = '#171717'; };
                        tabEl.onclick = (e) => {
                            if (e.isTrusted && !dragJustEnded && i !== tabsState.active) {
                                optimisticSelectTab(i);
                                ipcPost('SWITCH_TAB|' + i);
                            }
                        };

                        let titleSpan = document.createElement('span');
                        titleSpan.textContent = label || 'Новая вкладка';
                        titleSpan.style.cssText = `overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-grow: 1; margin-right: 8px; pointer-events: none;`;

                        if (isPinned) {
                            // закреплённая вкладка: только первая буква, без кнопки закрытия (полное название - во всплывающей подсказке)
                            titleSpan.textContent = Array.from(label || 'Новая вкладка')[0].toUpperCase();
                            titleSpan.style.marginRight = '0'; titleSpan.style.flexGrow = '0'; titleSpan.style.fontWeight = '600';
                        }

                        let closeBtn = document.createElement('span'); closeBtn.textContent = '×';
                        closeBtn.style.cssText = `font-size: 16px; width: 18px; height: 18px; display: flex; align-items: center; justify-content: center; border-radius: 50%; font-weight: bold; cursor: pointer; flex: none;`;
                        closeBtn.className = 'ul-tx';
                        closeBtn.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) closeTab(i, tabEl); };

                        tabEl.addEventListener('pointerdown', (e) => {
                            if (e.isTrusted && e.button === 0 && e.target !== closeBtn) startTabDrag(e, i, tabEl);
                        });
                        tabEl.addEventListener('mousedown', (e) => { if (e.button === 1) e.preventDefault(); });
                        tabEl.addEventListener('auxclick', (e) => {
                            if (e.isTrusted && e.button === 1) { e.preventDefault(); closeTab(i, tabEl); }
                        });

                        tabEl.addEventListener('contextmenu', (e) => {
                            e.preventDefault(); e.stopPropagation();
                            if (e.isTrusted) showTabCtx(i, e);
                        });

                        tabEl.appendChild(titleSpan); if (!isPinned) tabEl.appendChild(closeBtn); row.appendChild(tabEl);
                        tabEls.push(tabEl);
                        if (grew && i >= prevCount) tabIn(tabEl);
                    });

                    let newTabBtn = document.createElement('div'); newTabBtn.textContent = '+';
                    newTabBtn.style.cssText = `min-width: 28px; height: 28px; margin-top: 2px; font-size: 20px; display: flex; align-items: center; justify-content: center; cursor: pointer; border-radius: 50%; flex: none;`;
                    newTabBtn.className = 'ul-b';
                    newTabBtn.onclick = (e) => {
                        if (e.isTrusted) {
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            if (ui && ui.startProgress) ui.startProgress();
                            ipcPost('new_tab');
                        }
                    };
                    row.appendChild(newTabBtn);
                    row.appendChild(dropInd);
                }

                const warmedHosts = new Set();
                lockGlobal('__ulWarmHost', function(origin) {
                    if (!origin || warmedHosts.has(origin) || !document.head) return;
                    warmedHosts.add(origin);
                    try {
                        const l = document.createElement('link');
                        l.rel = 'preconnect';
                        l.href = origin;
                        l.crossOrigin = 'anonymous';
                        document.head.appendChild(l);
                    } catch (e) {}
                });

                function sendTitle() {
                    titleTimer = null;
                    const t = String(document.title || '').replace(/\s+/g, ' ').trim().slice(0, 120);
                    if (t === lastTitle) return;
                    lastTitle = t;
                    if (t) ipcPost('TITLE|' + DOC_ID + '|' + t);
                }
                function scheduleTitle() { if (!titleTimer) titleTimer = setTimeout(sendTitle, 250); }
                function watchTitle() {
                    const el = document.querySelector('title');
                    if (el && el !== titleEl) {
                        titleEl = el;
                        if (titleObs) titleObs.disconnect();
                        titleObs = new MutationObserver(scheduleTitle);
                        titleObs.observe(el, { childList: true, characterData: true, subtree: true });
                    }
                    scheduleTitle();
                }
                document.addEventListener('DOMContentLoaded', watchTitle);
                window.addEventListener('load', watchTitle);
                watchTitle();

                lockGlobal('__ulRT', function(tabsString, activeIndex) {
                    if (typeof tabsString !== 'string') return;
                    // формат: "<закреплено>\u001e<ярлык>|||<ярлык>..." (старый формат без числа тоже понимаем)
                    let pinned = 0, body = tabsString;
                    const sp = tabsString.indexOf('\u001e');
                    if (sp >= 0) { pinned = parseInt(tabsString.slice(0, sp), 10) || 0; body = tabsString.slice(sp + 1); }
                    const list = body.split('|||');
                    tabsState = { list: list, active: activeIndex | 0, pinned: Math.max(0, Math.min(pinned, list.length)) };
                    drawTabs();
                });

                function buildUI() {
                    const startVisible = pinTopBar || Date.now() < keepBarUntil;
                    const container = document.createElement('div'); container.id = 'light-browser-ui';
                    container.style.cssText = `position: fixed; top: ${startVisible ? '0' : '-75px'}; left: 0; width: 100%; height: 75px; background: #202124; z-index: 2147483647; font-family: sans-serif; box-shadow: 0 4px 10px rgba(0,0,0,0.5); display: flex; flex-direction: column; transition: top 0.24s cubic-bezier(0.22, 1, 0.36, 1); zoom: 1 !important;`;

                    let root = container;
                    try { root = rApply(rAttachShadow, container, [{ mode: 'closed' }]); } catch (e) { root = container; }

                    const UL_CSS = `
                        .ul-b{color:#9aa0a6;transition:background-color .15s ease,color .15s ease,transform .12s cubic-bezier(.2,.8,.3,1)}
                        .ul-b:hover{background:#303134;color:#fff}
                        .ul-b:active{background:#3c4043;transform:scale(.86)}
                        .ul-b.on{color:#fdd663}
                        .ul-b.on:hover{color:#ffe082}
                        .ul-wc{width:46px;height:32px;display:flex;align-items:center;justify-content:center;color:#9aa0a6;font-size:13px;cursor:pointer;user-select:none;transition:background-color .14s ease,color .14s ease}
                        .ul-wc:hover{background:#303134;color:#fff}
                        .ul-wc:active{background:#3c4043}
                        .ul-wc-close:hover{background:#e81123 !important;color:#fff !important}
                        .ul-wc-close:active{background:#f1707a !important;color:#fff !important}
                        .ul-pop{animation:ul-pop .42s cubic-bezier(.2,.8,.3,1)}
                        .ul-tab{transition:background-color .14s ease,color .14s ease,transform .1s ease,opacity .16s ease,min-width .16s ease,max-width .16s ease,padding .16s ease,margin .16s ease}
                        .ul-tab:active{transform:scale(.97)}
                        .ul-tx{transition:background-color .12s ease,transform .1s ease}
                        .ul-tx:hover{background:#5f6368}
                        .ul-tx:active{transform:scale(.85)}
                        .ul-x{transition:background-color .12s ease,color .12s ease,transform .1s ease}
                        .ul-x:hover{background:#3c4043;color:#fff}
                        .ul-x:active{transform:scale(.85)}
                        .ul-row{transition:background-color .12s ease,opacity .2s ease,transform .2s cubic-bezier(.4,0,.2,1),height .2s cubic-bezier(.4,0,.2,1),padding .2s ease}
                        .ul-row:hover{background:#303134}
                        .ul-row:active{background:#3c4043}
                        .ul-btn{transition:background-color .12s ease,transform .1s ease}
                        .ul-btn:hover{background:#303134}
                        .ul-btn:active{transform:scale(.95)}
                        .ul-btn2{transition:filter .12s ease,transform .1s ease}
                        .ul-btn2:hover{filter:brightness(1.12)}
                        .ul-btn2:active{transform:scale(.95)}
                        .ul-input{background:#303134;transition:background-color .15s ease,box-shadow .15s ease}
                        .ul-input:hover{background:#35363a}
                        .ul-input:focus{background:#3a3b3f;box-shadow:0 0 0 1px #8ab4f8}
                        .ul-sg{transition:background-color .1s ease}
                        .ul-sg:hover,.ul-sg.sel{background:#303134}
                        .ul-in{animation:ul-in .17s cubic-bezier(.2,.8,.3,1) both}
                        .ul-out{animation:ul-out .12s ease-in both}
                        .ul-view{animation:ul-fade .16s ease-out both}
                        .ul-prog{position:absolute;bottom:0;left:0;height:2px;width:0;background:linear-gradient(90deg,#8ab4f8,#c58af9);opacity:0;pointer-events:none;transition:width .25s cubic-bezier(.2,.8,.3,1),opacity .2s ease}
                        @keyframes ul-pop{0%{transform:scale(1)}35%{transform:scale(1.45) rotate(-14deg)}70%{transform:scale(.92)}100%{transform:scale(1)}}
                        @keyframes ul-in{from{opacity:0;transform:translateY(-8px) scale(.97)}to{opacity:1;transform:none}}
                        @keyframes ul-out{from{opacity:1;transform:none}to{opacity:0;transform:translateY(-8px) scale(.97)}}
                        @keyframes ul-fade{from{opacity:0}to{opacity:1}}
                        @media (prefers-reduced-motion:reduce){*{animation-duration:.01ms !important;transition-duration:.01ms !important}}
                    `;
                    let ulStyled = false;
                    if (root !== container) {
                        try { const sh = new CSSStyleSheet(); sh.replaceSync(UL_CSS); root.adoptedStyleSheets = [sh]; ulStyled = true; } catch (e) {}
                    }
                    if (!ulStyled) { try { const st = document.createElement('style'); st.textContent = UL_CSS; root.appendChild(st); } catch (e) {} }

                    const topBar = document.createElement('div');
                    topBar.style.cssText = `display: flex; height: 32px; background: #000; align-items: center; width: 100%; user-select: none;`;

                    const tabsRow = document.createElement('div'); tabsRow.id = 'light-tabs-row';
                    tabsRow.style.cssText = `display: flex; flex: 1; min-width: 0; height: 32px; background: #000; overflow-x: auto; padding: 0 5px; position: relative;`;

                    tabsRow.addEventListener('mousedown', (e) => {
                        if (e.isTrusted && e.button === 0 && e.target === tabsRow) {
                            e.preventDefault();
                            ipcPost('WIN_DRAG');
                        }
                    });
                    tabsRow.addEventListener('dblclick', (e) => {
                        if (e.isTrusted && e.target === tabsRow) {
                            e.preventDefault();
                            ipcPost('WIN_MAX');
                        }
                    });

                    const winCtrls = document.createElement('div');
                    winCtrls.style.cssText = `display: flex; height: 32px; flex: none; background: #000;`;

                    const minBtn = document.createElement('div');
                    minBtn.className = 'ul-wc';
                    minBtn.textContent = '\u2500';
                    minBtn.title = 'Свернуть';
                    minBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_MIN'); };

                    const maxBtn = document.createElement('div');
                    maxBtn.className = 'ul-wc';
                    maxBtn.textContent = '\u25A1';
                    maxBtn.title = 'Развернуть / Свернуть в окно';
                    maxBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_MAX'); };

                    const closeWinBtn = document.createElement('div');
                    closeWinBtn.className = 'ul-wc ul-wc-close';
                    closeWinBtn.textContent = '\u2715';
                    closeWinBtn.title = 'Закрыть браузер';
                    closeWinBtn.onclick = (e) => { if (e.isTrusted) ipcPost('WIN_CLOSE'); };

                    winCtrls.appendChild(minBtn);
                    winCtrls.appendChild(maxBtn);
                    winCtrls.appendChild(closeWinBtn);

                    topBar.appendChild(tabsRow);
                    topBar.appendChild(winCtrls);

                    const urlRow = document.createElement('div');
                    urlRow.style.cssText = `display: flex; height: 43px; align-items: center; padding: 0 10px; box-sizing: border-box; position: relative;`;
                    urlRow.addEventListener('mousedown', (e) => {
                        if (e.isTrusted && e.button === 0 && e.target === urlRow) {
                            e.preventDefault();
                            ipcPost('WIN_DRAG');
                        }
                    });

                    const progBar = document.createElement('div');
                    progBar.className = 'ul-prog';
                    let progTimer = 0;
                    function startProgress() {
                        clearTimeout(progTimer);
                        progBar.style.transition = 'none';
                        progBar.style.width = '15%';
                        progBar.style.opacity = '1';
                        requestAnimationFrame(() => {
                            progBar.style.transition = 'width 0.35s cubic-bezier(.2,.8,.3,1), opacity 0.2s ease';
                            progBar.style.width = '78%';
                        });
                    }
                    function finishProgress() {
                        clearTimeout(progTimer);
                        progBar.style.width = '100%';
                        progTimer = setTimeout(() => { progBar.style.opacity = '0'; }, 140);
                    }
                    if (document.readyState !== 'complete') {
                        startProgress();
                        window.addEventListener('load', finishProgress, { once: true });
                    }

                    const trigger = document.createElement('div'); trigger.id = 'light-trigger-zone';
                    trigger.style.cssText = `position: fixed; top: 0; left: 0; width: 100%; height: 15px; z-index: 2147483646; background: transparent; display: ${pinTopBar ? 'none' : 'block'};`;

                    const input = document.createElement('input'); input.type = 'text'; input.value = window.location.href;
                    input.style.cssText = `flex: 1; min-width: 0; height: 30px; border-radius: 15px; border: none; padding: 0 20px; color: #fff; outline: none; font-size: 14px;`;
                    input.className = 'ul-input';
                    ['keydown', 'keyup', 'keypress'].forEach((t) => input.addEventListener(t, (ev) => ev.stopPropagation()));

                    // ---- Подсказки из истории и закладок (ищет Rust: SUGGEST -> __ulSug) ----
                    const sug = document.createElement('div');
                    sug.style.cssText = 'display:none;position:absolute;top:100%;margin-top:2px;box-sizing:border-box;padding:4px 0;background:#202124;color:#e8eaed;font:13px/1.35 "Segoe UI",system-ui,sans-serif;border-radius:10px;box-shadow:0 6px 24px rgba(0,0,0,.6);overflow:hidden;z-index:5;';
                    sug.addEventListener('mousedown', (ev) => ev.preventDefault());
                    let sugItems = [], sugSel = -1, sugWait = '', sugTimer = 0, sugTyped = '';
                    const sugOpen = () => sug.style.display !== 'none';
                    function closeSug() {
                        clearTimeout(sugTimer);
                        sugWait = '';
                        sugItems = []; sugSel = -1;
                        sug.style.display = 'none'; sug.textContent = '';
                    }
                    function goTo(u) { closeSug(); startProgress(); window.location.href = u; }
                    function paintSug() {
                        const rows = sug.children;
                        for (let i = 0; i < rows.length; i++) rows[i].className = i === sugSel ? 'ul-sg sel' : 'ul-sg';
                    }
                    const sugShort = (u) => String(u).replace(/^https?:\/\/(www\.)?/i, '').replace(/\/$/, '');
                    function showSug(items) {
                        sug.textContent = '';
                        sugItems = items; sugSel = -1;
                        if (!items.length) { sug.style.display = 'none'; return; }
                        items.forEach((it) => {
                            const row = document.createElement('div');
                            row.className = 'ul-sg';
                            row.style.cssText = 'display:flex;align-items:center;padding:6px 16px;cursor:pointer;';
                            const ic = document.createElement('div');
                            ic.textContent = it.b ? '\u2605' : '\u25F7';
                            ic.style.cssText = 'width:18px;flex:none;text-align:center;margin-right:10px;font-size:13px;color:' + (it.b ? '#fdd663' : '#9aa0a6') + ';';
                            const col = document.createElement('div');
                            col.style.cssText = 'flex:1;min-width:0;';
                            const t = document.createElement('div');
                            t.textContent = it.t || sugShort(it.u);
                            t.style.cssText = 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
                            const u = document.createElement('div');
                            u.textContent = sugShort(it.u);
                            u.style.cssText = 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:11px;';
                            col.appendChild(t); col.appendChild(u);
                            row.appendChild(ic); row.appendChild(col);
                            row.addEventListener('click', (ev) => { if (ev.isTrusted) goTo(it.u); });
                            sug.appendChild(row);
                        });
                        const r = input.getBoundingClientRect(), c = container.getBoundingClientRect();
                        sug.style.left = Math.round(r.left - c.left) + 'px';
                        sug.style.width = Math.round(r.width) + 'px';
                        sug.style.display = 'block';
                    }
                    // Ответ Rust. Номер запроса случайный и одноразовый: страница не может подсунуть свои "подсказки".
                    lockGlobal('__ulSug', function(seq, json) {
                        if (!sugWait || String(seq) !== sugWait || !inputFocused()) return;
                        sugWait = '';
                        let items = [];
                        try { items = rApply(rJsonParse, JSON, [String(json)]); } catch (e) { items = []; }
                        if (!Array.isArray(items)) return;
                        const ok = [];
                        for (const x of items) {
                            if (x && typeof x.u === 'string' && /^https?:\/\//i.test(x.u) && ok.length < 8) ok.push({ u: x.u, t: typeof x.t === 'string' ? x.t : '', b: !!x.b });
                        }
                        showSug(ok);
                    });
                    input.addEventListener('input', () => {
                        sugTyped = input.value;
                        clearTimeout(sugTimer);
                        const q = sugTyped.trim();
                        if (!q || q.length > 100) { closeSug(); return; }
                        const my = String(100000000 + Math.floor(Math.random() * 899999999));
                        sugWait = my;
                        sugTimer = setTimeout(() => ipcPost('SUGGEST|' + my + '|' + q), 60);
                    });
                    input.addEventListener('blur', () => { setTimeout(closeSug, 120); });

                    input.addEventListener('keydown', (e) => {
                        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                            if (!sugOpen() || !sugItems.length) return;
                            e.preventDefault();
                            const n = sugItems.length;
                            let i = sugSel + (e.key === 'ArrowDown' ? 1 : -1);
                            if (i >= n) i = -1; else if (i < -1) i = n - 1;
                            sugSel = i; paintSug();
                            input.value = i >= 0 ? sugItems[i].u : sugTyped;
                            return;
                        }
                        if (e.key === 'Escape') {
                            if (sugOpen()) {
                                e.preventDefault();
                                if (sugSel >= 0) input.value = sugTyped;
                                closeSug();
                            }
                            return;
                        }
                        if (e.key === 'Enter') {
                            if (sugSel >= 0 && sugItems[sugSel]) { e.preventDefault(); goTo(sugItems[sugSel].u); return; }
                            closeSug();
                            let query = input.value.trim();
                            if (query.includes('.') && !query.includes(' ')) {
                                if (!query.startsWith('http')) query = 'https://' + query;
                                startProgress();
                                window.location.href = query;
                            } else {
                                startProgress();
                                window.location.href = searchPrefix + encodeURIComponent(query);
                            }
                        }
                    });
                    input.addEventListener('focus', () => input.select());

                    const show = () => {
                        container.style.top = '0';
                        input.value = window.location.href;
                        ipcPost('GET_TABS');
                    };
                    const inputFocused = () => ((root.activeElement !== undefined ? root.activeElement : document.activeElement) === input);
                    let panelOpen = false, popOpen = false, findOpen = false;
                    const hide = () => {
                        if (pinTopBar && !document.fullscreenElement) {
                            container.style.top = '0';
                            return;
                        }
                        if (dragState || panelOpen || popOpen || ctxMenu || Date.now() < keepBarUntil) return;
                        if (!inputFocused()) container.style.top = '-75px';
                    };

                    if (startVisible && !pinTopBar) {
                        setTimeout(() => {
                            if (ui && !ui.container.matches(':hover')) hide();
                        }, 1150);
                    }

                    trigger.addEventListener('mouseenter', show);
                    container.addEventListener('mouseleave', hide);

                    function navButton(symbol, title, onClick) {
                        const b = document.createElement('div');
                        b.textContent = symbol; b.title = title;
                        b.style.cssText = `width: 30px; height: 30px; margin-right: 4px; flex: none; border-radius: 50%; font-size: 18px; display: flex; align-items: center; justify-content: center; cursor: pointer; user-select: none;`;
                        b.className = 'ul-b';
                        b.onclick = onClick;
                        return b;
                    }
                    urlRow.appendChild(navButton('\u2190', 'Назад (Alt+←)', () => { startProgress(); history.back(); }));
                    urlRow.appendChild(navButton('\u2192', 'Вперёд (Alt+→)', () => { startProgress(); history.forward(); }));
                    urlRow.appendChild(navButton('\u21BB', 'Обновить (F5)', () => { startProgress(); location.reload(); }));

                    const popBtn = navButton('\u2197', 'Вывести текущую вкладку в отдельное окно (для видеозвонков)', (e) => { if (e && e.isTrusted) ipcPost('DETACH_TAB'); });
                    popBtn.style.marginLeft = '0';

                    const US = '\u001f';
                    const starBtn = navButton('\u2606', 'Добавить страницу в закладки (Ctrl+D)', (e) => { if (e && e.isTrusted) onStar(); });
                    starBtn.style.marginLeft = '6px';
                    let starOn = false, bmTitle = '';
                    function setStar(on) {
                        starOn = !!on;
                        starBtn.className = starOn ? 'ul-b on' : 'ul-b';
                        starBtn.textContent = starOn ? '\u2605' : '\u2606';
                        starBtn.title = starOn ? 'Изменить закладку (Ctrl+D)' : 'Добавить страницу в закладки (Ctrl+D)';
                    }
                    function popStar() {
                        starBtn.className = 'ul-b on ul-pop';
                        setTimeout(() => { if (starOn) starBtn.className = 'ul-b on'; }, 430);
                    }
                    lockGlobal('__ulBM', function(on, title) {
                        if (!!on !== starOn) setStar(on);
                        bmTitle = on ? String(title || '') : '';
                    });
                    lockGlobal('__ulMaxState', function(isMax) {
                        maxBtn.textContent = isMax ? '\u2750' : '\u25A1';
                    });
                    const cleanTitle = (x) => String(x || '').replace(/\s+/g, ' ').trim().slice(0, 120);
                    const pageTitle = () => cleanTitle(document.title) || hostOf(location.href);

                    const bmPop = document.createElement('div');
                    bmPop.style.cssText = 'display:none;position:absolute;top:100%;right:78px;margin-top:6px;width:300px;max-width:calc(100vw - 16px);box-sizing:border-box;padding:14px 16px 12px;background:#202124;color:#e8eaed;font:13px/1.35 sans-serif;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.6);';
                    let popHideT = 0, popUrl = '', popOrig = '', popInput = null;
                    function closeBmPop(commit) {
                        if (!popOpen) return;
                        popOpen = false;
                        if (commit && popInput) {
                            const v = cleanTitle(popInput.value);
                            if (v && v !== popOrig) { bmTitle = v; ipcPost('BM_RENAME|' + popUrl + US + v); }
                        }
                        bmPop.className = 'ul-out';
                        clearTimeout(popHideT);
                        popHideT = setTimeout(() => { bmPop.style.display = 'none'; bmPop.textContent = ''; bmPop.className = ''; popInput = null; }, 130);
                        setTimeout(() => { if (!container.matches(':hover')) hide(); }, 0);
                    }
                    function openBmPop(isNew) {
                        if (panelOpen) closePanel();
                        clearTimeout(popHideT);
                        popOpen = true; container.style.top = '0';
                        popUrl = location.href; popOrig = bmTitle || pageTitle();
                        bmPop.textContent = '';
                        bmPop.appendChild(mk('div', 'font-size:14px;font-weight:bold;margin-bottom:10px;', isNew ? 'Закладка добавлена' : 'Изменить закладку'));
                        bmPop.appendChild(mk('div', 'font-size:11px;color:#9aa0a6;margin-bottom:4px;', 'Название'));
                        const inp = document.createElement('input'); inp.type = 'text'; inp.value = popOrig; inp.maxLength = 120;
                        inp.className = 'ul-input';
                        inp.style.cssText = 'width:100%;height:32px;box-sizing:border-box;border:none;border-radius:6px;padding:0 10px;color:#fff;outline:none;font-size:13px;';
                        ['keydown', 'keyup', 'keypress'].forEach((t) => inp.addEventListener(t, (ev) => ev.stopPropagation()));
                        inp.addEventListener('keydown', (ev) => {
                            if (ev.key === 'Enter') { ev.preventDefault(); closeBmPop(true); }
                            else if (ev.key === 'Escape') { ev.preventDefault(); closeBmPop(false); }
                        });
                        popInput = inp;
                        bmPop.appendChild(inp);
                        const btns = mk('div', 'display:flex;justify-content:flex-end;gap:8px;margin-top:12px;');
                        const delBtn = mk('div', 'padding:6px 12px;border-radius:6px;cursor:pointer;color:#f28b82;user-select:none;', 'Удалить');
                        delBtn.className = 'ul-btn';
                        delBtn.onclick = (e) => {
                            if (!e.isTrusted) return;
                            ipcPost('BM_DEL|' + popUrl); setStar(false); bmTitle = '';
                            closeBmPop(false);
                        };
                        const okBtn = mk('div', 'padding:6px 14px;border-radius:6px;cursor:pointer;background:#8ab4f8;color:#202124;font-weight:bold;user-select:none;', 'Готово');
                        okBtn.className = 'ul-btn2';
                        okBtn.onclick = (e) => { if (e.isTrusted) closeBmPop(true); };
                        btns.appendChild(delBtn); btns.appendChild(okBtn);
                        bmPop.appendChild(btns);
                        bmPop.className = 'ul-in'; bmPop.style.display = 'block';
                        inp.focus(); inp.select();
                    }
                    function onStar() {
                        if (popOpen) { closeBmPop(true); return; }
                        if (starOn) { openBmPop(false); return; }
                        bmTitle = pageTitle();
                        ipcPost('BM_ADD|' + location.href + US + bmTitle);
                        setStar(true); popStar(); openBmPop(true);
                    }

                    // ---- Умный поиск по странице (Ctrl+F) с подсчётом (1/10) и подсветкой всех слов ----
                    const findBar = document.createElement('div');
                    findBar.style.cssText = 'display:none;position:fixed;top:82px;right:14px;height:36px;padding:0 6px 0 12px;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:18px;box-shadow:0 6px 20px rgba(0,0,0,.55);align-items:center;gap:4px;z-index:2147483647;font:13px sans-serif;user-select:none;';
                    const findInp = document.createElement('input');
                    findInp.type = 'text';
                    findInp.placeholder = 'Найти на странице…';
                    findInp.style.cssText = 'width:155px;height:26px;border:none;background:transparent;color:#fff;outline:none;font-size:13px;';
                    ['keydown', 'keyup', 'keypress'].forEach((t) => findInp.addEventListener(t, (ev) => ev.stopPropagation()));

                    const findCount = document.createElement('span');
                    findCount.style.cssText = 'font-size:11px;color:#9aa0a6;min-width:38px;text-align:right;margin-right:4px;font-variant-numeric:tabular-nums;user-select:none;';
                    findCount.textContent = '';

                    let findRanges = [];
                    let findIdx = -1;
                    let lastFindQuery = '';

                    function clearFindHighlights() {
                        findRanges = [];
                        findIdx = -1;
                        lastFindQuery = '';
                        findCount.textContent = '';
                        try {
                            if (window.CSS && CSS.highlights) {
                                CSS.highlights.delete('ul-find-all');
                                CSS.highlights.delete('ul-find-cur');
                            }
                        } catch (e) {}
                    }

                    function isNodeVisible(el) {
                        while (el && el.nodeType === 1) {
                            if (el === container) return false;
                            const tag = el.tagName;
                            if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE' || tag === 'SVG' || el.hidden) return false;
                            el = el.parentElement;
                        }
                        return true;
                    }

                    function updateActiveMatch(scroll) {
                        const total = findRanges.length;
                        if (!total) {
                            findCount.textContent = findInp.value ? '0/0' : '';
                            findCount.style.color = findInp.value ? '#f28b82' : '#9aa0a6';
                            findInp.style.color = findInp.value ? '#f28b82' : '#fff';
                            try { if (window.CSS && CSS.highlights) CSS.highlights.delete('ul-find-cur'); } catch (e) {}
                            return;
                        }
                        if (findIdx < 0) findIdx = total - 1;
                        if (findIdx >= total) findIdx = 0;
                        findCount.textContent = (findIdx + 1) + '/' + total;
                        findCount.style.color = '#9aa0a6';
                        findInp.style.color = '#fff';

                        const r = findRanges[findIdx];
                        try {
                            if (window.CSS && CSS.highlights && typeof Highlight !== 'undefined') {
                                CSS.highlights.set('ul-find-cur', new Highlight(r));
                            } else {
                                const sel = window.getSelection();
                                if (sel) { sel.removeAllRanges(); sel.addRange(r); }
                            }
                            if (scroll) {
                                const el = r.startContainer && r.startContainer.parentElement;
                                if (el && el.scrollIntoView) {
                                    el.scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'nearest' });
                                }
                            }
                        } catch (e) {}
                    }

                    function buildFindMatches(q) {
                        findRanges = [];
                        findIdx = -1;
                        lastFindQuery = q;
                        if (!q || !document.body) {
                            clearFindHighlights();
                            return;
                        }
                        const needle = q.toLowerCase();
                        const nLen = needle.length;
                        const MAX_MATCHES = 2000;
                        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
                        let node;
                        while ((node = walker.nextNode())) {
                            const parent = node.parentElement;
                            if (!parent || !isNodeVisible(parent)) continue;
                            if (parent.offsetWidth === 0 && parent.offsetHeight === 0 && parent.getClientRects().length === 0) continue;
                            const val = node.nodeValue;
                            if (!val || val.length < nLen) continue;
                            const low = val.toLowerCase();
                            let pos = 0;
                            while ((pos = low.indexOf(needle, pos)) !== -1) {
                                const rng = document.createRange();
                                rng.setStart(node, pos);
                                rng.setEnd(node, pos + nLen);
                                findRanges.push(rng);
                                pos += nLen;
                                if (findRanges.length >= MAX_MATCHES) break;
                            }
                            if (findRanges.length >= MAX_MATCHES) break;
                        }

                        try {
                            if (window.CSS && CSS.highlights && typeof Highlight !== 'undefined') {
                                if (findRanges.length) {
                                    CSS.highlights.set('ul-find-all', new Highlight(...findRanges));
                                } else {
                                    CSS.highlights.delete('ul-find-all');
                                }
                            }
                        } catch (e) {}

                        if (findRanges.length) {
                            findIdx = 0;
                            for (let i = 0; i < findRanges.length; i++) {
                                const rect = findRanges[i].getBoundingClientRect();
                                if (rect && rect.top >= 0) { findIdx = i; break; }
                            }
                        }
                        updateActiveMatch(true);
                    }

                    function runFind(backwards) {
                        const q = findInp.value;
                        if (!q) { clearFindHighlights(); return; }
                        if (q !== lastFindQuery) {
                            buildFindMatches(q);
                            return;
                        }
                        if (!findRanges.length) {
                            updateActiveMatch(false);
                            return;
                        }
                        findIdx += backwards ? -1 : 1;
                        updateActiveMatch(true);
                    }

                    findInp.addEventListener('input', () => {
                        buildFindMatches(findInp.value);
                    });

                    findInp.addEventListener('keydown', (e) => {
                        if (e.key === 'Enter') { e.preventDefault(); runFind(e.shiftKey); }
                        else if (e.key === 'Escape') { e.preventDefault(); closeFind(); }
                    });

                    const mkFindBtn = (sym, title, fn) => {
                        const b = document.createElement('div');
                        b.textContent = sym; b.title = title;
                        b.style.cssText = 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;color:#9aa0a6;font-size:13px;';
                        b.className = 'ul-x';
                        b.onclick = (e) => { if (e.isTrusted) fn(); };
                        return b;
                    };
                    const findPrev = mkFindBtn('\u25B2', 'Предыдущее (Shift+Enter)', () => runFind(true));
                    const findNext = mkFindBtn('\u25BC', 'Следующее (Enter)', () => runFind(false));
                    const findClose = mkFindBtn('\u2715', 'Закрыть поиск (Esc)', () => closeFind());
                    findBar.appendChild(findInp);
                    findBar.appendChild(findCount);
                    findBar.appendChild(findPrev);
                    findBar.appendChild(findNext);
                    findBar.appendChild(findClose);

                    function openFind() {
                        findOpen = true;
                        findBar.className = 'ul-in';
                        findBar.style.display = 'flex';
                        try {
                            const sel = String(window.getSelection ? window.getSelection() : '').trim();
                            if (sel && sel.length <= 80 && !sel.includes('\n')) findInp.value = sel;
                        } catch (e) {}
                        if (findInp.value) buildFindMatches(findInp.value);
                        findInp.focus();
                        findInp.select();
                    }
                    function closeFind() {
                        if (!findOpen) return;
                        findOpen = false;
                        findBar.style.display = 'none';
                        clearFindHighlights();
                    }

                    // ---- Меню справа (три точки): История, Закладки, Загрузки, Поиск, Вернуть вкладку, Настройки ----
                    const panel = document.createElement('div');
                    panel.style.cssText = 'display:none;position:absolute;top:100%;right:8px;margin-top:6px;width:380px;max-width:calc(100vw - 16px);max-height:min(78vh,580px);overflow-y:auto;background:#202124;color:#e8eaed;font:13px/1.35 sans-serif;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.6);box-sizing:border-box;';
                    let viewKind = 'menu', panelHideT = 0, activeDlDirInput = null;
                    const mk = (tag, css, text) => {
                        const n = document.createElement(tag);
                        if (css) n.style.cssText = css;
                        if (text !== undefined) n.textContent = text;
                        return n;
                    };
                    const pad2 = (x) => (x < 10 ? '0' : '') + x;
                    const dayKey = (d) => d.getFullYear() * 10000 + d.getMonth() * 100 + d.getDate();
                    function dayLabel(ts) {
                        const d = new Date(ts * 1000), n = new Date();
                        if (dayKey(d) === dayKey(n)) return 'Сегодня';
                        if (dayKey(d) === dayKey(new Date(n.getFullYear(), n.getMonth(), n.getDate() - 1))) return 'Вчера';
                        return pad2(d.getDate()) + '.' + pad2(d.getMonth() + 1) + '.' + d.getFullYear();
                    }
                    const timeLabel = (ts) => { const d = new Date(ts * 1000); return pad2(d.getHours()) + ':' + pad2(d.getMinutes()); };
                    const hostOf = (u) => String(u).replace(/^https?:\/\//i, '').split(/[\/?#]/)[0];

                    function closePanel() {
                        if (!panelOpen) return;
                        panelOpen = false; viewKind = 'menu'; activeDlDirInput = null;
                        panel.className = 'ul-out';
                        clearTimeout(panelHideT);
                        panelHideT = setTimeout(() => { panel.style.display = 'none'; panel.textContent = ''; panel.className = ''; }, 130);
                        setTimeout(() => { if (!container.matches(':hover')) hide(); }, 0);
                    }
                    function panelHeader(title, extra) {
                        const h = mk('div', 'display:flex;align-items:center;padding:8px 8px;border-bottom:1px solid #3c4043;position:sticky;top:0;background:#202124;z-index:1;');
                        const back = mk('div', 'width:28px;height:28px;display:flex;align-items:center;justify-content:center;border-radius:50%;cursor:pointer;color:#9aa0a6;font-size:16px;', '\u2190');
                        back.title = 'Назад в меню';
                        back.className = 'ul-x';
                        back.onclick = (e) => { if (e.isTrusted) showMenu(); };
                        h.appendChild(back);
                        h.appendChild(mk('div', 'flex:1;font-weight:bold;margin-left:8px;', title));
                        if (extra) h.appendChild(extra);
                        return h;
                    }
                    function showMenu() {
                        viewKind = 'menu'; activeDlDirInput = null;
                        panel.textContent = '';
                        const box = mk('div', 'padding:6px 0;');
                        box.className = 'ul-view';
                        [
                            ['История (Ctrl+H)', 'HIST_LIST'],
                            ['Закладки (Ctrl+D)', 'BM_LIST'],
                            ['Загрузки (Ctrl+J)', 'DL_LIST'],
                        ].forEach((it) => {
                            const r = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', it[0]);
                            r.className = 'ul-row';
                            r.onclick = (e) => { if (!e.isTrusted) return; openLoading(it[0].split(' (')[0]); ipcPost(it[1]); };
                            box.appendChild(r);
                        });
                        const findRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', 'Найти на странице (Ctrl+F)');
                        findRow.className = 'ul-row';
                        findRow.onclick = (e) => { if (!e.isTrusted) return; closePanel(); openFind(); };
                        box.appendChild(findRow);

                        const reopenRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', 'Вернуть закрытую вкладку (Ctrl+Shift+T)');
                        reopenRow.className = 'ul-row';
                        reopenRow.onclick = (e) => {
                            if (!e.isTrusted) return;
                            closePanel();
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            startProgress();
                            ipcPost('REOPEN_TAB');
                        };
                        box.appendChild(reopenRow);

                        const settingsRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', 'Настройки (Ctrl+,)');
                        settingsRow.className = 'ul-row';
                        settingsRow.onclick = (e) => {
                            if (!e.isTrusted) return;
                            openLoading('Настройки');
                            ipcPost('SETTINGS_GET');
                        };
                        box.appendChild(settingsRow);

                        // Быстрое управление масштабом страницы прямо из меню
                        const zoomRow = mk('div', 'display:flex;align-items:center;justify-content:space-between;padding:8px 16px;border-top:1px solid #3c4043;margin-top:4px;user-select:none;');
                        zoomRow.appendChild(mk('span', 'color:#9aa0a6;', 'Масштаб страницы'));
                        const zCtrls = mk('div', 'display:flex;align-items:center;gap:6px;');
                        const zMinus = mk('div', 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#fff;font-weight:bold;', '\u2212');
                        zMinus.className = 'ul-btn';
                        const zVal = mk('span', 'min-width:44px;text-align:center;font-variant-numeric:tabular-nums;cursor:pointer;', Math.round(pageZoom * 100) + '%');
                        zVal.title = 'Сбросить на 100% (Ctrl+0)';
                        const zPlus = mk('div', 'width:26px;height:26px;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#fff;font-weight:bold;', '+');
                        zPlus.className = 'ul-btn';
                        zMinus.onclick = (e) => { if (e.isTrusted) { stepPageZoom(-1); zVal.textContent = Math.round(pageZoom * 100) + '%'; } };
                        zPlus.onclick = (e) => { if (e.isTrusted) { stepPageZoom(1); zVal.textContent = Math.round(pageZoom * 100) + '%'; } };
                        zVal.onclick = (e) => { if (e.isTrusted) { stepPageZoom(0); zVal.textContent = '100%'; } };
                        zCtrls.appendChild(zMinus); zCtrls.appendChild(zVal); zCtrls.appendChild(zPlus);
                        zoomRow.appendChild(zCtrls);
                        box.appendChild(zoomRow);

                        panel.appendChild(box);
                    }
                    function openLoading(title) {
                        viewKind = 'loading';
                        panel.textContent = '';
                        const v = mk('div'); v.className = 'ul-view';
                        v.appendChild(panelHeader(title));
                        panel.appendChild(v);
                    }
                    function openPanel() {
                        if (popOpen) closeBmPop(true);
                        clearTimeout(panelHideT);
                        panelOpen = true; container.style.top = '0';
                        showMenu();
                        panel.className = 'ul-in'; panel.style.display = 'block';
                    }
                    function openUrl(u, background) {
                        if (!/^https?:\/\//i.test(u)) return;
                        if (background) ipcPost('OPEN_TAB_BG|' + u);
                        else { closePanel(); startProgress(); location.href = u; }
                    }
                    function collapseRow(row, done) {
                        row.style.height = row.getBoundingClientRect().height + 'px';
                        row.style.overflow = 'hidden'; row.style.pointerEvents = 'none';
                        void row.offsetHeight;
                        requestAnimationFrame(() => {
                            row.style.height = '0'; row.style.paddingTop = '0'; row.style.paddingBottom = '0';
                            row.style.opacity = '0'; row.style.transform = 'translateX(28px)';
                        });
                        setTimeout(done, 210);
                    }
                    function listRow(kind, it) {
                        const isDl = kind === 'downloads';
                        const row = mk('div', 'display:flex;align-items:center;padding:6px 8px 6px 16px;cursor:pointer;box-sizing:border-box;');
                        row.className = 'ul-row';
                        row.title = isDl ? 'Показать в папке: ' + String(it.u) : String(it.u);
                        const col = mk('div', 'flex:1;min-width:0;');
                        col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;', String(it.t || it.u)));
                        const sub = isDl
                            ? timeLabel(it.d) + '  \u00B7  ' + String(it.u)
                            : hostOf(it.u) + (kind === 'history' ? '  \u00B7  ' + timeLabel(it.d) : '');
                        col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:11px;', sub));
                        row.appendChild(col);

                        if (isDl) {
                            const showBtn = mk('div', 'width:26px;height:26px;flex:none;margin-left:6px;border-radius:50%;display:flex;align-items:center;justify-content:center;color:#8ab4f8;font-size:14px;', '\uD83D\uDCC2');
                            showBtn.className = 'ul-x';
                            showBtn.title = 'Показать файл в папке';
                            showBtn.onclick = (e) => {
                                e.stopPropagation();
                                if (e.isTrusted) ipcPost('DL_SHOW|' + it.u);
                            };
                            row.appendChild(showBtn);
                        }

                        const del = mk('div', 'width:24px;height:24px;flex:none;margin-left:4px;border-radius:50%;display:flex;align-items:center;justify-content:center;color:#9aa0a6;font-size:13px;', '\u2715');
                        del.className = 'ul-x';
                        del.title = isDl ? 'Убрать из списка' : (kind === 'history' ? 'Удалить из истории' : 'Удалить закладку');
                        del.onclick = (e) => {
                            e.stopPropagation();
                            if (!e.isTrusted) return;
                            const cmd = (isDl ? 'DL_DEL|' : (kind === 'history' ? 'HIST_DEL|' : 'BM_DEL|')) + it.u;
                            collapseRow(row, () => ipcPost(cmd));
                        };
                        if (isDl) {
                            row.onclick = (e) => { if (e.isTrusted) ipcPost('DL_SHOW|' + it.u); };
                        } else {
                            row.onclick = (e) => { if (e.isTrusted) openUrl(String(it.u), e.ctrlKey || e.metaKey); };
                            row.onauxclick = (e) => { if (e.isTrusted && e.button === 1) { e.preventDefault(); openUrl(String(it.u), true); } };
                        }
                        row.appendChild(del);
                        return row;
                    }
                    function showList(kind, items) {
                        const fresh = viewKind !== kind;
                        const keep = panel.scrollTop;
                        viewKind = kind; activeDlDirInput = null;
                        panel.textContent = '';
                        const body = mk('div');
                        if (fresh) body.className = 'ul-view';
                        let extra = null;
                        if (kind === 'history') {
                            extra = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', 'Очистить');
                            extra.className = 'ul-btn';
                            let armed = false;
                            extra.onclick = (e) => {
                                if (!e.isTrusted) return;
                                if (!armed) {
                                    armed = true; extra.textContent = 'Удалить всё?'; extra.style.color = '#f28b82';
                                    setTimeout(() => { armed = false; extra.textContent = 'Очистить'; extra.style.color = '#8ab4f8'; }, 3000);
                                    return;
                                }
                                body.style.transition = 'opacity .2s ease, transform .2s ease';
                                body.style.opacity = '0'; body.style.transform = 'translateY(-8px)';
                                setTimeout(() => ipcPost('HIST_CLEAR'), 220);
                            };
                        } else if (kind === 'downloads') {
                            extra = mk('div', 'display:flex;align-items:center;gap:4px;');
                            const folderBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', '\uD83D\uDCC2 Папка');
                            folderBtn.className = 'ul-btn';
                            folderBtn.title = 'Открыть папку Загрузки в Проводнике';
                            folderBtn.onclick = (e) => { if (e.isTrusted) ipcPost('DL_FOLDER'); };
                            extra.appendChild(folderBtn);
                            if (items.length) {
                                const clrBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#9aa0a6;font-size:12px;user-select:none;', 'Очистить');
                                clrBtn.className = 'ul-btn';
                                let armed = false;
                                clrBtn.onclick = (e) => {
                                    if (!e.isTrusted) return;
                                    if (!armed) {
                                        armed = true; clrBtn.textContent = 'Точно?'; clrBtn.style.color = '#f28b82';
                                        setTimeout(() => { armed = false; clrBtn.textContent = 'Очистить'; clrBtn.style.color = '#9aa0a6'; }, 3000);
                                        return;
                                    }
                                    body.style.transition = 'opacity .2s ease, transform .2s ease';
                                    body.style.opacity = '0'; body.style.transform = 'translateY(-8px)';
                                    setTimeout(() => ipcPost('DL_CLEAR'), 220);
                                };
                                extra.appendChild(clrBtn);
                            }
                        }
                        const headerTitle = kind === 'history' ? 'История' : (kind === 'bookmarks' ? 'Закладки' : 'Загрузки');
                        panel.appendChild(panelHeader(headerTitle, extra));
                        panel.appendChild(body);
                        if (!items.length) {
                            const emptyText = kind === 'history'
                                ? 'История пуста'
                                : (kind === 'bookmarks' ? 'Закладок пока нет. Нажмите \u2606 в адресной строке.' : 'Список загрузок пуст');
                            const empty = mk('div', 'padding:18px 16px;color:#9aa0a6;', emptyText);
                            empty.className = 'ul-view';
                            body.appendChild(empty);
                            return;
                        }
                        let lastDay = '';
                        items.forEach((it) => {
                            if (!it || typeof it.u !== 'string') return;
                            if (kind === 'history' || kind === 'downloads') {
                                const day = dayLabel(it.d);
                                if (day !== lastDay) {
                                    lastDay = day;
                                    body.appendChild(mk('div', 'padding:10px 16px 2px;color:#9aa0a6;font-size:11px;text-transform:uppercase;letter-spacing:.04em;', day));
                                }
                            }
                            body.appendChild(listRow(kind, it));
                        });
                        if (!fresh) panel.scrollTop = keep;
                    }

                    function showSettings(cfg) {
                        viewKind = 'settings';
                        panel.textContent = '';
                        const openDirBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', '\uD83D\uDCC2 Папка конфигов');
                        openDirBtn.className = 'ul-btn';
                        openDirBtn.title = 'Открыть папку Falco (settings.txt, filters, scripts)';
                        openDirBtn.onclick = (e) => { if (e.isTrusted) ipcPost('OPEN_APP_DIR'); };

                        panel.appendChild(panelHeader('Настройки', openDirBtn));
                        const body = mk('div', 'padding:12px 16px;display:flex;flex-direction:column;gap:10px;');
                        body.className = 'ul-view';

                        const stopKeys = (el) => ['keydown', 'keyup', 'keypress'].forEach((t) => el.addEventListener(t, (ev) => ev.stopPropagation()));
                        const lbl = (txt) => mk('div', 'font-size:11px;color:#9aa0a6;margin-bottom:3px;', txt);
                        const inpCss = 'width:100%;height:30px;box-sizing:border-box;border:none;border-radius:6px;padding:0 10px;color:#fff;outline:none;font-size:12px;';

                        // Поисковая система
                        const gSearch = mk('div');
                        gSearch.appendChild(lbl('Поисковая система в адресной строке'));
                        const selSearch = document.createElement('select');
                        selSearch.className = 'ul-input';
                        selSearch.style.cssText = inpCss + 'cursor:pointer;';
                        [
                            ['ddg', 'DuckDuckGo'],
                            ['google', 'Google'],
                            ['yandex', 'Яндекс'],
                            ['bing', 'Bing'],
                            ['brave', 'Brave Search'],
                        ].forEach(([val, name]) => {
                            const o = document.createElement('option');
                            o.value = val; o.textContent = name;
                            o.style.background = '#202124';
                            if (cfg.search_engine === val) o.selected = true;
                            selSearch.appendChild(o);
                        });
                        gSearch.appendChild(selSearch);
                        body.appendChild(gSearch);

                        // Стартовая страница
                        const gStart = mk('div');
                        gStart.appendChild(lbl('Стартовая страница (новая вкладка)'));
                        const inpStart = document.createElement('input');
                        inpStart.type = 'text'; inpStart.value = cfg.start_url || 'https://duckduckgo.com';
                        inpStart.className = 'ul-input'; inpStart.style.cssText = inpCss;
                        stopKeys(inpStart);
                        gStart.appendChild(inpStart);
                        body.appendChild(gStart);

                        // Папка загрузок + кнопка Обзор в Проводнике
                        const gDl = mk('div');
                        gDl.appendChild(lbl('Папка для загрузок (пусто — стандартная Загрузки)'));
                        const dlRow = mk('div', 'display:flex;align-items:center;gap:6px;');
                        const inpDl = document.createElement('input');
                        inpDl.type = 'text'; inpDl.value = cfg.download_dir || '';
                        inpDl.placeholder = 'По умолчанию (Downloads)';
                        inpDl.className = 'ul-input';
                        inpDl.style.cssText = inpCss + 'flex:1;min-width:0;';
                        stopKeys(inpDl);
                        activeDlDirInput = inpDl;
                        const browseBtn = mk('div', 'height:30px;padding:0 10px;border-radius:6px;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#8ab4f8;font-size:12px;white-space:nowrap;user-select:none;flex:none;', '\uD83D\uDCC2 Обзор…');
                        browseBtn.className = 'ul-btn';
                        browseBtn.title = 'Выбрать папку через Проводник';
                        browseBtn.onclick = (e) => { if (e.isTrusted) ipcPost('PICK_DL_DIR'); };
                        dlRow.appendChild(inpDl);
                        dlRow.appendChild(browseBtn);
                        gDl.appendChild(dlRow);
                        body.appendChild(gDl);

                        // Доступ к камере и микрофону
                        const gMedia = mk('div');
                        gMedia.appendChild(lbl('Доступ к камере и микрофону'));
                        const selMedia = document.createElement('select');
                        selMedia.className = 'ul-input';
                        selMedia.style.cssText = inpCss + 'cursor:pointer;';
                        [
                            ['0', 'Всегда спрашивать'],
                            ['1', 'Без вопроса для trusted-media.txt'],
                            ['2', 'Разрешать всем HTTPS-сайтам'],
                        ].forEach(([val, name]) => {
                            const o = document.createElement('option');
                            o.value = val; o.textContent = name;
                            o.style.background = '#202124';
                            if (String(cfg.media_mode) === val) o.selected = true;
                            selMedia.appendChild(o);
                        });
                        gMedia.appendChild(selMedia);
                        body.appendChild(gMedia);

                        // Переключатели (чекбоксы)
                        const mkCheck = (text, checked) => {
                            const wrap = mk('label', 'display:flex;align-items:center;gap:8px;cursor:pointer;user-select:none;font-size:12px;color:#e8eaed;');
                            const cb = document.createElement('input');
                            cb.type = 'checkbox'; cb.checked = !!checked;
                            cb.style.cssText = 'accent-color:#8ab4f8;width:15px;height:15px;cursor:pointer;margin:0;flex:none;';
                            wrap.appendChild(cb);
                            wrap.appendChild(mk('span', '', text));
                            body.appendChild(wrap);
                            return cb;
                        };

                        const cbPinBar = mkCheck('Закрепить верхнюю панель вкладок и поиска (не скрывать)', cfg.pin_top_bar);
                        const cbSession = mkCheck('Восстанавливать вкладки при запуске', cfg.restore_session);
                        const cbHist = mkCheck('Сохранять историю посещений', cfg.history_enabled);
                        const cbWebrtc = mkCheck('Скрывать локальный IP в WebRTC (после перезапуска)', cfg.webrtc_hide_ip);
                        const cbSmart = mkCheck('Защита Microsoft SmartScreen (после перезапуска)', cfg.smartscreen);
                        const cbH264 = mkCheck('Принудительно H.264 на YouTube для слабых ПК (после перезапуска)', cfg.yt_force_h264);

                        const saveBtn = mk('div', 'margin-top:6px;padding:8px 14px;border-radius:6px;cursor:pointer;background:#8ab4f8;color:#202124;font-weight:bold;text-align:center;user-select:none;', 'Сохранить настройки');
                        saveBtn.className = 'ul-btn2';
                        saveBtn.onclick = (e) => {
                            if (!e.isTrusted) return;
                            let su = inpStart.value.trim();
                            if (su && !/^https?:\/\//i.test(su)) su = 'https://' + su;
                            const parts = [
                                'start_url=' + (su || 'https://duckduckgo.com'),
                                'search_engine=' + selSearch.value,
                                'restore_session=' + (cbSession.checked ? '1' : '0'),
                                'download_dir=' + inpDl.value.trim(),
                                'history_enabled=' + (cbHist.checked ? '1' : '0'),
                                'media_mode=' + selMedia.value,
                                'smartscreen=' + (cbSmart.checked ? '1' : '0'),
                                'webrtc_hide_ip=' + (cbWebrtc.checked ? '1' : '0'),
                                'yt_force_h264=' + (cbH264.checked ? '1' : '0'),
                                'pin_top_bar=' + (cbPinBar.checked ? '1' : '0'),
                            ];
                            ipcPost('SETTINGS_SAVE|' + parts.join(US));
                            closePanel();
                        };
                        body.appendChild(saveBtn);
                        panel.appendChild(body);
                    }

                    lockGlobal('__ulPickedDlDir', function(folderPath) {
                        if (activeDlDirInput && typeof folderPath === 'string') {
                            activeDlDirInput.value = folderPath;
                        }
                    });

                    lockGlobal('__ulSettings', function(json) {
                        let cfg = null;
                        try { cfg = rApply(rJsonParse, JSON, [String(json)]); } catch (e) {}
                        if (!cfg) return;
                        if (typeof cfg.search_prefix === 'string') searchPrefix = cfg.search_prefix;
                        if (typeof cfg.pin_top_bar === 'boolean' && cfg.pin_top_bar !== pinTopBar) {
                            pinTopBar = cfg.pin_top_bar;
                            applyPinBarLayout();
                        }
                        if (panelOpen && (viewKind === 'loading' || viewKind === 'settings')) {
                            showSettings(cfg);
                        }
                    });

                    lockGlobal('__ulPanel', function(kind, json) {
                        if (!panelOpen) return;
                        let items = [];
                        try { items = rApply(rJsonParse, JSON, [String(json)]); } catch (e) { items = []; }
                        const k = kind === 'bookmarks' ? 'bookmarks' : (kind === 'downloads' ? 'downloads' : 'history');
                        showList(k, Array.isArray(items) ? items : []);
                    });
                    const menuBtn = navButton('\u22EF', 'Меню', (e) => { if (e && e.isTrusted) { if (panelOpen) closePanel(); else openPanel(); } });
                    menuBtn.style.marginRight = '0';
                    root.addEventListener('pointerdown', (e) => {
                        if (!e.isTrusted) return;
                        const t = e.target;
                        if (popOpen && !bmPop.contains(t) && t !== starBtn) closeBmPop(true);
                        if (panelOpen && !panel.contains(t) && t !== menuBtn) closePanel();
                    }, true);
                    window.addEventListener('pointerdown', (e) => {
                        if (!(panelOpen || popOpen) || !e.isTrusted) return;
                        if (e.target === container || container.contains(e.target)) return;
                        if (popOpen) closeBmPop(true);
                        if (panelOpen) closePanel();
                    }, true);
                    root.appendChild(panel); root.appendChild(bmPop); root.appendChild(findBar); root.appendChild(sug);

                    urlRow.appendChild(input); urlRow.appendChild(starBtn); urlRow.appendChild(popBtn); urlRow.appendChild(menuBtn); urlRow.appendChild(progBar);
                    root.appendChild(topBar); root.appendChild(urlRow);

                    ui = {
                        container, trigger, tabsRow, input, show, hide, inputFocused, startProgress, finishProgress,
                        openFind, findStep: runFind, isFindOpen: () => findOpen,
                        closeOverlays: () => {
                            const any = popOpen || panelOpen || findOpen;
                            if (popOpen) closeBmPop(false);
                            if (panelOpen) closePanel();
                            if (findOpen) closeFind();
                            return any;
                        },
                        toggleBookmark: onStar,
                        openHistory: () => { if (!panelOpen) openPanel(); openLoading('История'); ipcPost('HIST_LIST'); },
                        openDownloads: () => { if (!panelOpen) openPanel(); openLoading('Загрузки'); ipcPost('DL_LIST'); },
                        openSettings: () => { if (!panelOpen) openPanel(); openLoading('Настройки'); ipcPost('SETTINGS_GET'); },
                    };
                    lockGlobal('__ulFlash', function() {
                        if (!ui) return;
                        ui.container.style.top = '0';
                        setTimeout(() => { if (ui && !ui.container.matches(':hover')) ui.hide(); }, 2000);
                    });
                    drawTabs();
                    applyPinBarLayout();
                    ipcPost('GET_TABS');
                }

                // ---- Ручное скрытие элементов: Ctrl+Shift+X (выбрать), Ctrl+Shift+Z (отменить последнее) ----
                const RULES_KEY = 'ultralight_hidden';
                let userRules = [];
                try { userRules = JSON.parse(localStorage.getItem(RULES_KEY) || '[]'); } catch (e) { userRules = []; }
                if (!Array.isArray(userRules)) userRules = [];
                function saveRules() { try { localStorage.setItem(RULES_KEY, JSON.stringify(userRules)); } catch (e) {} }
                function ensureUserCss() {
                    const rules = userRules.map((s) => s + ' { display: none !important; }').join('\n');
                    if (ulSetCss('user', rules)) return;
                    if (!document.head) return;
                    let st = document.getElementById('ultralight-user-css');
                    if (!st) { st = document.createElement('style'); st.id = 'ultralight-user-css'; document.head.appendChild(st); }
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
                    t.style.cssText = 'position:fixed;left:16px;bottom:16px;max-width:60%;padding:10px 14px;background:#202124;color:#fff;font:13px sans-serif;border-radius:8px;z-index:2147483647;box-shadow:0 2px 10px rgba(0,0,0,.5);pointer-events:none;opacity:0;transform:translateY(8px);transition:opacity .2s ease,transform .2s cubic-bezier(.2,.8,.3,1);zoom:1 !important;';
                    document.documentElement.appendChild(t);
                    requestAnimationFrame(() => requestAnimationFrame(() => { t.style.opacity = '1'; t.style.transform = 'none'; }));
                    setTimeout(() => { t.style.opacity = '0'; t.style.transform = 'translateY(8px)'; }, 3250);
                    setTimeout(() => t.remove(), 3500);
                }

                // ---- Компактная плашка-пилюля загрузки и масштаба в правом нижнем углу ----
                let dlChip = null, dlTimer = 0;
                lockGlobal('__ulDlNotice', function(state, name, fullPath) {
                    if (!document.documentElement) return;
                    clearTimeout(dlTimer);
                    if (!dlChip || !dlChip.isConnected) {
                        dlChip = document.createElement('div');
                        dlChip.style.cssText = 'position:fixed;right:14px;bottom:14px;max-width:270px;height:32px;padding:0 12px;background:rgba(32,33,36,.95);color:#e8eaed;border:1px solid #3c4043;border-radius:16px;z-index:2147483647;box-shadow:0 4px 12px rgba(0,0,0,.45);display:flex;align-items:center;gap:7px;font:12px/1 "Segoe UI",system-ui,sans-serif;user-select:none;opacity:0;transform:translateY(6px);transition:opacity .18s ease,transform .18s cubic-bezier(.2,.8,.3,1),border-color .15s ease;zoom:1 !important;';
                        document.documentElement.appendChild(dlChip);
                    }
                    dlChip.replaceChildren();
                    const icon = document.createElement('span');
                    icon.style.cssText = 'font-size:13px;flex:none;font-weight:bold;';
                    const txt = document.createElement('span');
                    txt.style.cssText = 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;min-width:0;';
                    txt.textContent = String(name || '');

                    if (state === 'start') {
                        icon.textContent = '\u2B07';
                        icon.style.color = '#8ab4f8';
                        dlChip.style.cursor = 'default';
                        dlChip.title = 'Скачивается: ' + name;
                        dlChip.onclick = null;
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    } else if (state === 'ok') {
                        icon.textContent = '\u2714';
                        icon.style.color = '#81c995';
                        dlChip.style.cursor = 'pointer';
                        dlChip.title = fullPath ? 'Нажмите, чтобы показать в папке' : '';
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                        if (fullPath) {
                            const openIcon = document.createElement('span');
                            openIcon.textContent = '\uD83D\uDCC2';
                            openIcon.style.cssText = 'font-size:12px;flex:none;opacity:.85;margin-left:2px;';
                            dlChip.appendChild(openIcon);
                            dlChip.onmouseenter = () => { dlChip.style.borderColor = '#8ab4f8'; };
                            dlChip.onmouseleave = () => { dlChip.style.borderColor = '#3c4043'; };
                            dlChip.onclick = (e) => {
                                if (!e.isTrusted) return;
                                ipcPost('DL_SHOW|' + fullPath);
                                dlChip.style.opacity = '0';
                                dlChip.style.transform = 'translateY(6px)';
                                setTimeout(() => { if (dlChip) { dlChip.remove(); dlChip = null; } }, 180);
                            };
                        } else {
                            dlChip.onclick = null;
                        }
                    } else if (state === 'info') {
                        icon.textContent = '\uD83D\uDD0D';
                        icon.style.color = '#8ab4f8';
                        dlChip.style.cursor = 'pointer';
                        dlChip.title = 'Нажмите, чтобы сбросить масштаб (100%)';
                        dlChip.onclick = (e) => { if (e.isTrusted) stepPageZoom(0); };
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    } else {
                        icon.textContent = '\u2716';
                        icon.style.color = '#f28b82';
                        dlChip.style.cursor = 'default';
                        dlChip.title = 'Ошибка загрузки';
                        dlChip.onclick = null;
                        dlChip.appendChild(icon);
                        dlChip.appendChild(txt);
                    }
                    requestAnimationFrame(() => {
                        if (dlChip) { dlChip.style.opacity = '1'; dlChip.style.transform = 'none'; }
                    });
                    const delay = state === 'start' ? 12000 : (state === 'info' ? 1600 : 3800);
                    dlTimer = setTimeout(() => {
                        if (!dlChip) return;
                        dlChip.style.opacity = '0';
                        dlChip.style.transform = 'translateY(6px)';
                        setTimeout(() => { if (dlChip) { dlChip.remove(); dlChip = null; } }, 200);
                    }, delay);
                });

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
                    PICK_BLOCKED.forEach((t) => window.addEventListener(t, swallow, true));
                    window.removeEventListener('keydown', onPickKey, true);
                    if (hl) { hl.remove(); hl = null; }
                }
                function undoRule() {
                    const s = userRules.pop();
                    saveRules(); ensureUserCss();
                    toast(s ? 'Правило удалено: ' + s : 'Для этого сайта нет сохранённых правил');
                }

                // ---- Сохранение прокрутки, позиции видео и текста в полях ввода ----
                const STATE_KEY = 'ultralight_tabstate';
                const STATE_TTL = 12 * 60 * 60 * 1000;
                const RESTART_TTL = 7 * 24 * 60 * 60 * 1000;
                const STATE_MAX = 60;
                const RUN_ID = '__RUN_ID__';
                const RESTORE_URLS = __RESTORE_URLS__;
                const FORM_MAX = 20, FORM_LEN = 2000;
                let scrollDone = true, videoDone = true, formDone = true;
                let switching = false;
                let scrollTimer = null;

                function stateKey(h) {
                    try {
                        const u = new URL(h || window.location.href);
                        if (/(^|\.)youtube\.com$/i.test(u.hostname) && u.pathname === '/watch') return u.origin + '/watch?v=' + (u.searchParams.get('v') || '');
                        return u.origin + u.pathname + u.search;
                    } catch (e) { return h || window.location.href; }
                }
                const RESTORE_KEYS = new Set(RESTORE_URLS.map((u) => stateKey(u)));
                function readStates() {
                    try {
                        const o = JSON.parse(localStorage.getItem(STATE_KEY) || '{}');
                        return (o && typeof o === 'object' && !Array.isArray(o)) ? o : {};
                    } catch (e) { return {}; }
                }
                function writeStates(o) {
                    try {
                        const nowTs = Date.now();
                        Object.keys(o).forEach((k) => { if (!o[k] || nowTs - (o[k].ts || 0) > RESTART_TTL) delete o[k]; });
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

                function fieldId(el) {
                    if (el.id) return '#' + el.id;
                    if (el.name) return '@' + el.name;
                    return null;
                }
                function collectForm() {
                    const f = {};
                    let n = 0;
                    const els = document.querySelectorAll('textarea, input[type=text], input[type=search], input:not([type])');
                    for (let i = 0; i < els.length && n < FORM_MAX; i++) {
                        const el = els[i];
                        if (ui && ui.container.contains(el)) continue;
                        const id = fieldId(el);
                        if (!id || !el.value || el.value.length > FORM_LEN) continue;
                        const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
                        if (/password|cc-|one-time|username|email|tel|address|postal|bday/.test(ac)) continue;
                        if (/pass|card|cvv|cvc|otp|secret|token|login|e-?mail|phone|addr|iban|ssn|passport|birth|account|\bpin\b|\bsms\b/i.test((el.id || '') + ' ' + (el.name || ''))) continue;
                        f[id] = el.value; n++;
                    }
                    return n ? f : null;
                }
                function startFormRestore(f) {
                    const pending = Object.assign({}, f);
                    let ticks = 0, timer = null;
                    formDone = false;
                    const onUser = (e) => {
                        if (!e.isTrusted) return;
                        const id = e.target && e.target.nodeType === 1 ? fieldId(e.target) : null;
                        if (id) delete pending[id];
                    };
                    const stop = () => {
                        clearInterval(timer);
                        document.removeEventListener('input', onUser, true);
                        formDone = true;
                    };
                    document.addEventListener('input', onUser, true);
                    timer = setInterval(() => {
                        ticks++;
                        Object.keys(pending).forEach((id) => {
                            const el = id.charAt(0) === '#' ? document.getElementById(id.slice(1)) : document.getElementsByName(id.slice(1))[0];
                            if (!el) return;
                            const isTa = el instanceof HTMLTextAreaElement;
                            if (isTa || el instanceof HTMLInputElement) {
                                if (!el.value) {
                                    const d = Object.getOwnPropertyDescriptor(isTa ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, 'value');
                                    if (d && d.set) d.set.call(el, String(pending[id])); else el.value = String(pending[id]);
                                    el.dispatchEvent(new Event('input', { bubbles: true }));
                                    el.dispatchEvent(new Event('change', { bubbles: true }));
                                }
                            }
                            delete pending[id];
                        });
                        if (!Object.keys(pending).length || ticks >= 40) stop();
                    }, 250);
                }

                function saveState() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const key = stateKey();
                    const states = readStates();
                    const old = states[key];
                    let x = Math.round(window.scrollX), y = Math.round(window.scrollY), t = 0;
                    let f = collectForm();
                    const v = mainVideo();
                    if (v && !window.location.pathname.startsWith('/shorts') && v.currentTime > 5 && v.currentTime < v.duration - 10) {
                        t = Math.floor(v.currentTime);
                    }
                    if (old) {
                        if (!scrollDone) { x = old.x || 0; y = old.y || 0; }
                        if (!videoDone) { t = old.t || 0; }
                        if (!formDone) { f = old.f || null; }
                    }
                    if (!x && !y && !t && !f) delete states[key];
                    else states[key] = { x: x, y: y, t: t, f: f || undefined, sw: switching, run: RUN_ID, ts: Date.now() };
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
                    INPUT_EVENTS.forEach((ev) => window.addEventListener(ev, stop, { capture: true, passive: true }));
                    const step = () => {
                        if (stopped) return;
                        ticks++;
                        const root = document.documentElement;
                        const h = Math.max(root ? root.scrollHeight : 0, document.body ? document.body.scrollHeight : 0);
                        const maxY = h - window.innerHeight;
                        if (maxY >= y - 2) {
                            window.scrollTo({ left: x, top: y, behavior: 'instant' });
                            if (Math.abs(window.scrollY - y) <= 3) stable++; else stable = 0;
                            if (stable >= 2) stop();
                        } else if (maxY > 0 && ticks <= 12) {
                            window.scrollTo({ left: x, top: maxY, behavior: 'instant' });
                        }
                        if (ticks >= 30) stop();
                    };
                    document.addEventListener('DOMContentLoaded', step, { once: true });
                    timer = setInterval(step, 120);
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
                    document.addEventListener('loadedmetadata', onMeta, true);
                }

                (function restoreOnLoad() {
                    if (!/^https?:$/.test(window.location.protocol)) return;
                    const states = readStates();
                    const key = stateKey();
                    const st = states[key];
                    if (!st) return;
                    const age = Date.now() - (st.ts || 0);
                    const bySwitch = !!st.sw && age <= STATE_TTL;
                    const byRestart = RESTORE_KEYS.has(key) && st.run !== RUN_ID && age <= RESTART_TTL;
                    if (!bySwitch && !byRestart) return;
                    st.sw = false; st.run = RUN_ID; writeStates(states);
                    if (st.f && typeof st.f === 'object') startFormRestore(st.f);
                    if (st.y > 0 || st.x > 0) { scrollDone = false; startScrollRestore(st.x || 0, st.y || 0); }
                    if (st.t > 5 && !window.location.pathname.startsWith('/shorts')) { videoDone = false; startVideoRestore(st.t); }
                })();

                window.addEventListener('scroll', () => {
                    if (scrollTimer) clearTimeout(scrollTimer);
                    const k = stateKey();
                    scrollTimer = setTimeout(() => { scrollTimer = null; if (stateKey() === k) saveState(); }, 400);
                }, { passive: true });
                window.addEventListener('pagehide', () => saveState());
                window.addEventListener('pageshow', (e) => {
                    if (e.persisted && document.body) {
                        document.body.style.opacity = '1';
                        if (ui && ui.finishProgress) ui.finishProgress();
                    }
                });
                let inputTimer = null;
                document.addEventListener('input', (e) => {
                    if (!e.isTrusted || (ui && e.target && ui.container.contains(e.target))) return;
                    if (inputTimer) clearTimeout(inputTimer);
                    inputTimer = setTimeout(() => { inputTimer = null; saveState(); }, 800);
                }, { capture: true, passive: true });
                setInterval(() => {
                    if (document.hidden) return;
                    const v = mainVideo();
                    if (v && !v.paused) saveState();
                }, 10000);
                document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') saveState(); });
                lockGlobal('__ulSaveForSwitch', () => { switching = true; saveState(); });

                // Масштабирование ТОЛЬКО страницы колесом мыши с зажатым Ctrl
                let ctrlDown = false;
                window.addEventListener('keydown', (e) => { if (e.key === 'Control') ctrlDown = true; }, true);
                window.addEventListener('keyup', (e) => { if (e.key === 'Control') ctrlDown = false; }, true);
                window.addEventListener('blur', () => { ctrlDown = false; });

                let lastWheelZoom = 0;
                const onCtrlWheel = (e) => {
                    if (!e.ctrlKey && !e.metaKey && !ctrlDown) return;
                    e.preventDefault();
                    e.stopPropagation();
                    e.stopImmediatePropagation();
                    const now = perfNow();
                    if (now - lastWheelZoom < 35) return;
                    lastWheelZoom = now;
                    const dy = e.deltaY !== undefined ? e.deltaY : -(e.wheelDelta || 0);
                    if (dy === 0) return;
                    stepPageZoom(dy < 0 ? 1 : -1);
                };
                window.addEventListener('wheel', onCtrlWheel, { passive: false, capture: true });
                document.addEventListener('wheel', onCtrlWheel, { passive: false, capture: true });
                window.addEventListener('mousewheel', onCtrlWheel, { passive: false, capture: true });

                window.addEventListener('keydown', (e) => {
                    if (!ui || !e.isTrusted) return;
                    if (e.key === 'Escape' && ui.closeOverlays()) { e.preventDefault(); return; }
                    if (e.key === 'F11') { e.preventDefault(); ipcPost('WIN_FULLSCREEN'); return; }
                    if (e.key === 'F3') {
                        e.preventDefault();
                        if (!ui.isFindOpen()) ui.openFind();
                        else ui.findStep(e.shiftKey);
                        return;
                    }
                    const k = e.key.toLowerCase();
                    const code = e.code || '';
                    if (e.altKey && !e.ctrlKey) {
                        if (e.key === 'ArrowLeft') { e.preventDefault(); ui.startProgress(); history.back(); }
                        else if (e.key === 'ArrowRight') { e.preventDefault(); ui.startProgress(); history.forward(); }
                        return;
                    }
                    if (e.key === 'F5' || (e.ctrlKey && !e.shiftKey && (k === 'r' || code === 'KeyR'))) {
                        e.preventDefault(); ui.startProgress(); location.reload(); return;
                    }
                    if (e.ctrlKey && e.shiftKey && !e.altKey) {
                        if (k === 'x' || code === 'KeyX') { e.preventDefault(); startPick(); return; }
                        if (k === 'z' || code === 'KeyZ') { e.preventDefault(); undoRule(); return; }
                        if (k === 't' || code === 'KeyT') {
                            e.preventDefault();
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            ui.startProgress();
                            ipcPost('REOPEN_TAB');
                            return;
                        }
                    }
                    if (!e.ctrlKey || e.altKey) return;
                    // Масштаб только страницы: Ctrl + (+ / - / 0)
                    if (e.key === '+' || e.key === '=' || code === 'Equal' || code === 'NumpadAdd') {
                        e.preventDefault(); stepPageZoom(1); return;
                    }
                    if (e.key === '-' || e.key === '_' || code === 'Minus' || code === 'NumpadSubtract') {
                        e.preventDefault(); stepPageZoom(-1); return;
                    }
                    if (e.key === '0' || code === 'Digit0' || code === 'Numpad0') {
                        e.preventDefault(); stepPageZoom(0); return;
                    }
                    if ((k === 'f' || code === 'KeyF') && !e.shiftKey) {
                        e.preventDefault(); ui.openFind(); return;
                    }
                    if (k === 'l' || code === 'KeyL') { e.preventDefault(); ui.show(); ui.input.focus(); }
                    else if ((k === 't' || code === 'KeyT') && !e.shiftKey) {
                        e.preventDefault();
                        try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                        ui.startProgress();
                        ipcPost('new_tab');
                    }
                    else if (k === 'w' || code === 'KeyW') { e.preventDefault(); ipcPost('CLOSE_TAB|-1'); }
                    else if (k === 'tab') {
                        e.preventDefault();
                        if (tabsState.list.length > 1) {
                            optimisticSelectTab((tabsState.active + 1) % tabsState.list.length);
                        }
                        ipcPost('next_tab');
                    }
                    else if ((k === 'd' || code === 'KeyD') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.toggleBookmark(); }
                    else if ((k === 'h' || code === 'KeyH') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.openHistory(); }
                    else if ((k === 'j' || code === 'KeyJ') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.openDownloads(); }
                    else if ((e.key === ',' || code === 'Comma') && !e.shiftKey) { e.preventDefault(); ui.show(); ui.openSettings(); }
                }, true);

                function attachUI() {
                    const root = document.documentElement;
                    if (!root) return;
                    if (!ui) buildUI();
                    if (!ui.trigger.isConnected) root.appendChild(ui.trigger);
                    if (!ui.container.isConnected) root.appendChild(ui.container);
                    if (pageZoom !== 1.0 && document.body && !document.body.style.zoom) {
                        document.body.style.zoom = String(pageZoom);
                    }
                }

                let observedRoot = null, observedHead = null, docObserved = false;
                function keepAlive() {
                    if (!ui || !ui.container.isConnected || !ui.trigger.isConnected) attachUI();
                    injectCss();
                    applyCosmetic();
                    ensureUserCss();
                    applyPinBarLayout();
                    observeHead();
                    watchTitle();
                }
                function observeHead() {
                    const h = document.head;
                    if (h && h !== observedHead) {
                        observedHead = h;
                        new MutationObserver(keepAlive).observe(h, { childList: true });
                    }
                }
                function watch() {
                    const root = document.documentElement;
                    if (!root) { requestAnimationFrame(watch); return; }
                    attachUI();
                    if (!docObserved) {
                        docObserved = true;
                        new MutationObserver(() => { if (document.documentElement !== observedRoot) watch(); }).observe(document, { childList: true });
                    }
                    observeHead();
                    if (root === observedRoot) return;
                    observedRoot = root;
                    new MutationObserver(keepAlive).observe(root, { childList: true });
                }
                watch();

                // ---- Контекстное меню (правая кнопка) ----
                let ctxMenu = null;
                function closeCtx() {
                    if (!ctxMenu) return;
                    const old = ctxMenu; ctxMenu = null;
                    old.style.pointerEvents = 'none'; old.style.opacity = '0';
                    setTimeout(() => old.remove(), 120);
                    window.removeEventListener('mousedown', ctxOutside, true);
                    window.removeEventListener('keydown', ctxKey, true);
                    window.removeEventListener('scroll', closeCtx, true);
                    window.removeEventListener('blur', closeCtx);
                    window.removeEventListener('resize', closeCtx);
                    try { if (ui && ui.hide && !ui.container.matches(':hover')) ui.hide(); } catch (err) {}
                }
                function ctxOutside(e) { if (ctxMenu && !ctxMenu.contains(e.target)) closeCtx(); }
                function ctxKey(e) { if (e.key === 'Escape') { e.preventDefault(); closeCtx(); } }

                function copyText(t) {
                    const fallback = () => {
                        try {
                            const ta = document.createElement('textarea');
                            ta.value = t; ta.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0;';
                            (document.body || document.documentElement).appendChild(ta);
                            ta.select(); document.execCommand('copy'); ta.remove();
                        } catch (e) {}
                    };
                    try {
                        if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(t).catch(fallback);
                        else fallback();
                    } catch (e) { fallback(); }
                }
                lockGlobal('__ulCopyText', (t) => { if (typeof t === 'string' && t.length <= 4096) copyText(t); });
                function openInWindow(u) {
                    try {
                        const abs = new URL(u, window.location.href).href;
                        if (/^https?:\/\//i.test(abs)) ipcPost('OPEN_WINDOW|' + abs);
                    } catch (e) {}
                }
                function triggerDownload(u) {
                    try {
                        const a = document.createElement('a');
                        a.href = u;
                        a.setAttribute('download', '');
                        (document.body || document.documentElement).appendChild(a);
                        origElemClick.call(a);
                        a.remove();
                    } catch (e) {}
                }

                function showCtx(e) {
                    closeCtx();
                    const t = e.target;
                    const items = [];
                    const sep = () => { if (items.length && items[items.length - 1] !== null) items.push(null); };
                    const isHttp = (u) => /^https?:\/\//i.test(u);

                    // Ссылка
                    const link = t.closest ? t.closest('a[href], area[href]') : null;
                    if (link) {
                        let abs = '';
                        try { abs = new URL(typeof link.href === 'string' ? link.href : link.getAttribute('href'), window.location.href).href; } catch (err) {}
                        if (abs) {
                            if (isHttp(abs) || abs.startsWith('blob:') || abs.startsWith('data:')) {
                                if (isHttp(abs)) {
                                    items.push({ label: 'Открыть ссылку в новой вкладке', fn: () => openInTab(abs, true) });
                                    items.push({ label: 'Открыть ссылку в новом окне', fn: () => openInWindow(abs) });
                                }
                                items.push({ label: 'Скачать по ссылке', fn: () => triggerDownload(abs) });
                            }
                            items.push({ label: 'Копировать адрес ссылки', fn: () => copyText(abs) });
                            sep();
                        }
                    }
                    // Изображение
                    const img = t.closest ? t.closest('img') : null;
                    const src = img ? (img.currentSrc || img.src || '') : '';
                    if (src && (isHttp(src) || src.startsWith('blob:') || src.startsWith('data:'))) {
                        if (isHttp(src)) {
                            items.push({ label: 'Открыть изображение в новой вкладке', fn: () => openInTab(src, true) });
                        }
                        items.push({ label: 'Скачать изображение', fn: () => triggerDownload(src) });
                        items.push({ label: 'Копировать адрес изображения', fn: () => copyText(src) });
                        sep();
                    }
                    // Выделенный текст
                    let sel = '';
                    try { sel = String(window.getSelection ? window.getSelection() : '').trim(); } catch (err) {}
                    if (sel) {
                        const short = sel.length > 28 ? sel.slice(0, 28) + '…' : sel;
                        items.push({ label: 'Копировать', fn: () => copyText(sel) });
                        items.push({ label: 'Найти «' + short.replace(/\s+/g, ' ') + '» в поиске', fn: () => openInTab(searchPrefix + encodeURIComponent(sel.slice(0, 200)), false) });
                        sep();
                    }
                    // Видео: картинка в картинке
                    let vid = null;
                    try { vid = (document.elementsFromPoint(e.clientX, e.clientY) || []).find((el) => el.tagName === 'VIDEO') || null; } catch (err) {}
                    if (vid && document.pictureInPictureEnabled && !vid.disablePictureInPicture) {
                        items.push({
                            label: document.pictureInPictureElement === vid ? 'Выйти из режима «картинка в картинке»' : 'Картинка в картинке',
                            fn: () => {
                                if (document.pictureInPictureElement === vid) document.exitPictureInPicture().catch(() => {});
                                else vid.requestPictureInPicture().catch(() => {});
                            }
                        });
                        sep();
                    }
                    // Страница
                    items.push({ label: 'Назад', fn: () => history.back() });
                    items.push({ label: 'Вперёд', fn: () => history.forward() });
                    items.push({ label: 'Обновить', fn: () => location.reload() });
                    sep();
                    items.push({ label: 'Найти на странице… (Ctrl+F)', fn: () => { if (ui) ui.openFind(); } });
                    items.push({ label: 'Вернуть закрытую вкладку (Ctrl+Shift+T)', fn: () => ipcPost('REOPEN_TAB') });
                    sep();
                    items.push({ label: 'Печать…', fn: () => window.print() });
                    items.push({ label: 'Выделить всё', fn: () => { try { window.getSelection().selectAllChildren(document.body || document.documentElement); } catch (err) {} } });
                    items.push({ label: 'Копировать адрес страницы', fn: () => copyText(window.location.href) });
                    items.push({ label: 'Вывести текущую вкладку в отдельное окно', fn: () => ipcPost('DETACH_TAB') });
                    openCtx(items, e.clientX, e.clientY);
                }

                // Меню правой кнопки на вкладке (как в обычных браузерах)
                function showTabCtx(i, e) {
                    const list = tabsState.list;
                    const pn = tabsState.pinned | 0;
                    const isPinned = i < pn;
                    const isActive = i === tabsState.active;
                    const cmd = (c, navigates) => {
                        if (navigates) {
                            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (err) {}
                            if (ui && ui.startProgress) ui.startProgress();
                        }
                        ipcPost('TAB_CMD|' + c + '|' + i);
                    };
                    const items = [];
                    items.push({ label: 'Новая вкладка справа', fn: () => cmd('new_right', true) });
                    items.push(null);
                    if (isActive) items.push({ label: 'Обновить', fn: () => { if (ui) ui.startProgress(); location.reload(); } });
                    items.push({ label: 'Дублировать', fn: () => cmd('dup', true) });
                    items.push({ label: isPinned ? 'Открепить вкладку' : 'Закрепить вкладку', fn: () => cmd(isPinned ? 'unpin' : 'pin', false) });
                    items.push({ label: 'Копировать адрес вкладки', fn: () => { if (isActive) copyText(window.location.href); else cmd('copy_url', false); } });
                    items.push({ label: 'Открыть в отдельном окне', fn: () => ipcPost('DETACH_TAB|' + i) });
                    items.push(null);
                    items.push({ label: 'Закрыть', fn: () => closeTab(i, tabEls[i]) });
                    items.push({ label: 'Закрыть другие вкладки', disabled: !list.some((_, j) => j !== i && j >= pn), fn: () => cmd('close_others', true) });
                    items.push({ label: 'Закрыть вкладки справа', disabled: !list.some((_, j) => j > i && j >= pn), fn: () => cmd('close_right', true) });
                    items.push(null);
                    items.push({ label: 'Вернуть закрытую вкладку (Ctrl+Shift+T)', fn: () => ipcPost('REOPEN_TAB') });
                    openCtx(items, e.clientX, e.clientY);
                }

                function openCtx(items, cx, cy) {
                    closeCtx();
                    while (items.length && items[items.length - 1] === null) items.pop();

                    // Прикрепляем контекстное меню к documentElement (<html>), чтобы зум body его не увеличивал
                    const m = document.createElement('div');
                    m.style.cssText = 'position:fixed;left:0;top:0;z-index:2147483647;min-width:230px;max-width:360px;padding:6px 0;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:8px;box-shadow:0 6px 24px rgba(0,0,0,.5);font:13px/1.4 "Segoe UI",system-ui,sans-serif;user-select:none;-webkit-user-select:none;zoom:1 !important;';
                    m.addEventListener('mousedown', (ev) => ev.preventDefault());
                    m.addEventListener('contextmenu', (ev) => ev.preventDefault());
                    m.style.transformOrigin = 'top left'; m.style.opacity = '0'; m.style.transform = 'scale(.96)';
                    m.style.transition = 'opacity .12s ease, transform .12s cubic-bezier(.2,.8,.3,1)';
                    items.forEach((it) => {
                        const row = document.createElement('div');
                        if (it === null) {
                            row.style.cssText = 'height:1px;margin:6px 0;background:#3c4043;';
                        } else if (it.disabled) {
                            row.textContent = it.label;
                            row.style.cssText = 'padding:6px 18px;color:#6f7378;cursor:default;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;';
                        } else {
                            row.textContent = it.label;
                            row.style.cssText = 'padding:6px 18px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;transition:background-color .12s ease;';
                            row.onmouseover = () => { row.style.background = '#303134'; };
                            row.onmouseout = () => { row.style.background = 'transparent'; };
                            row.onclick = (ev) => { closeCtx(); if (!ev.isTrusted) return; try { it.fn(); } catch (err) {} };
                        }
                        m.appendChild(row);
                    });
                    document.documentElement.appendChild(m);
                    const w = m.offsetWidth, h = m.offsetHeight;
                    m.style.left = Math.max(4, Math.min(cx, window.innerWidth - w - 4)) + 'px';
                    m.style.top = Math.max(4, Math.min(cy, window.innerHeight - h - 4)) + 'px';
                    ctxMenu = m;
                    requestAnimationFrame(() => requestAnimationFrame(() => { m.style.opacity = '1'; m.style.transform = 'none'; }));
                    window.addEventListener('mousedown', ctxOutside, true);
                    window.addEventListener('keydown', ctxKey, true);
                    window.addEventListener('scroll', closeCtx, true);
                    window.addEventListener('blur', closeCtx);
                    window.addEventListener('resize', closeCtx);
                }

                window.addEventListener('contextmenu', (e) => {
                    if (e.defaultPrevented || e.shiftKey) return;
                    const t = e.target;
                    if (!t || t.nodeType !== 1) return;
                    if (ui && ui.container.contains(t)) { e.preventDefault(); return; }
                    if (t.isContentEditable || (t.closest && t.closest('input, textarea, select'))) return;
                    e.preventDefault();
                    showCtx(e);
                }, false);

                if (PERF && window === window.top) {
                    const initMs = performance.now() - perfT0;
                    window.addEventListener('load', () => setTimeout(() => {
                        try {
                            const n = performance.getEntriesByType('navigation')[0];
                            if (!n) return;
                            ipcPost('PERF|' + Math.round(initMs) + '|' + Math.round(n.responseStart) + '|' + Math.round(n.domContentLoadedEventEnd) + '|' + Math.round(n.loadEventEnd) + '|' + window.location.hostname);
                        } catch (e) {}
                    }, 0));
                }

            } catch (err) { console.error("Script error: ", err); }
        })();