(function() {
    try {
        const PERF = __PERF__;
        const NATIVE = __NATIVE__;
        let searchPrefix = '__SEARCH_URL__';
        let pinTopBar = __PIN_BAR__;
        const INCOGNITO = __INCOGNITO__;

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
        const runIdle = window.requestIdleCallback
            ? (fn) => window.requestIdleCallback(fn, { timeout: 500 })
            : (fn) => setTimeout(fn, 16);
        // id этого документа: Rust принимает SYNC_URL/TITLE только от документа, объявившегося через LOAD_URL
        const DOC_ID = Math.random().toString(36).slice(2, 12) + Date.now().toString(36);
        if (window === window.top) ipcPost('LOAD_URL|' + DOC_ID + '|' + window.location.href);

        // 1. БЛОКИРОВЩИК И БЕЛЫЙ СПИСОК ("ЩИТ")
        const adRegex = /(doubleclick\.net|google-analytics\.com|googlesyndication\.com|adservice\.google\.com|mc\.yandex\.ru|an\.yandex\.ru|amazon-adsystem\.com|appnexus\.com|criteo\.com|taboola\.com|yadro\.ru|top-fwz1\.mail\.ru|propellerads|popads|adk2x|betboom|1xbet|fonbet|melbet|awaps\.|a-ads\.com|mgid\.com|adskeeper|exoclick|clickadu|trafficstars|adfox|adriver|\/pagead\/|\/get_midroll_info)/i;
        const curHost = window.location.hostname.toLowerCase();
        const isYT = /(^|\.)youtube(-nocookie)?\.com$/.test(curHost);
        // Сайты без рекламы (ИИ-чаты, документы, репозитории): отключаем тяжёлый DOM-сканер и 3000 косметических правил
        const isCleanApp = /(^|\.)(gemini\.google\.com|aistudio\.google\.com|notebooklm\.google\.com|docs\.google\.com|drive\.google\.com|mail\.google\.com|chatgpt\.com|openai\.com|claude\.ai|deepseek\.com|grok\.com|x\.ai|perplexity\.ai|copilot\.microsoft\.com|poe\.com|mistral\.ai|huggingface\.co|github\.com|gitlab\.com)$/.test(curHost);

        let WL_SET = new Set('__WHITELIST__'.split(',').filter(Boolean));
        function hostInSet(set, h) {
            while (h) {
                if (set.has(h)) return true;
                const i = h.indexOf('.');
                if (i < 0) return false;
                h = h.slice(i + 1);
            }
            return false;
        }
        let pageWhitelisted = hostInSet(WL_SET, curHost);

        function urlOf(x) {
            try { return typeof x === 'string' ? x : (x && x.url) ? x.url : String(x); } catch (e) { return ''; }
        }
        let AD_SET = null;
        function hostBlocked(h) {
            if (!AD_SET) AD_SET = new Set('__AD_HOSTS__'.split(',').filter(Boolean));
            return hostInSet(AD_SET, h);
        }
        function isBlocked(x) {
            if (isYT || isCleanApp || pageWhitelisted) return false;
            const u = urlOf(x);
            if (!u || u.startsWith('data:') || u.startsWith('blob:')) return false;
            if (adRegex.test(u)) return true;
            // Относительные пути того же домена (/api/..., ./...) не могут вести на сторонний рекламный хост
            if (u.charCodeAt(0) === 47 && u.charCodeAt(1) !== 47) return false;
            try {
                const h = new URL(u, window.location.href).hostname.toLowerCase();
                return h !== curHost && hostBlocked(h);
            } catch (e) { return false; }
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

        function isInteractiveTarget(el, checkCursor) {
            if (!el || el.nodeType !== 1 || el.isContentEditable) return false;
            if (el.closest && el.closest('a, area, button, input, select, textarea, summary, [role="button"], [role="link"], [tabindex]')) return true;
            if (checkCursor) {
                try { if (window.getComputedStyle(el).cursor === 'pointer') return true; } catch (e) {}
            }
            return false;
        }

        const recordGesture = (e) => {
            if (!e || !e.isTrusted) return;
            if (e.type === 'keydown') {
                if (e.key !== 'Enter' && e.key !== ' ') return;
                lastGestureInteractive = isInteractiveTarget(e.target, false);
            } else {
                if (e.type === 'auxclick' && e.button !== 1) return;
                if (e.type === 'click' && e.button !== 0) return;
                lastGestureInteractive = isInteractiveTarget(e.target, true);
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
                if (pageWhitelisted || !data || typeof data !== 'object') return data;
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
                if (pageWhitelisted || !YT_PLAYER_API.test(this.url || '')) return p;
                return p.then((d) => { try { return prunePlayerAds(d); } catch (e) { return d; } });
            };

            let inAd = false, saved = null, adTimer = null, watchedPlayer = null, playerObs = null;
            // защита от зацикливания: реакция на смену класса плеера не чаще раза в 100 мс, перемотка рекламы не чаще раза в секунду
            let lastAdStep = 0, adSoon = 0, adSeekAt = 0;
            const isAdClass = (pl) => pl.classList.contains('ad-showing') || pl.classList.contains('ad-interrupting');
            function adStepSoon() {
                if (adSoon) return;
                adSoon = setTimeout(() => { adSoon = 0; adStep(); }, 150);
            }
            function adStep() {
                if (pageWhitelisted) return;
                const stepT = performance.now();
                if (stepT - lastAdStep < 100) return;
                lastAdStep = stepT;
                const player = document.getElementById('movie_player');
                if (!player) return;
                const video = player.querySelector('video');

                const skip = player.querySelector('.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button, .ytp-ad-skip-button-container button');
                if (skip) skip.click();

                if (isAdClass(player) && video) {
                    if (!inAd) { inAd = true; saved = { rate: video.playbackRate, muted: video.muted }; }
                    if (!video.muted) video.muted = true;
                    if (video.playbackRate !== 16) video.playbackRate = 16;
                    if (isFinite(video.duration) && video.duration > 0 && stepT - adSeekAt > 1000 && video.currentTime < video.duration - 0.3) {
                        adSeekAt = stepT;
                        video.currentTime = video.duration;
                    }
                    if (!adTimer) adTimer = setInterval(adStep, 250);
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
                if (pageWhitelisted) return;
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
                if (document.hidden || pageWhitelisted) return;
                const player = document.getElementById('movie_player');
                if (player && player !== watchedPlayer) {
                    watchedPlayer = player;
                    if (playerObs) playerObs.disconnect();
                    playerObs = new MutationObserver(() => { if (inAd || isAdClass(player)) adStepSoon(); });
                    playerObs.observe(player, { attributes: true, attributeFilter: ['class'] });
                    adStep();
                }
                removeWall();
            }
            setInterval(ytWatch, 1000);
            window.addEventListener('yt-navigate-finish', ytWatch);
        }
        if (isYT && !pageWhitelisted) setupYouTube();

        // ---- Блокировка рекламы на уровне DOM (script / iframe / img), кроме YouTube, чистых приложений и белого списка ----
        if (!isYT && !isCleanApp && !pageWhitelisted) {
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
            const TEXT_TAGS = new Set(['P', 'SPAN', 'CODE', 'PRE', 'B', 'I', 'EM', 'STRONG', 'SMALL', 'SUB', 'SUP', 'LI', 'TD', 'TH', 'TR', 'TBODY', 'THEAD']);
            let pendingNodes = new Set(), scanScheduled = false;
            const flushScan = () => {
                scanScheduled = false;
                const batch = pendingNodes;
                pendingNodes = new Set();
                if (pageWhitelisted) return;
                for (const n of batch) {
                    if (!n.isConnected) continue;
                    if (AD_TAGS.has(n.tagName)) {
                        if (isBlocked(n.src)) n.remove();
                    } else if (n.firstElementChild) {
                        const els = n.querySelectorAll('script[src],iframe[src],img[src],embed[src]');
                        for (let i = 0; i < els.length; i++) {
                            if (isBlocked(els[i].src)) els[i].remove();
                        }
                    }
                }
            };
            if (!(NATIVE && window === window.top)) {
                new MutationObserver((muts) => {
                    if (pageWhitelisted) return;
                    for (let i = 0; i < muts.length; i++) {
                        const nodes = muts[i].addedNodes;
                        for (let j = 0; j < nodes.length; j++) {
                            const n = nodes[j];
                            if (!n || n.nodeType !== 1 || n.isContentEditable) continue;
                            if (AD_TAGS.has(n.tagName)) {
                                if (isBlocked(n.src)) { n.remove(); continue; }
                            } else if (n.firstElementChild && !(TEXT_TAGS.has(n.tagName) && n.childElementCount < 3)) {
                                pendingNodes.add(n);
                            }
                        }
                    }
                    if (pendingNodes.size && !scanScheduled) {
                        scanScheduled = true;
                        requestAnimationFrame(flushScan);
                    }
                }).observe(document, { childList: true, subtree: true });
            }
        }

        // Стили подсветки найденных слов на странице (CSS Custom Highlight API - без изменения DOM)
        const highlightRules = `
            ::highlight(ul-find-all) { background-color: rgba(253, 214, 99, 0.42); color: inherit; }
            ::highlight(ul-find-cur) { background-color: #ff9800; color: #000; }
        `;
        // @view-transition оставляем обычным <style>, как было раньше (плавные переходы между страницами)
        const vtCss = `
            @view-transition { navigation: auto; }
            ::view-transition-old(root), ::view-transition-new(root) { animation-duration: 0.14s; }
        `;
        const cleanCss = highlightRules + vtCss;
        const css = cleanCss + `
            .b-adv, .b-banner, #branding, .branding, .ad-container,
            [id*="yandex_rtb"], [class*="yandex-rtb"],
            [id^="ad_"], [class^="ad_"],
            div[data-ad], div[data-banner],
            ins.adsbygoogle, [id*="adfox"], [class*="adfox"], [id^="div-gpt-ad"], [id^="google_ads"],
            iframe[src*="adfox"], iframe[src*="adriver"],
            a[href*="1xbet"], a[href*="fonbet"], a[href*="casino"] {
                display: none !important; opacity: 0 !important; pointer-events: none !important; width: 0 !important; height: 0 !important;
            }
        `;
        const ytCss = cleanCss + `
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

        let cssOk = null;
        function injectCss() {
            const id = isYT ? 'ultralight-yt-css' : 'ultralight-css';
            const targetCss = (isCleanApp || pageWhitelisted) ? cleanCss : (isYT ? ytCss : css);
            if (cssOk === null) cssOk = ulSetCss(id, targetCss);
            else if (cssOk) ulSetCss(id, targetCss);
            if (!document.head) { requestAnimationFrame(injectCss); return; }
            if (!document.getElementById(id)) {
                const style = document.createElement('style'); style.id = id;
                style.textContent = cssOk ? vtCss : targetCss;
                document.head.appendChild(style);
            }
        }
        injectCss();

        let COSM = null;
        let cosmCss = null, cosmEl = null;
        function buildCosmCss() {
            if (!COSM) return '';
            const sels = Array.isArray(COSM.g) ? COSM.g.slice() : [];
            let h = curHost;
            while (h) {
                if (COSM.h && COSM.h[h]) {
                    for (let i = 0; i < COSM.h[h].length; i++) sels.push(COSM.h[h][i]);
                }
                const i = h.indexOf('.');
                if (i < 0) break;
                h = h.slice(i + 1);
            }
            return sels.length ? sels.join('{display:none!important}\n') + '{display:none!important}' : '';
        }
        function applyCosmetic() {
            if (isYT || isCleanApp || pageWhitelisted || window !== window.top) return;
            if (cosmCss === null) {
                if (!COSM) {
                    if (typeof window.__ulCosmRaw !== 'function') return;
                    let raw = '{"g":[],"h":{}}';
                    try { raw = window.__ulCosmRaw(); } catch (e) {}
                    try { delete window.__ulCosmRaw; } catch (e) {}
                    try { COSM = rApply(rJsonParse, JSON, [raw]); } catch (e) { COSM = { g: [], h: {} }; }
                }
                cosmCss = buildCosmCss();
            }
            if (!cosmCss) return;
            if (ulSetCss('cosm', cosmCss)) return;
            const root = document.head || document.documentElement;
            if (!root) { requestAnimationFrame(applyCosmetic); return; }
            if (!cosmEl || !cosmEl.isConnected) {
                cosmEl = document.createElement('style');
                cosmEl.textContent = cosmCss;
                root.appendChild(cosmEl);
            } else if (cosmEl.textContent !== cosmCss) {
                cosmEl.textContent = cosmCss;
            }
        }
        applyCosmetic();

        // Горячее обновление фильтров без перезапуска браузера (после нажатия "Обновить фильтры" в Настройках)
        lockGlobal('__ulUpdateFilters', function(newAdHostsCsv, newCosmJson) {
            if (typeof newAdHostsCsv === 'string') {
                AD_SET = new Set(newAdHostsCsv.split(',').filter(Boolean));
            }
            if (typeof newCosmJson === 'string' && newCosmJson) {
                try { COSM = rApply(rJsonParse, JSON, [newCosmJson]); } catch (e) {}
                cosmCss = null;
                applyCosmetic();
            }
        });

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

        const PIN_H = 75;
        const pinAdjusted = new Set();
        let fixedAdjustTimer = 0, pinWatchOn = false;

        function pinRestore(el) {
            try {
                const o = el.dataset.ulOrigTop;
                if (o !== undefined) {
                    delete el.dataset.ulOrigTop;
                    if (o) el.style.top = o; else el.style.removeProperty('top');
                }
                const m = el.dataset.ulOrigMax;
                if (m !== undefined) {
                    delete el.dataset.ulOrigMax;
                    if (m) el.style.maxHeight = m; else el.style.removeProperty('max-height');
                }
            } catch (e) {}
        }

        // Сдвигает элемент, если он стоит под панелью. Возвращает true, если сдвинули.
        function pinShift(el, cs, bodyStatic) {
            if (el.dataset.ulOrigTop !== undefined || pinAdjusted.size >= 60) return false;
            const pos = cs.position;
            if (pos !== 'fixed' && pos !== 'sticky' && !(pos === 'absolute' && bodyStatic && el.offsetParent === document.body)) return false;
            const topPx = parseFloat(cs.top);
            if (!(topPx >= 0 && topPx < PIN_H)) return false;
            const r = el.getBoundingClientRect();
            // sticky: сдвигаем порог прилипания (на место в потоке это не влияет); остальные - только то, что реально под панелью
            if (pos !== 'sticky' && (r.top >= PIN_H || r.bottom <= 0)) return false;
            const nt = topPx + PIN_H;
            el.dataset.ulOrigTop = el.style.top || '';
            el.style.setProperty('top', nt + 'px', 'important');
            // элемент на всю высоту окна: после сдвига не даём ему вылезти за нижний край
            if (pos !== 'sticky' && r.height >= window.innerHeight - 2) {
                el.dataset.ulOrigMax = el.style.maxHeight || '';
                el.style.setProperty('max-height', 'calc(100% - ' + nt + 'px)', 'important');
            }
            pinAdjusted.add(el);
            return true;
        }

        function pinIsOurs(el) {
            return el.id === 'light-browser-ui' || el.id === 'light-trigger-zone' || el.tagName === 'YTD-MASTHEAD' || el.id === 'masthead-container';
        }

        function adjustFixedHeaders() {
            if (window !== window.top || !document.body) return;
            const active = pinTopBar && !document.fullscreenElement;
            // вернуть всё, что сдвигали раньше (панель открепили, полноэкранный режим) и выброшенное из DOM
            pinAdjusted.forEach((el) => {
                if (!active || !el.isConnected) { pinRestore(el); pinAdjusted.delete(el); }
            });
            if (!active) return;
            let bodyStatic = true;
            try { bodyStatic = window.getComputedStyle(document.body).position === 'static'; } catch (e) {}
            const seen = new Set();
            const check = (start) => {
                for (let el = start; el && el !== document.body && el !== document.documentElement; el = el.parentElement) {
                    if (seen.has(el)) return;
                    seen.add(el);
                    if (pinIsOurs(el)) return;
                    let cs = null;
                    try { cs = window.getComputedStyle(el); } catch (e) { return; }
                    const pos = cs.position;
                    if (pos === 'fixed' || pos === 'sticky' || pos === 'absolute') {
                        // ближайший прибитый к окну предок - это и есть то, что нужно сдвинуть
                        if (pinShift(el, cs, bodyStatic)) return;
                        if (pos !== 'absolute') return;
                    }
                }
            };
            // 1) шапки по типичным селекторам (прилипающие меню, которые ещё не дошли до верха)
            const named = document.querySelectorAll('header, nav, [role="banner"], [role="navigation"], #header, #navbar, .header, .navbar, .sticky-top, .fixed-top');
            for (let i = 0; i < named.length; i++) check(named[i]);
            // 2) облегчённая сетка точек под панелью
            const w = window.innerWidth, xs = [0.15, 0.5, 0.85], ys = [10, 55];
            for (let yi = 0; yi < ys.length; yi++) {
                for (let xi = 0; xi < xs.length; xi++) {
                    let stack = [];
                    try { stack = document.elementsFromPoint(Math.round(w * xs[xi]), ys[yi]); } catch (e) {}
                    for (let i = 0; i < stack.length; i++) check(stack[i]);
                }
            }
        }

        function pinSchedule(ms) {
            if (fixedAdjustTimer) return;
            fixedAdjustTimer = setTimeout(() => {
                fixedAdjustTimer = 0;
                runIdle(adjustFixedHeaders);
            }, ms);
        }
        function pinOnScroll() {
            if (window.scrollY < 180) pinSchedule(250);
        }
        function pinOnResize() { pinSchedule(200); }

        function setPinWatch(on) {
            if (on === pinWatchOn) return;
            pinWatchOn = on;
            if (on) {
                window.addEventListener('scroll', pinOnScroll, { passive: true, capture: false });
                window.addEventListener('resize', pinOnResize, { passive: true });
            } else {
                window.removeEventListener('scroll', pinOnScroll, { capture: false });
                window.removeEventListener('resize', pinOnResize);
            }
        }

        function applyPinBarLayout() {
            // только главное окно: во фреймах (встроенные плееры, виджеты) отступ под панель не нужен
            if (window !== window.top) return;
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
            setPinWatch(active);
            clearTimeout(fixedAdjustTimer);
            fixedAdjustTimer = 0;
            pinSchedule(60);
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
                if (pinTopBar) pinSchedule(200);
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

        // ---- Картинка-в-картинке (Picture-in-Picture) в один клик ----
        function findBestPiPVideo() {
            if (document.pictureInPictureElement) return document.pictureInPictureElement;
            const vids = document.querySelectorAll('video');
            let best = null, bestScore = -1;
            for (let i = 0; i < vids.length; i++) {
                const v = vids[i];
                if (!v.isConnected || v.readyState === 0) continue;
                if (v.closest && v.closest('#movie_player.ad-showing, #movie_player.ad-interrupting')) continue;
                const r = v.getBoundingClientRect();
                const area = Math.max(0, r.width) * Math.max(0, r.height);
                let score = 0;
                if (!v.paused && !v.ended) score += 1000000;
                if (v.currentTime > 0) score += 100000;
                if (isFinite(v.duration) && v.duration > 30) score += 50000;
                score += Math.min(area, 50000);
                if (score > bestScore) { bestScore = score; best = v; }
            }
            return best;
        }

        async function togglePiP() {
            try {
                if (document.pictureInPictureElement) {
                    await document.exitPictureInPicture();
                    if (ui && ui.syncPiP) ui.syncPiP();
                    return;
                }
                const v = findBestPiPVideo();
                if (!v) {
                    if (window.__ulDlNotice) window.__ulDlNotice('info', 'На странице нет активного видео', '');
                    return;
                }
                if (v.disablePictureInPicture) v.disablePictureInPicture = false;
                if (v.hasAttribute('disablepictureinpicture')) v.removeAttribute('disablepictureinpicture');
                await v.requestPictureInPicture();
                if (ui && ui.syncPiP) ui.syncPiP();
            } catch (e) {
                if (window.__ulDlNotice) window.__ulDlNotice('err', 'Не удалось включить Картинку в картинке', '');
            }
        }
        lockGlobal('__ulTogglePiP', togglePiP);

        document.addEventListener('enterpictureinpicture', () => { if (ui && ui.syncPiP) ui.syncPiP(); }, true);
        document.addEventListener('leavepictureinpicture', () => { if (ui && ui.syncPiP) ui.syncPiP(); }, true);
        document.addEventListener('loadedmetadata', (e) => {
            if (e.target && e.target.tagName === 'VIDEO' && ui && ui.syncPiP) ui.syncPiP();
        }, true);
        document.addEventListener('play', (e) => {
            if (e.target && e.target.tagName === 'VIDEO' && ui && ui.syncPiP) ui.syncPiP();
        }, true);

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

        let tabsState = { list: [], active: 0, pinned: 0, hosts: [] };
        let audioPlaying = new Set(), audioMuted = new Set();
        let tabEls = [], tabSoundBtns = [];
        let drawnCount = -1, tabClosing = false;
        let dragState = false, pendingDraw = false, dragJustEnded = false;
        let fileDragJustEnded = false;
        let ctxMenu = null; // открытое меню правой кнопки по вкладке (см. showTabCtx)
        const dropInd = document.createElement('div');
        dropInd.style.cssText = 'position:absolute;top:2px;width:2px;height:28px;background:#8ab4f8;border-radius:1px;display:none;pointer-events:none;';

        // ---- Нативное перетаскивание скачанного файла мышкой (в мессенджеры, почту, Проводник) ----
        function bindFileDrag(el, getPath, onStartDrag) {
            el.addEventListener('pointerdown', (e0) => {
                if (!e0.isTrusted || e0.button !== 0) return;
                const fp = getPath();
                if (!fp) return;
                const sx = e0.clientX, sy = e0.clientY;
                let started = false;
                const onMove = (e) => {
                    if (started) return;
                    if (Math.hypot(e.clientX - sx, e.clientY - sy) < 5) return;
                    started = true;
                    cleanup();
                    fileDragJustEnded = true;
                    setTimeout(() => { fileDragJustEnded = false; }, 350);
                    if (onStartDrag) onStartDrag();
                    ipcPost('DL_DRAG|' + fp);
                };
                const cleanup = () => {
                    window.removeEventListener('pointermove', onMove, true);
                    window.removeEventListener('pointerup', cleanup, true);
                    window.removeEventListener('pointercancel', cleanup, true);
                };
                window.addEventListener('pointermove', onMove, true);
                window.addEventListener('pointerup', cleanup, true);
                window.addEventListener('pointercancel', cleanup, true);
            });
        }

        // ---- Иконки сайтов (favicon) для обычных и закреплённых вкладок ----
        // Каждая обычная вкладка - новый документ, поэтому кэш иконок живёт в Rust (favicons.txt) и приходит сюда
        // через __ulFavs (ключ - хост без "www.", значение - base64 PNG, "!N" - иконку получить не удалось).
        // Иконка рисуется в <canvas> из ImageBitmap: CSP сайта (img-src) её не блокирует, а перерисовка вкладок не мигает.
        const favMemCache = {};
        const favData = Object.create(null);
        const favBmp = Object.create(null);
        const favDone = Object.create(null);   // host -> готовый ImageBitmap (или false, если не декодировался)
        let favWait = false, favsReady = false, favGate = false;   // favsReady: кэш иконок от Rust для этого документа уже пришёл
        const favTried = new Set();
        let favSig = 0, favOwnDone = false, favCapTimer = null, favBusy = false;
        const favKey = (h) => String(h || '').toLowerCase().replace(/^www\./, '');
        const favHostOk = (k) => /^[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(k) && !/^[\d.]+$/.test(k);
        const favMisses = (k) => { const v = favData[k]; return (typeof v === 'string' && v.charCodeAt(0) === 33) ? (parseInt(v.slice(1), 10) || 1) : 0; };

        function pageDeclaredFavicon() {
            try {
                const links = document.querySelectorAll('link[rel*="icon"]');
                for (let i = 0; i < links.length; i++) {
                    const href = links[i].href;
                    if (href && /^https?:\/\//i.test(href)) return href;
                }
            } catch (e) {}
            return '';
        }

        function favBitmap(key) {
            if (favBmp[key]) return favBmp[key];
            const b64 = favData[key];
            if (!b64 || b64.charCodeAt(0) === 33) return null;
            let p;
            try {
                const bin = atob(b64);
                const arr = new Uint8Array(bin.length);
                for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
                p = createImageBitmap(new Blob([arr], { type: 'image/png' })).catch(() => null);
            } catch (e) { p = Promise.resolve(null); }
            favBmp[key] = p;
            p.then((b) => { favDone[key] = b || false; });
            return p;
        }

        function paintFav(cv, bmp) {
            try {
                const c = cv.getContext('2d');
                c.clearRect(0, 0, 32, 32);
                c.drawImage(bmp, 0, 0, 32, 32);
            } catch (e) {}
        }

        function createTabFavicon(host, label, isActive, isPinned) {
            const wrap = document.createElement('span');
            wrap.style.cssText = `width: 16px; height: 16px; flex: none; display: inline-flex; align-items: center; justify-content: center; position: relative; margin-right: ${isPinned ? 0 : 7}px; pointer-events: none; overflow: hidden; border-radius: 3px;`;

            const cleanH = favKey(host);
            const letter = Array.from(String(label || cleanH || '•').trim())[0] || '•';
            // Цветная подложка с буквой на случай, когда иконки нет (или ещё не загрузилась)
            let hash = 0;
            for (let i = 0; i < cleanH.length; i++) hash = ((hash << 5) - hash + cleanH.charCodeAt(i)) | 0;
            const hue = Math.abs(hash) % 360;
            const fallback = document.createElement('span');
            fallback.textContent = letter.toUpperCase();
            fallback.style.cssText = `width: 16px; height: 16px; border-radius: 3px; background: hsl(${hue}, 45%, 32%); color: #f1f3f4; font-size: 10px; font-weight: 700; line-height: 16px; text-align: center; display: flex; align-items: center; justify-content: center;`;
            wrap.appendChild(fallback);

            if (!host || !host.includes('.')) return wrap;

            // 1. Иконка есть в кэше Rust: рисуем её сразу, буквенную заглушку не показываем (иначе она мигает)
            const bp = favDone[cleanH] === false ? null : favBitmap(cleanH);
            if (bp) {
                fallback.style.display = 'none';
                const cv = document.createElement('canvas');
                cv.width = 32; cv.height = 32;
                cv.style.cssText = 'width: 16px; height: 16px; display: block; border-radius: 2px;';
                wrap.appendChild(cv);
                if (favDone[cleanH]) {
                    paintFav(cv, favDone[cleanH]);   // уже декодировано: рисуем синхронно, без кадра пустоты
                } else {
                    bp.then((bmp) => {
                        if (bmp) paintFav(cv, bmp);
                        else { cv.remove(); fallback.style.display = 'flex'; }
                    });
                }
                return wrap;
            }

            // 2. Кэша ещё нет: показываем картинку как раньше, а в фоне иконка будет сохранена (scheduleFavCapture)
            const candidates = [];
            if (isActive && host.toLowerCase() === curHost) {
                const decl = pageDeclaredFavicon();
                if (decl) {
                    favMemCache[host] = decl;
                    candidates.push(decl);
                }
                candidates.push(window.location.origin + '/favicon.ico');
            } else if (favMemCache[host]) {
                candidates.push(favMemCache[host]);
            }
            candidates.push('https://www.google.com/s2/favicons?sz=32&domain=' + encodeURIComponent(host));
            candidates.push('https://icons.duckduckgo.com/ip3/' + encodeURIComponent(host) + '.ico');
            candidates.push('https://' + host + '/favicon.ico');

            const img = document.createElement('img');
            img.style.cssText = 'width: 16px; height: 16px; object-fit: contain; display: none; border-radius: 2px;';
            img.referrerPolicy = 'no-referrer';
            let step = 0;
            const tryNext = () => {
                if (step >= candidates.length) {
                    img.remove();
                    fallback.style.display = 'flex';
                    return;
                }
                img.src = candidates[step++];
            };
            img.onload = () => {
                if (img.naturalWidth > 0) {
                    favMemCache[host] = img.src;
                    fallback.style.display = 'none';
                    img.style.display = 'block';
                } else {
                    tryNext();
                }
            };
            img.onerror = tryNext;
            wrap.appendChild(img);
            tryNext();
            return wrap;
        }

        // Загрузка картинки в ImageBitmap: fetch (для того же origin работает при любом CSP), затем <img crossorigin>
        async function loadFavBitmap(url) {
            try {
                const r = await fetch(url, { mode: 'cors', credentials: 'omit' });
                if (r.ok) {
                    const b = await r.blob();
                    if (b.size > 0 && b.size < 300000) return await createImageBitmap(b);
                }
            } catch (e) {}
            return await new Promise((resolve) => {
                try {
                    const im = new Image();
                    im.crossOrigin = 'anonymous';
                    im.referrerPolicy = 'no-referrer';
                    im.onload = () => { createImageBitmap(im).then(resolve, () => resolve(null)); };
                    im.onerror = () => resolve(null);
                    im.src = url;
                } catch (e) { resolve(null); }
            });
        }

        function favBitmapToB64(bmp) {
            for (const size of [32, 16]) {
                try {
                    const cv = document.createElement('canvas');
                    cv.width = size; cv.height = size;
                    cv.getContext('2d').drawImage(bmp, 0, 0, size, size);
                    const d = cv.toDataURL('image/png');
                    const i = d.indexOf(',');
                    if (i > 0 && d.length - i - 1 <= 7000) return d.slice(i + 1);
                } catch (e) { return ''; }
            }
            return '';
        }

        // own = true: иконка самой открытой страницы (самый надёжный источник); иначе - сторонние сервисы иконок
        async function captureFav(key, own) {
            const urls = [];
            if (own) {
                const decl = pageDeclaredFavicon();
                if (decl) urls.push(decl);
                urls.push(window.location.origin + '/favicon.ico');
            } else {
                urls.push('https://www.google.com/s2/favicons?sz=64&domain=' + encodeURIComponent(key));
                urls.push('https://icons.duckduckgo.com/ip3/' + encodeURIComponent(key) + '.ico');
                urls.push('https://' + key + '/favicon.ico');
            }
            for (let i = 0; i < urls.length; i++) {
                const bmp = await loadFavBitmap(urls[i]);
                if (!bmp || bmp.width < 8) continue;
                const b64 = favBitmapToB64(bmp);
                if (!b64) continue;
                if (favData[key] !== b64) {
                    favData[key] = b64;
                    favBmp[key] = Promise.resolve(bmp);
                    favDone[key] = bmp;
                    ipcPost('FAV|' + key + '|' + b64);
                    favSig++;
                    drawTabs();
                }
                return true;
            }
            if (!own) { favData[key] = '!' + (favMisses(key) + 1); ipcPost('FAV|' + key + '|!'); }
            return false;
        }

        async function runFavCapture() {
            if (favBusy) return;
            favBusy = true;
            try {
                const own = favKey(curHost);
                if (!favOwnDone && document.readyState === 'complete' && favHostOk(own)) {
                    favOwnDone = true;
                    await captureFav(own, true);
                }
                let n = 0;
                const hs = tabsState.hosts || [];
                for (let i = 0; i < hs.length && n < 3; i++) {
                    const k = favKey(hs[i]);
                    if (!favHostOk(k) || k === own || favTried.has(k)) continue;
                    const v = favData[k];
                    if (v && v.charCodeAt(0) !== 33) continue;   // настоящая иконка уже есть
                    if (favMisses(k) >= 3) continue;
                    favTried.add(k); n++;
                    await captureFav(k, false);
                }
            } catch (e) {}
            favBusy = false;
        }
        function scheduleFavCapture() {
            if (favCapTimer) return;
            favCapTimer = setTimeout(() => { favCapTimer = null; runIdle(runFavCapture); }, 700);
        }
        window.addEventListener('load', scheduleFavCapture);

        lockGlobal('__ulFavs', function(json) {
            let o = null;
            try { o = rApply(rJsonParse, JSON, [String(json)]); } catch (e) { return; }
            if (!o || typeof o !== 'object') return;
            let changed = !favsReady;
            favsReady = true;
            Object.keys(o).forEach((k) => {
                const v = o[k];
                if (typeof v !== 'string' || favData[k] === v) return;
                favData[k] = v;
                delete favBmp[k];
                delete favDone[k];
                favBitmap(k);   // декодируем заранее, до отрисовки вкладок
                changed = true;
            });
            if (changed) { favSig++; drawTabs(); }
            scheduleFavCapture();
        });

        function updateAudioIcons() {
            for (let i = 0; i < tabSoundBtns.length; i++) {
                const btn = tabSoundBtns[i];
                if (!btn) continue;
                const isPinned = i < (tabsState.pinned | 0);
                const playing = audioPlaying.has(i);
                const muted = audioMuted.has(i);
                if (playing || muted) {
                    btn.style.display = 'flex';
                    btn.textContent = muted ? '\uD83D\uDD07' : '\uD83D\uDD0A';
                    btn.style.color = muted ? '#f28b82' : '#8ab4f8';
                    btn.title = muted ? 'Включить звук вкладки (Ctrl+M)' : 'Выключить звук вкладки (Ctrl+M)';
                    if (isPinned && tabEls[i]) {
                        tabEls[i].style.minWidth = '54px';
                        tabEls[i].style.maxWidth = '54px';
                        tabEls[i].style.padding = '0 5px';
                    }
                } else {
                    btn.style.display = 'none';
                    if (isPinned && tabEls[i]) {
                        tabEls[i].style.minWidth = '36px';
                        tabEls[i].style.maxWidth = '36px';
                        tabEls[i].style.padding = '0';
                    }
                }
            }
        }

        lockGlobal('__ulAudio', function(playingArr, mutedArr) {
            audioPlaying = new Set(Array.isArray(playingArr) ? playingArr : []);
            audioMuted = new Set(Array.isArray(mutedArr) ? mutedArr : []);
            updateAudioIcons();
            if (ui && ui.syncPiP) ui.syncPiP();
        });

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
            // Страницу больше не затемняем: затемнение + пустой документ + возврат яркости давали «рваное» переключение.
            // Обратная связь - подсветка вкладки и полоска прогресса.
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
        let lastTabsSig = '';
        function drawTabs(noWait) {
            if (!ui) return;
            if (dragState) { pendingDraw = true; return; }
            // __ulRT приходит на каждое SYNC_URL/TITLE; если вкладки, активная, закреплённые и иконки не менялись,
            // не пересоздаём всю панель (раньше иконки при этом перезагружались и мигали)
            const sig = JSON.stringify([tabsState.list, tabsState.hosts, tabsState.active, tabsState.pinned, favSig]);
            if (sig === lastTabsSig && tabEls.length) { updateAudioIcons(); return; }
            // Иконки из кэша декодируются асинхронно. Ждём их (не дольше 150 мс), чтобы панель появилась сразу с иконками,
            // а не сначала пустой/с буквами
            // Rust отвечает на LOAD_URL вкладками РАНЬШЕ, чем присылает кэш иконок: без этого ожидания панель рисовалась
            // с буквами и сразу перерисовывалась. Ждём кэш иконок (не дольше 400 мс).
            if (noWait !== true && !favsReady) {
                if (!favGate) {
                    favGate = true;
                    setTimeout(() => { favGate = false; favsReady = true; drawTabs(); }, 400);
                }
                return;
            }
            if (noWait !== true) {
                const pend = [];
                (tabsState.hosts || []).forEach((h) => {
                    const k = favKey(h);
                    const v = favData[k];
                    if (v && v.charCodeAt(0) !== 33 && favDone[k] === undefined) { const p = favBitmap(k); if (p) pend.push(p); }
                });
                if (pend.length) {
                    if (!favWait) {
                        favWait = true;
                        Promise.race([Promise.all(pend), new Promise((r) => setTimeout(r, 150))]).then(() => { favWait = false; drawTabs(true); });
                    }
                    return;
                }
            }
            lastTabsSig = sig;
            const row = ui.tabsRow;
            row.style.visibility = tabsState.list.length ? '' : 'hidden';   // пока Rust не прислал вкладки - не показываем пустую панель
            row.replaceChildren();
            tabEls = [];
            tabSoundBtns = [];
            tabClosing = false;
            const prevCount = drawnCount;
            const grew = prevCount > 0 && tabsState.list.length > prevCount;
            drawnCount = tabsState.list.length;

            tabsState.list.forEach((label, i) => {
                let tabEl = document.createElement('div');
                let isActive = (i === tabsState.active);
                const isPinned = i < (tabsState.pinned | 0);
                const host = (tabsState.hosts && tabsState.hosts[i]) ? tabsState.hosts[i] : (isActive ? curHost : '');

                tabEl.style.cssText = `
                    min-width: ${isPinned ? 36 : 120}px; max-width: ${isPinned ? 36 : 210}px; height: 28px; margin-top: 2px; margin-right: ${(isPinned && i === (tabsState.pinned | 0) - 1) ? 8 : 2}px;
                    background: ${isActive ? '#303134' : '#171717'}; color: ${isActive ? '#fff' : '#9aa0a6'};
                    border-radius: 8px 8px 0 0; display: flex; align-items: center; justify-content: ${isPinned ? 'center' : 'space-between'};
                    padding: 0 ${isPinned ? 0 : 9}px; font-size: 13px; cursor: pointer; user-select: none;
                    touch-action: none; flex: none;
                `;
                tabEl.className = 'ul-tab';
                tabEl.title = label || host || 'Новая вкладка';
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

                // Иконка сайта (в закреплённых вкладках отображается только она и индикатор звука, без текста)
                const iconEl = createTabFavicon(host, label, isActive, isPinned);
                tabEl.appendChild(iconEl);

                if (!isPinned) {
                    let titleSpan = document.createElement('span');
                    titleSpan.textContent = label || 'Новая вкладка';
                    titleSpan.style.cssText = `overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-grow: 1; margin-right: 6px; pointer-events: none;`;
                    tabEl.appendChild(titleSpan);
                }

                // Кнопка индикации и отключения звука вкладки (появляется, когда играет звук или вкладка заглушена)
                let soundBtn = document.createElement('span');
                soundBtn.style.cssText = `font-size: 12px; width: 18px; height: 18px; display: none; align-items: center; justify-content: center; border-radius: 50%; cursor: pointer; flex: none; margin-left: ${isPinned ? 3 : 0}px; margin-right: ${isPinned ? 0 : 2}px;`;
                soundBtn.className = 'ul-tx';
                soundBtn.onclick = (e) => {
                    e.stopPropagation();
                    if (e.isTrusted) ipcPost('MUTE_TAB|' + i);
                };
                tabEl.appendChild(soundBtn);

                let closeBtn = null;
                if (!isPinned) {
                    closeBtn = document.createElement('span');
                    closeBtn.textContent = '×';
                    closeBtn.style.cssText = `font-size: 16px; width: 18px; height: 18px; display: flex; align-items: center; justify-content: center; border-radius: 50%; font-weight: bold; cursor: pointer; flex: none;`;
                    closeBtn.className = 'ul-tx';
                    closeBtn.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) closeTab(i, tabEl); };
                    tabEl.appendChild(closeBtn);
                }

                tabEl.addEventListener('pointerdown', (e) => {
                    if (e.isTrusted && e.button === 0 && e.target !== closeBtn && e.target !== soundBtn) startTabDrag(e, i, tabEl);
                });
                tabEl.addEventListener('mousedown', (e) => { if (e.button === 1) e.preventDefault(); });
                tabEl.addEventListener('auxclick', (e) => {
                    if (e.isTrusted && e.button === 1) { e.preventDefault(); closeTab(i, tabEl); }
                });

                tabEl.addEventListener('contextmenu', (e) => {
                    e.preventDefault(); e.stopPropagation();
                    if (e.isTrusted) showTabCtx(i, e);
                });

                row.appendChild(tabEl);
                tabEls.push(tabEl);
                tabSoundBtns.push(soundBtn);
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
            updateAudioIcons();
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

        lockGlobal('__ulRT', function(tabsString, activeIndex, hostsString) {
            if (typeof tabsString !== 'string') return;
            // формат: "<закреплено>\u001e<ярлык>|||<ярлык>..." (старый формат без числа тоже понимаем)
            let pinned = 0, body = tabsString;
            const sp = tabsString.indexOf('\u001e');
            if (sp >= 0) { pinned = parseInt(tabsString.slice(0, sp), 10) || 0; body = tabsString.slice(sp + 1); }
            const list = body.split('|||');
            const hosts = typeof hostsString === 'string' ? hostsString.split('|||') : [];
            tabsState = { list: list, active: activeIndex | 0, pinned: Math.max(0, Math.min(pinned, list.length)), hosts: hosts };
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
                .ul-b.pip-on{color:#8ab4f8}
                .ul-b.pip-on:hover{color:#aecbfa}
                .ul-b.shield-on{color:#81c995}
                .ul-b.shield-on:hover{color:#a8dab5}
                .ul-b.shield-off{color:#f28b82;opacity:.85}
                .ul-b.shield-off:hover{color:#f6aea9;opacity:1}
                .ul-b.ul-dis{opacity:.35;cursor:default}
                .ul-b.ul-dis:hover,.ul-b.ul-dis:active{background:transparent;color:#9aa0a6;transform:none}
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
            if (INCOGNITO) {
                const incogBadge = document.createElement('div');
                incogBadge.textContent = '\u25D1 Инкогнито';
                incogBadge.title = 'Режим инкогнито: история и сессия не сохраняются, данные удаляются при закрытии окна';
                incogBadge.style.cssText = 'flex:none;display:flex;align-items:center;height:22px;margin:0 8px;padding:0 10px;border-radius:11px;background:#3b2a5a;color:#d7c5ff;font:12px/1 "Segoe UI",system-ui,sans-serif;white-space:nowrap;';
                incogBadge.addEventListener('mousedown', (e) => {
                    if (e.isTrusted && e.button === 0) { e.preventDefault(); ipcPost('WIN_DRAG'); }
                });
                topBar.appendChild(incogBadge);
            }
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
                if (ui && ui.syncPiP) ui.syncPiP();
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
            // История «Назад / Вперёд» ведётся в Rust отдельно для каждой вкладки (TabHist): общий WebView один
            // на все обычные вкладки, поэтому history.back() смешало бы переходы разных вкладок.
            let navCanBack = false, navCanFwd = false;
            const navBackBtn = navButton('\u2190', 'Назад (Alt+←)', (e) => { if (e && e.isTrusted) navStep(-1); });
            const navFwdBtn = navButton('\u2192', 'Вперёд (Alt+→)', (e) => { if (e && e.isTrusted) navStep(1); });
            function paintNavBtn(b, on) {
                b.className = on ? 'ul-b' : 'ul-b ul-dis';
                b.setAttribute('aria-disabled', on ? 'false' : 'true');
            }
            paintNavBtn(navBackBtn, false);
            paintNavBtn(navFwdBtn, false);
            function navStep(dir) {
                if (dir < 0 ? !navCanBack : !navCanFwd) return;
                startProgress();
                ipcPost(dir < 0 ? 'NAV_BACK' : 'NAV_FWD');
                // если Rust шаг не выполнил (страница не выгрузилась), не оставляем полосу загрузки висеть
                setTimeout(finishProgress, 2500);
            }
            // Rust присылает доступность кнопок вместе с состоянием вкладок (tabs_rt -> nav_state_js)
            lockGlobal('__ulNavState', function(canBack, canFwd) {
                navCanBack = !!canBack; navCanFwd = !!canFwd;
                paintNavBtn(navBackBtn, navCanBack);
                paintNavBtn(navFwdBtn, navCanFwd);
            });
            urlRow.appendChild(navBackBtn);
            urlRow.appendChild(navFwdBtn);
            urlRow.appendChild(navButton('\u21BB', 'Обновить (F5)', () => { startProgress(); location.reload(); }));

            // ---- Кнопка «Щит» (белый список блокировщика рекламы в 1 клик) ----
            const shieldBtn = navButton('\u26E8', 'Блокировщик активен (нажмите, чтобы отключить на этом сайте)', (e) => {
                if (e && e.isTrusted) {
                    startProgress();
                    ipcPost('SHIELD_TOGGLE');
                }
            });
            shieldBtn.style.marginLeft = '6px';
            shieldBtn.style.fontSize = '15px';
            function updateShieldUI(whitelisted) {
                pageWhitelisted = !!whitelisted;
                if (pageWhitelisted) {
                    shieldBtn.className = 'ul-b shield-off';
                    shieldBtn.title = 'Защита отключена на этом сайте (в белом списке). Нажмите, чтобы включить.';
                } else {
                    shieldBtn.className = 'ul-b shield-on';
                    shieldBtn.title = 'Блокировщик рекламы активен для этого сайта. Нажмите, чтобы добавить в белый список.';
                }
            }
            updateShieldUI(pageWhitelisted);

            lockGlobal('__ulShield', function(whitelisted, csv) {
                if (typeof csv === 'string') {
                    WL_SET = new Set(csv.split(',').filter(Boolean));
                }
                updateShieldUI(!!whitelisted || hostInSet(WL_SET, curHost));
            });

            // ---- Кнопка «Картинка в картинке» (PiP) в адресной строке ----
            const pipBtn = navButton('\u29C9', 'Картинка в картинке (Alt+P)', (e) => {
                if (e && e.isTrusted) togglePiP();
            });
            pipBtn.style.marginLeft = '2px';
            pipBtn.style.fontSize = '15px';
            pipBtn.style.display = 'none';
            function syncPiP() {
                const inPip = !!document.pictureInPictureElement;
                const hasVid = inPip || !!findBestPiPVideo() || audioPlaying.has(tabsState.active | 0);
                pipBtn.style.display = hasVid ? 'flex' : 'none';
                pipBtn.className = inPip ? 'ul-b pip-on' : 'ul-b';
                pipBtn.title = inPip ? 'Выйти из режима «Картинка в картинке» (Alt+P)' : 'Картинка в картинке поверх всех окон (Alt+P)';
            }

            const popBtn = navButton('\u2197', 'Вывести текущую вкладку в отдельное окно (для видеозвонков)', (e) => { if (e && e.isTrusted) ipcPost('DETACH_TAB'); });
            popBtn.style.marginLeft = '0';
            if (INCOGNITO) popBtn.style.display = 'none';
            const US = '\u001f';
            const starBtn = navButton('\u2606', 'Добавить страницу в закладки (Ctrl+D)', (e) => { if (e && e.isTrusted) onStar(); });
            starBtn.style.marginLeft = '2px';
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
            // ---- Ручки изменения размера безрамочного окна: края и углы -> WIN_RESIZE|<направление> ----
            // Rust вызывает window.drag_resize_window(...) и сам игнорирует запрос, если окно развёрнуто / в полноэкранном режиме.
            const GRIP = 5, GRIP_TOP = 4, CORNER = 8;
            const gripHost = document.createElement('div');
            gripHost.id = 'light-resize-grips';
            gripHost.style.cssText = 'position:fixed;top:0;left:0;width:0;height:0;z-index:2147483647;';
            let gripRoot = gripHost;
            try { gripRoot = rApply(rAttachShadow, gripHost, [{ mode: 'closed' }]); } catch (e) { gripRoot = gripHost; }
            const GRIP_DEFS = [
                ['n',  `top:0;left:${CORNER}px;right:${CORNER}px;height:${GRIP_TOP}px;`, 'ns-resize'],
                ['s',  `bottom:0;left:${CORNER}px;right:${CORNER}px;height:${GRIP}px;`, 'ns-resize'],
                ['w',  `left:0;top:${CORNER}px;bottom:${CORNER}px;width:${GRIP}px;`, 'ew-resize'],
                ['e',  `right:0;top:${CORNER}px;bottom:${CORNER}px;width:${GRIP}px;`, 'ew-resize'],
                ['nw', `top:0;left:0;width:${CORNER}px;height:${CORNER}px;`, 'nwse-resize'],
                ['ne', `top:0;right:0;width:${CORNER}px;height:${CORNER}px;`, 'nesw-resize'],
                ['sw', `bottom:0;left:0;width:${CORNER}px;height:${CORNER}px;`, 'nesw-resize'],
                ['se', `bottom:0;right:0;width:${CORNER}px;height:${CORNER}px;`, 'nwse-resize'],
            ];
            GRIP_DEFS.forEach(([dir, pos, cur]) => {
                const g = document.createElement('div');
                g.style.cssText = `position:fixed;${pos}cursor:${cur};background:transparent;user-select:none;`;
                g.addEventListener('mousedown', (e) => {
                    if (!e.isTrusted || e.button !== 0) return;
                    e.preventDefault(); e.stopPropagation();
                    ipcPost('WIN_RESIZE|' + dir);
                });
                gripRoot.appendChild(g);
            });
            let winMaximized = false;
            function updateGrips() {
                gripHost.style.display = (winMaximized || document.fullscreenElement) ? 'none' : 'block';
            }
            document.addEventListener('fullscreenchange', updateGrips);
            lockGlobal('__ulMaxState', function(isMax) {
                maxBtn.textContent = isMax ? '\u2750' : '\u25A1';
                winMaximized = !!isMax;
                updateGrips();
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
            let viewKind = 'menu', panelHideT = 0, activeDlDirInput = null, activeFilterUpdBtn = null, settingsTab = 'general';
            let loadTitle = '', lastDlItems = [];
            let listSearch = null, listQuery = '', listShown = []; // поле поиска в панелях «История», «Закладки», «Загрузки»
            const dlRows = new Map(); // key загрузки -> элементы живой строки в списке «Загрузки»
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
            // Приведение текста для поиска: регистр не важен, «ё» = «е», а кириллические буквы-двойники латинских
            // (а, е, о, р, с, у, х, к, м, н, т, в) считаются теми же латинскими. В именах файлов и заголовках они
            // часто перемешаны, и глазом такую подмену не отличить («Elden» с кириллической «е» не находился).
            const FOLD_MAP = { '\u0430': 'a', '\u0432': 'b', '\u0435': 'e', '\u043a': 'k', '\u043c': 'm', '\u043d': 'h', '\u043e': 'o', '\u0440': 'p', '\u0441': 'c', '\u0442': 't', '\u0443': 'y', '\u0445': 'x' };
            function searchFold(s) {
                let r = String(s).toLowerCase();
                try { r = r.normalize('NFKD').replace(/[\u0300-\u036f]/g, ''); } catch (e) {}
                return r.replace(/[\u0430\u0432\u0435\u043a\u043c\u043d\u043e\u0440\u0441\u0442\u0443\u0445]/g, (c) => FOLD_MAP[c]);
            }

            function closePanel() {
                if (!panelOpen) return;
                panelOpen = false; viewKind = 'menu'; activeDlDirInput = null; activeFilterUpdBtn = null;
                panel.className = 'ul-out';
                clearTimeout(panelHideT);
                panelHideT = setTimeout(() => { panel.style.display = 'none'; panel.textContent = ''; panel.className = ''; }, 130);
                setTimeout(() => { if (!container.matches(':hover')) hide(); }, 0);
                dlRows.clear(); dlVisSync();
            }
            function panelHeader(title, extra, searchEl) {
                const h = mk('div', 'display:flex;align-items:center;padding:8px 8px;border-bottom:1px solid #3c4043;position:sticky;top:0;background:#202124;z-index:1;');
                const back = mk('div', 'width:28px;height:28px;display:flex;align-items:center;justify-content:center;border-radius:50%;cursor:pointer;color:#9aa0a6;font-size:16px;', '\u2190');
                back.title = 'Назад в меню';
                back.className = 'ul-x';
                back.onclick = (e) => { if (e.isTrusted) showMenu(); };
                h.appendChild(back);
                h.appendChild(mk('div', 'flex:1;font-weight:bold;margin-left:8px;', title));
                if (extra) h.appendChild(extra);
                if (searchEl) { h.style.flexWrap = 'wrap'; h.appendChild(searchEl); }
                return h;
            }
            function showMenu() {
                viewKind = 'menu'; activeDlDirInput = null; activeFilterUpdBtn = null;
                dlRows.clear(); dlVisSync();
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

                const incogRow = mk('div', 'padding:10px 16px;cursor:pointer;user-select:none;box-sizing:border-box;', (INCOGNITO ? 'Ещё одно окно инкогнито' : 'Новое окно инкогнито') + ' (Ctrl+Shift+N)');
                incogRow.className = 'ul-row';
                incogRow.onclick = (e) => {
                    if (!e.isTrusted) return;
                    closePanel();
                    ipcPost('INCOGNITO');
                };
                box.appendChild(incogRow);

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
                viewKind = 'loading'; loadTitle = title;
                dlRows.clear(); dlVisSync();
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
                const row = mk('div', 'display:flex;align-items:center;padding:6px 8px 6px 16px;cursor:pointer;box-sizing:border-box;user-select:none;');
                row.className = 'ul-row';
                row.title = isDl ? 'Показать в папке (или зажмите и перетащите файл): ' + String(it.u) : String(it.u);
                const col = mk('div', 'flex:1;min-width:0;');
                col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;', String(it.t || it.u)));
                const sub = isDl
                    ? timeLabel(it.d) + '  \u00B7  ' + String(it.u)
                    : hostOf(it.u) + (kind === 'history' ? '  \u00B7  ' + timeLabel(it.d) : '');
                col.appendChild(mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:11px;', sub));
                row.appendChild(col);

                let showBtn = null;
                if (isDl) {
                    showBtn = mk('div', 'width:26px;height:26px;flex:none;margin-left:6px;border-radius:50%;display:flex;align-items:center;justify-content:center;color:#8ab4f8;font-size:14px;', '\uD83D\uDCC2');
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
                    bindFileDrag(
                        row,
                        () => it.u,
                        () => { closePanel(); hide(); }
                    );
                    row.onclick = (e) => {
                        if (e.isTrusted && !fileDragJustEnded) ipcPost('DL_SHOW|' + it.u);
                    };
                    row.addEventListener('contextmenu', (e) => {
                        e.preventDefault(); e.stopPropagation();
                        if (e.isTrusted) dlItemCtx(it, e);
                    });
                } else {
                    row.onclick = (e) => { if (e.isTrusted) openUrl(String(it.u), e.ctrlKey || e.metaKey); };
                    row.onauxclick = (e) => { if (e.isTrusted && e.button === 1) { e.preventDefault(); openUrl(String(it.u), true); } };
                }
                row.appendChild(del);
                return row;
            }
            // Живая строка скачивания (прогресс, скорость, пауза/отмена) в начале списка «Загрузки»
            function liveRow(d) {
                const row = mk('div', 'display:flex;align-items:center;padding:8px 8px 8px 16px;box-sizing:border-box;cursor:default;');
                row.className = 'ul-row';
                const col = mk('div', 'flex:1;min-width:0;');
                const nm = mk('div', 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;');
                const track = mk('div', 'height:4px;border-radius:2px;background:#3c4043;margin:6px 0 4px;overflow:hidden;');
                const fill = mk('div', 'height:100%;width:0;border-radius:2px;background:#8ab4f8;transition:width .25s linear,background-color .2s ease;');
                track.appendChild(fill);
                const st = mk('div', 'color:#9aa0a6;font-size:11px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-variant-numeric:tabular-nums;');
                col.appendChild(nm); col.appendChild(track); col.appendChild(st);
                const pb = dlMkBtn('', 'Пауза'), cb = dlMkBtn('\u2715', 'Отменить загрузку');
                pb.style.marginLeft = '6px'; cb.style.marginLeft = '2px';
                pb.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) dlAct(d, d.paused ? 'RESUME' : 'PAUSE'); };
                cb.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) dlAct(d, 'CANCEL'); };
                row.appendChild(col); row.appendChild(pb); row.appendChild(cb);
                row.addEventListener('contextmenu', (e) => {
                    e.preventDefault(); e.stopPropagation();
                    if (e.isTrusted) dlCtx(d, e);
                });
                const parts = { el: row, nm, st, fill, pb, cb };
                dlRows.set(d.key, parts);
                dlPaint(d, parts);
                return row;
            }
            // Обновление живых строк «на месте» без перестройки списка
            dlHub.listOpen = () => panelOpen && (viewKind === 'downloads' || (viewKind === 'loading' && loadTitle === 'Загрузки'));
            dlHub.live = (d) => {
                if (!panelOpen || viewKind !== 'downloads') return;
                const r = dlRows.get(d.key);
                if (r) dlPaint(d, r);
                else if (d.state === 'run') showList('downloads', lastDlItems);
            };
            function showList(kind, items) {
                const fresh = viewKind !== kind;
                const keep = panel.scrollTop;
                // список перестраивается после удаления записи или новой загрузки: текст поиска и фокус сохраняем
                const hadFocus = !!(listSearch && listSearch.isConnected && listSearch.getRootNode().activeElement === listSearch);
                if (fresh) listQuery = '';
                viewKind = kind; activeDlDirInput = null; activeFilterUpdBtn = null;
                dlRows.clear();
                if (kind === 'downloads') lastDlItems = items;
                dlVisSync();
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
                let search = null;
                if (items.length) {
                    search = document.createElement('input');
                    search.type = 'text'; search.placeholder = 'Поиск\u2026'; search.maxLength = 100;
                    search.value = listQuery;
                    search.className = 'ul-input';
                    search.style.cssText = 'flex:0 0 100%;box-sizing:border-box;height:28px;border:none;border-radius:14px;padding:0 12px;margin-top:8px;color:#fff;outline:none;font-size:12px;';
                    ['keydown', 'keyup', 'keypress'].forEach((t) => search.addEventListener(t, (ev) => ev.stopPropagation()));
                    // Enter открывает первый найденный пункт (Ctrl+Enter - в фоновой вкладке)
                    search.addEventListener('keydown', (e) => {
                        if (e.key !== 'Enter' || !e.isTrusted || e.isComposing) return;
                        const first = listShown[0];
                        if (!first) return;
                        e.preventDefault();
                        if (kind === 'downloads') ipcPost('DL_SHOW|' + first.u);
                        else openUrl(String(first.u), e.ctrlKey || e.metaKey);
                    });
                }
                listSearch = search;
                panel.appendChild(panelHeader(headerTitle, extra, search));
                panel.appendChild(body);
                let liveN = 0;
                if (kind === 'downloads') {
                    dlMap.forEach((d) => {
                        if (d.state !== 'run') return;
                        if (!liveN) body.appendChild(mk('div', 'padding:10px 16px 2px;color:#9aa0a6;font-size:11px;text-transform:uppercase;letter-spacing:.04em;', 'Загружается'));
                        liveN++;
                        body.appendChild(liveRow(d));
                    });
                }
                if (!items.length && !liveN) {
                    const emptyText = kind === 'history'
                        ? 'История пуста'
                        : (kind === 'bookmarks' ? 'Закладок пока нет. Нажмите \u2606 в адресной строке.' : 'Список загрузок пуст');
                    const empty = mk('div', 'padding:18px 16px;color:#9aa0a6;', emptyText);
                    empty.className = 'ul-view';
                    body.appendChild(empty);
                    return;
                }
                // Записи (кроме «Загружается») лежат в отдельном блоке: поиск перерисовывает только его
                const listBox = mk('div');
                const foldCache = new Map(); // нормализованный текст записи считаем один раз на перестройку списка
                body.appendChild(listBox);
                function fillList() {
                    listBox.textContent = '';
                    const tokens = searchFold(listQuery.trim()).split(/\s+/).filter(Boolean);
                    listShown = items.filter((it) => {
                        if (!it || typeof it.u !== 'string') return false;
                        if (!tokens.length) return true;
                        let hay = foldCache.get(it);
                        if (hay === undefined) { hay = searchFold(String(it.t || '') + ' ' + it.u); foldCache.set(it, hay); }
                        return tokens.every((w) => hay.indexOf(w) !== -1);
                    });
                    if (!listShown.length) {
                        if (tokens.length) listBox.appendChild(mk('div', 'padding:18px 16px;color:#9aa0a6;', 'Ничего не найдено'));
                        return;
                    }
                    let lastDay = '';
                    listShown.forEach((it) => {
                        if (kind === 'history' || kind === 'downloads') {
                            const day = dayLabel(it.d);
                            if (day !== lastDay) {
                                lastDay = day;
                                listBox.appendChild(mk('div', 'padding:10px 16px 2px;color:#9aa0a6;font-size:11px;text-transform:uppercase;letter-spacing:.04em;', day));
                            }
                        }
                        listBox.appendChild(listRow(kind, it));
                    });
                }
                fillList();
                if (search) {
                    search.addEventListener('input', () => { listQuery = search.value; fillList(); });
                    // при открытии панели курсор сразу в поиске, чтобы можно было печатать
                    if (fresh || hadFocus) {
                        try { search.focus({ preventScroll: true }); const n = search.value.length; search.setSelectionRange(n, n); } catch (e) {}
                    }
                }
                if (!fresh) panel.scrollTop = keep;
            }

            function showSettings(cfg) {
                viewKind = 'settings';
                dlRows.clear(); dlVisSync();
                panel.textContent = '';
                const openDirBtn = mk('div', 'padding:4px 10px;border-radius:12px;cursor:pointer;color:#8ab4f8;font-size:12px;user-select:none;', '\uD83D\uDCC2 Папка конфигов');
                openDirBtn.className = 'ul-btn';
                openDirBtn.title = 'Открыть папку Falco (settings.txt, whitelist.txt, filters, scripts)';
                openDirBtn.onclick = (e) => { if (e.isTrusted) ipcPost('OPEN_APP_DIR'); };

                // шапка и вкладки категорий закреплены сверху панели, кнопка сохранения - снизу
                const topWrap = mk('div', 'position:sticky;top:0;z-index:2;background:#202124;');
                topWrap.appendChild(panelHeader('Настройки', openDirBtn));
                panel.appendChild(topWrap);
                const body = mk('div', 'padding:12px 16px;min-height:280px;box-sizing:border-box;');
                body.className = 'ul-view';
                const secCss = 'display:flex;flex-direction:column;gap:12px;';
                const SEC = { general: mk('div', secCss), tabs: mk('div', secCss), downloads: mk('div', secCss), privacy: mk('div', secCss) };
                const TABS = [['general', 'Общие'], ['tabs', 'Вкладки'], ['downloads', 'Загрузки'], ['privacy', 'Приватность']];

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
                SEC.general.appendChild(gSearch);

                // Стартовая страница
                const gStart = mk('div');
                gStart.appendChild(lbl('Стартовая страница (новая вкладка)'));
                const inpStart = document.createElement('input');
                inpStart.type = 'text'; inpStart.value = cfg.start_url || 'https://duckduckgo.com';
                inpStart.className = 'ul-input'; inpStart.style.cssText = inpCss;
                stopKeys(inpStart);
                gStart.appendChild(inpStart);
                SEC.general.appendChild(gStart);

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
                SEC.downloads.appendChild(gDl);

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
                SEC.privacy.appendChild(gMedia);

                // Онлайн-обновление списков фильтров (EasyList + EasyPrivacy)
                const gFilters = mk('div');
                gFilters.appendChild(lbl('Списки блокировки рекламы (EasyList и EasyPrivacy)'));
                const updFiltersBtn = mk('div', 'height:30px;padding:0 12px;border-radius:6px;display:flex;align-items:center;justify-content:center;cursor:pointer;background:#303134;color:#8ab4f8;font-size:12px;font-weight:500;user-select:none;', '\u21BB Обновить списки фильтров');
                updFiltersBtn.className = 'ul-btn';
                updFiltersBtn.title = 'Скачать свежие списки EasyList и EasyPrivacy в папку filters';
                updFiltersBtn.onclick = (e) => {
                    if (e.isTrusted) ipcPost('FILTERS_UPDATE');
                };
                activeFilterUpdBtn = updFiltersBtn;
                gFilters.appendChild(updFiltersBtn);
                SEC.privacy.appendChild(gFilters);

                // Переключатели (чекбоксы)
                const mkCheck = (text, checked, sec) => {
                    const wrap = mk('label', 'display:flex;align-items:center;gap:8px;cursor:pointer;user-select:none;font-size:12px;color:#e8eaed;');
                    const cb = document.createElement('input');
                    cb.type = 'checkbox'; cb.checked = !!checked;
                    cb.style.cssText = 'accent-color:#8ab4f8;width:15px;height:15px;cursor:pointer;margin:0;flex:none;';
                    wrap.appendChild(cb);
                    wrap.appendChild(mk('span', '', text));
                    sec.appendChild(wrap);
                    return cb;
                };

                const cbPinBar = mkCheck('Закрепить верхнюю панель вкладок и поиска (не скрывать)', cfg.pin_top_bar, SEC.general);
                const cbAskDl = mkCheck('Спрашивать, куда сохранить каждый скачиваемый файл', cfg.ask_download, SEC.downloads);
                const cbSession = mkCheck('Восстанавливать вкладки при запуске', cfg.restore_session, SEC.tabs);
                const cbLive = mkCheck('Закреплённые вкладки работают в фоне (музыка и видео не прерываются)', cfg.live_pinned, SEC.tabs);
                const cbHist = mkCheck('Сохранять историю посещений', cfg.history_enabled, SEC.privacy);
                const cbWebrtc = mkCheck('Скрывать локальный IP в WebRTC (после перезапуска)', cfg.webrtc_hide_ip, SEC.privacy);
                const cbSmart = mkCheck('Защита Microsoft SmartScreen (после перезапуска)', cfg.smartscreen, SEC.privacy);
                const cbH264 = mkCheck('Принудительно H.264 на YouTube для слабых ПК (после перезапуска)', cfg.yt_force_h264, SEC.general);

                const saveBtn = mk('div', 'padding:8px 14px;border-radius:6px;cursor:pointer;background:#8ab4f8;color:#202124;font-weight:bold;text-align:center;user-select:none;', 'Сохранить настройки');
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
                        'live_pinned=' + (cbLive.checked ? '1' : '0'),
                        'ask_download=' + (cbAskDl.checked ? '1' : '0'),
                    ];
                    ipcPost('SETTINGS_SAVE|' + parts.join(US));
                    closePanel();
                };
                // вкладки категорий (все поля остаются в DOM, поэтому сохраняются значения из всех категорий)
                const tabBar = mk('div', 'display:flex;gap:2px;padding:0 8px;border-bottom:1px solid #3c4043;');
                const tabBtns = {};
                const selectTab = (key) => {
                    settingsTab = key;
                    TABS.forEach((tb) => {
                        const on = tb[0] === key;
                        SEC[tb[0]].style.display = on ? 'flex' : 'none';
                        tabBtns[tb[0]].style.color = on ? '#8ab4f8' : '#9aa0a6';
                        tabBtns[tb[0]].style.borderBottomColor = on ? '#8ab4f8' : 'transparent';
                    });
                    panel.scrollTop = 0;
                };
                TABS.forEach((tb) => {
                    const b = mk('div', 'flex:1;text-align:center;padding:9px 4px;font-size:12px;cursor:pointer;user-select:none;white-space:nowrap;border-bottom:2px solid transparent;', tb[1]);
                    b.className = 'ul-row';
                    b.onclick = (e) => { if (e.isTrusted) selectTab(tb[0]); };
                    tabBtns[tb[0]] = b;
                    tabBar.appendChild(b);
                    body.appendChild(SEC[tb[0]]);
                });
                topWrap.appendChild(tabBar);
                selectTab(SEC[settingsTab] ? settingsTab : 'general');

                // кнопка сохранения всегда видна внизу панели; в конце списка стоит на своём месте и ничего не перекрывает
                const footer = mk('div', 'position:sticky;bottom:0;z-index:2;background:#202124;border-top:1px solid #3c4043;padding:10px 16px;');
                footer.appendChild(saveBtn);
                panel.appendChild(body);
                panel.appendChild(footer);
            }

            lockGlobal('__ulFilterUpdateState', function(state, text) {
                if (!activeFilterUpdBtn || !activeFilterUpdBtn.isConnected) return;
                if (state === 'busy') {
                    activeFilterUpdBtn.textContent = '\u23F3 ' + (text || 'Скачивание…');
                    activeFilterUpdBtn.style.color = '#9aa0a6';
                    activeFilterUpdBtn.style.pointerEvents = 'none';
                } else if (state === 'ok') {
                    activeFilterUpdBtn.textContent = '\u2714 ' + (text || 'Фильтры обновлены');
                    activeFilterUpdBtn.style.color = '#81c995';
                    activeFilterUpdBtn.style.pointerEvents = 'auto';
                } else {
                    activeFilterUpdBtn.textContent = '\u2716 ' + (text || 'Ошибка обновления');
                    activeFilterUpdBtn.style.color = '#f28b82';
                    activeFilterUpdBtn.style.pointerEvents = 'auto';
                }
            });

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
                if (ctxMenu && ctxMenu.contains(e.target)) return;
                if (popOpen) closeBmPop(true);
                if (panelOpen) closePanel();
            }, true);
            root.appendChild(panel); root.appendChild(bmPop); root.appendChild(findBar); root.appendChild(sug);

            urlRow.appendChild(input); urlRow.appendChild(shieldBtn); urlRow.appendChild(pipBtn); urlRow.appendChild(starBtn); urlRow.appendChild(popBtn); urlRow.appendChild(menuBtn); urlRow.appendChild(progBar);
            root.appendChild(topBar); root.appendChild(urlRow);

            ui = {
                container, trigger, grips: gripHost, navStep, tabsRow, input, show, hide, inputFocused, startProgress, finishProgress,
                syncPiP, openFind, findStep: runFind, isFindOpen: () => findOpen,
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
            syncPiP();
            applyPinBarLayout();
            ipcPost('GET_TABS|F');
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
        let dlChip = null, dlTimer = 0, dlChipPath = '';
        lockGlobal('__ulDlNotice', function(state, name, fullPath) {
            if (!document.documentElement) return;
            clearTimeout(dlTimer);
            dlChipPath = (state === 'ok' && fullPath) ? String(fullPath) : '';
            if (!dlChip || !dlChip.isConnected) {
                dlChip = document.createElement('div');
                dlChip.style.cssText = 'position:fixed;right:14px;bottom:14px;max-width:270px;height:32px;padding:0 12px;background:rgba(32,33,36,.95);color:#e8eaed;border:1px solid #3c4043;border-radius:16px;z-index:2147483647;box-shadow:0 4px 12px rgba(0,0,0,.45);display:flex;align-items:center;gap:7px;font:12px/1 "Segoe UI",system-ui,sans-serif;user-select:none;opacity:0;transform:translateY(6px);transition:opacity .18s ease,transform .18s cubic-bezier(.2,.8,.3,1),border-color .15s ease;zoom:1 !important;';
                bindFileDrag(
                    dlChip,
                    () => dlChipPath,
                    () => {
                        clearTimeout(dlTimer);
                        if (dlChip) {
                            dlChip.style.opacity = '0';
                            dlChip.style.transform = 'translateY(6px)';
                            setTimeout(() => { if (dlChip) { dlChip.remove(); dlChip = null; } }, 180);
                        }
                    }
                );
                document.documentElement.appendChild(dlChip);
            }
            dlNoticePlace();
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
                dlChip.title = 'Выполняется: ' + name;
                dlChip.onclick = null;
                dlChip.appendChild(icon);
                dlChip.appendChild(txt);
            } else if (state === 'ok') {
                icon.textContent = '\u2714';
                icon.style.color = '#81c995';
                dlChip.style.cursor = 'pointer';
                dlChip.title = fullPath ? 'Нажмите, чтобы показать в папке (или зажмите и перетащите файл)' : '';
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
                        if (!e.isTrusted || fileDragJustEnded) return;
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
                dlChip.title = 'Уведомление Falco';
                dlChip.onclick = (e) => { if (e.isTrusted && String(name || '').startsWith('Масштаб:')) stepPageZoom(0); };
                dlChip.appendChild(icon);
                dlChip.appendChild(txt);
            } else {
                icon.textContent = '\u2716';
                icon.style.color = '#f28b82';
                dlChip.style.cursor = 'default';
                dlChip.title = 'Ошибка';
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

        // ---- Живые загрузки: прогресс, скорость, пауза/отмена, правая кнопка (Rust вызывает __ulDlLive) ----
        const dlMap = new Map(); // key -> { id, key, name, path, url, rec, tot, spd, paused, state: run|ok|cancel|err, chip, hidden, tm }
        const dlHub = { listOpen: () => false, live: null }; // заполняется при построении панели
        let dlCard = null;
        const dlSet = (el, t) => { if (el.textContent !== t) el.textContent = t; };
        const dlP2 = (x) => (x < 10 ? '0' : '') + x;
        function fmtB(n) {
            n = +n || 0;
            if (n < 1024) return n + ' Б';
            if (n < 1048576) return Math.round(n / 1024) + ' КБ';
            const big = n >= 1073741824, v = n / (big ? 1073741824 : 1048576);
            return (v < 10 ? v.toFixed(2) : (v < 100 ? v.toFixed(1) : String(Math.round(v)))).replace('.', ',') + (big ? ' ГБ' : ' МБ');
        }
        function fmtEta(sec) {
            const s = Math.max(1, Math.ceil(sec)), h = (s / 3600) | 0, m = ((s % 3600) / 60) | 0;
            return h > 0 ? h + ':' + dlP2(m) + ':' + dlP2(s % 60) : m + ':' + dlP2(s % 60);
        }
        function dlStat(d) {
            if (d.state === 'ok') return 'Готово \u00B7 ' + fmtB(d.tot > 0 ? d.tot : d.rec);
            if (d.state === 'err') return 'Ошибка загрузки' + (d.rec > 0 ? ' \u00B7 ' + fmtB(d.rec) : '');
            if (d.state === 'cancel') return 'Отменено';
            const det = d.tot > 0;
            let s = det ? Math.floor(Math.min(100, d.rec * 100 / d.tot)) + '% \u00B7 ' + fmtB(d.rec) + ' из ' + fmtB(d.tot) : fmtB(d.rec);
            if (d.paused) return s + ' \u00B7 пауза';
            if (d.spd > 0) {
                s += ' \u00B7 ' + fmtB(d.spd) + '/с';
                if (det && d.tot > d.rec) s += ' \u00B7 ост. ' + fmtEta((d.tot - d.rec) / d.spd);
            }
            return s;
        }
        function dlMkBtn(sym, title) {
            const b = document.createElement('div');
            b.textContent = sym; b.title = title;
            b.style.cssText = 'width:22px;height:22px;flex:none;border-radius:50%;display:flex;align-items:center;justify-content:center;cursor:pointer;color:#9aa0a6;font-size:11px;transition:background-color .12s ease,color .12s ease;';
            b.onmouseenter = () => { b.style.background = '#3c4043'; b.style.color = '#fff'; };
            b.onmouseleave = () => { b.style.background = ''; b.style.color = '#9aa0a6'; };
            return b;
        }
        // Отрисовка одной строки (в плашке или в списке): p = { el, nm, st, fill, pb, cb, ic?, chip? }
        function dlPaint(d, p) {
            const run = d.state === 'run', det = d.tot > 0;
            const col = d.state === 'ok' ? '#81c995' : ((d.state === 'err' || d.state === 'cancel') ? '#f28b82' : (d.paused ? '#9aa0a6' : '#8ab4f8'));
            dlSet(p.nm, d.name || 'файл');
            dlSet(p.st, dlStat(d));
            if (p.ic) {
                dlSet(p.ic, d.state === 'ok' ? '\u2714' : (run ? (d.paused ? '\u275A\u275A' : '\u2B07') : '\u2716'));
                p.ic.style.color = col;
            }
            p.fill.style.background = col;
            if (run && !det && !d.paused) {
                if (!p.fill._a) {
                    p.fill.style.width = '35%';
                    try { p.fill._a = p.fill.animate([{ transform: 'translateX(-100%)' }, { transform: 'translateX(290%)' }], { duration: 1100, iterations: Infinity, easing: 'ease-in-out' }); } catch (e) {}
                }
            } else {
                if (p.fill._a) { try { p.fill._a.cancel(); } catch (e) {} p.fill._a = null; }
                p.fill.style.width = (run ? (det ? Math.min(100, d.rec * 100 / d.tot) : 0) : 100) + '%';
            }
            const ctl = run && d.id > 0;
            p.pb.style.display = ctl ? 'flex' : 'none';
            p.cb.style.display = ctl ? 'flex' : 'none';
            dlSet(p.pb, d.paused ? '\u25B6\uFE0E' : '\u275A\u275A');
            p.pb.title = d.paused ? 'Продолжить' : 'Пауза';
            if (p.chip) {
                const click = run || (d.state === 'ok' && d.path);
                p.el.style.cursor = click ? 'pointer' : 'default';
                p.el.title = run ? 'Открыть список загрузок' : (d.state === 'ok' ? 'Показать в папке (или зажмите и перетащите файл)' : '');
            }
        }
        function dlAct(d, cmd) { if (d.id > 0) ipcPost('DL_' + cmd + '|' + d.id); }
        function dlCopy(text, msg) {
            const t = String(text == null ? '' : text).slice(0, 4096);
            const done = () => { if (window.__ulDlNotice) window.__ulDlNotice('ok', msg, ''); };
            const fallback = () => {
                try {
                    const ta = document.createElement('textarea');
                    ta.value = t; ta.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0;';
                    document.documentElement.appendChild(ta); ta.select();
                    const ok = document.execCommand('copy'); ta.remove();
                    if (ok) done();
                } catch (e) {}
            };
            try {
                if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(t).then(done, fallback);
                else fallback();
            } catch (e) { fallback(); }
        }
        const dlSrc = (u) => (/^https?:\/\//i.test(u || '') ? u : '');
        // Меню правой кнопки: идущая / только что завершённая загрузка
        function dlCtx(d, ev) {
            const items = [], src = dlSrc(d.url);
            if (d.state === 'run') {
                if (d.id > 0) {
                    items.push([d.paused ? 'Продолжить' : 'Поставить на паузу', () => dlAct(d, d.paused ? 'RESUME' : 'PAUSE')]);
                    items.push(['Отменить загрузку', () => dlAct(d, 'CANCEL'), { danger: true }]);
                    items.push(null);
                }
                if (src) items.push(['Копировать ссылку на источник', () => dlCopy(src, 'Ссылка скопирована')]);
                items.push(['Открыть папку загрузок', () => ipcPost('DL_FOLDER')]);
                if (!dlHub.listOpen()) items.push(['Показать список загрузок', () => { if (ui) ui.openDownloads(); }]);
            } else if (d.state === 'ok' && d.path) {
                items.push(['Открыть файл', () => ipcPost('DL_OPEN|' + d.path)]);
                items.push(['Показать в папке', () => ipcPost('DL_SHOW|' + d.path)]);
                items.push(null);
                items.push(['Копировать путь к файлу', () => dlCopy(d.path, 'Путь скопирован')]);
                if (src) items.push(['Копировать ссылку на источник', () => dlCopy(src, 'Ссылка скопирована')]);
            } else if (src) {
                items.push(['Копировать ссылку на источник', () => dlCopy(src, 'Ссылка скопирована')]);
                items.push(['Скачать заново', () => ipcPost('DL_RETRY|' + src)]);
            }
            if (d.chip) {
                if (items.length) items.push(null);
                items.push(['Скрыть плашку', () => { d.hidden = true; d.chip = null; dlLayout(); }]);
            }
            if (items.length) openCtx(items, ev);
        }
        // Меню правой кнопки: запись в списке завершённых загрузок
        function dlItemCtx(it, ev) {
            const src = dlSrc(it.src);
            const items = [
                ['Открыть файл', () => ipcPost('DL_OPEN|' + it.u)],
                ['Показать в папке', () => ipcPost('DL_SHOW|' + it.u)],
                null,
                ['Копировать путь к файлу', () => dlCopy(it.u, 'Путь скопирован')],
            ];
            if (src) {
                items.push(['Копировать ссылку на источник', () => dlCopy(src, 'Ссылка скопирована')]);
                items.push(['Скачать заново', () => ipcPost('DL_RETRY|' + src)]);
            }
            items.push(null);
            items.push(['Убрать из списка', () => ipcPost('DL_DEL|' + it.u)]);
            items.push(['Удалить файл с диска\u2026', () => openCtx([
                ['Удалить файл навсегда?', null, { dim: true }],
                ['Да, удалить с диска', () => ipcPost('DL_DEL_FILE|' + it.u), { danger: true }],
                ['Отмена', () => {}],
            ], ev), { danger: true }]);
            openCtx(items, ev);
        }

        // Плашка в правом нижнем углу: до 3 загрузок, остальные сворачиваются в «и ещё N»
        function dlMkRow(d) {
            const el = document.createElement('div');
            el.style.cssText = 'padding:8px 0;';
            const top = document.createElement('div');
            top.style.cssText = 'display:flex;align-items:center;gap:6px;height:22px;';
            const ic = document.createElement('span');
            ic.style.cssText = 'flex:none;width:14px;text-align:center;font-size:12px;font-weight:bold;';
            const nm = document.createElement('span');
            nm.style.cssText = 'flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
            const pb = dlMkBtn('', 'Пауза'), cb = dlMkBtn('\u2715', 'Отменить загрузку');
            top.appendChild(ic); top.appendChild(nm); top.appendChild(pb); top.appendChild(cb);
            const track = document.createElement('div');
            track.style.cssText = 'height:4px;border-radius:2px;background:#3c4043;margin:5px 0 4px;overflow:hidden;';
            const fill = document.createElement('div');
            fill.style.cssText = 'height:100%;width:0;border-radius:2px;background:#8ab4f8;transition:width .25s linear,background-color .2s ease;';
            track.appendChild(fill);
            const st = document.createElement('div');
            st.style.cssText = 'color:#9aa0a6;font-size:11px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-variant-numeric:tabular-nums;';
            el.appendChild(top); el.appendChild(track); el.appendChild(st);
            pb.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) dlAct(d, d.paused ? 'RESUME' : 'PAUSE'); };
            cb.onclick = (e) => { e.stopPropagation(); if (e.isTrusted) dlAct(d, 'CANCEL'); };
            el.addEventListener('contextmenu', (e) => {
                e.preventDefault(); e.stopPropagation();
                if (e.isTrusted) dlCtx(d, e);
            });
            bindFileDrag(
                el,
                () => (d.state === 'ok' && d.path) ? d.path : '',
                () => { dlRetire(d); }
            );
            el.addEventListener('click', (e) => {
                if (!e.isTrusted || fileDragJustEnded) return;
                if (d.state === 'run') { if (ui) ui.openDownloads(); }
                else if (d.state === 'ok' && d.path) { ipcPost('DL_SHOW|' + d.path); dlRetire(d); }
            });
            return { el, ic, nm, st, fill, pb, cb, chip: true };
        }
        function dlNoticePlace() {
            if (!dlChip || !dlChip.isConnected) return;
            const up = dlCard && dlCard.isConnected && dlCard.style.opacity !== '0';
            dlChip.style.bottom = (up ? 14 + dlCard.offsetHeight + 8 : 14) + 'px';
        }
        function dlVisSync() {
            if (dlCard) {
                const h = dlHub.listOpen();
                dlCard.style.opacity = h ? '0' : '1';
                dlCard.style.transform = h ? 'translateY(6px)' : 'none';
                dlCard.style.pointerEvents = h ? 'none' : '';
            }
            dlNoticePlace();
        }
        function dlLayout() {
            const arr = [];
            dlMap.forEach((d) => { if (d.chip) arr.push(d); });
            if (!arr.length) {
                if (dlCard) {
                    const c = dlCard; dlCard = null;
                    c.style.opacity = '0'; c.style.transform = 'translateY(6px)';
                    setTimeout(() => c.remove(), 200);
                }
                dlNoticePlace();
                return;
            }
            if (!document.documentElement) return;
            let fresh = false;
            if (!dlCard || !dlCard.isConnected) {
                fresh = true;
                dlCard = document.createElement('div');
                dlCard.style.cssText = 'position:fixed;right:14px;bottom:14px;width:300px;max-width:calc(100vw - 28px);box-sizing:border-box;padding:2px 12px;background:rgba(32,33,36,.96);color:#e8eaed;border:1px solid #3c4043;border-radius:12px;z-index:2147483647;box-shadow:0 4px 12px rgba(0,0,0,.45);font:12px/1.2 "Segoe UI",system-ui,sans-serif;user-select:none;opacity:0;transform:translateY(6px);transition:opacity .18s ease,transform .18s cubic-bezier(.2,.8,.3,1);zoom:1 !important;';
                dlCard.addEventListener('contextmenu', (e) => { e.preventDefault(); e.stopPropagation(); });
                document.documentElement.appendChild(dlCard);
            }
            const kids = [];
            arr.slice(0, 3).forEach((d, i) => {
                d.chip.el.style.borderTop = i ? '1px solid #3c4043' : '';
                kids.push(d.chip.el);
            });
            if (arr.length > 3) {
                const more = document.createElement('div');
                more.style.cssText = 'padding:6px 0 8px;color:#9aa0a6;font-size:11px;border-top:1px solid #3c4043;';
                more.textContent = 'и ещё ' + (arr.length - 3);
                kids.push(more);
            }
            dlCard.replaceChildren.apply(dlCard, kids);
            if (fresh) requestAnimationFrame(dlVisSync); else dlVisSync();
        }
        function dlRetire(d) {
            clearTimeout(d.tm);
            d.chip = null;
            dlMap.delete(d.key);
            dlLayout();
        }
        lockGlobal('__ulDlLive', function(kind, id, rec, tot, spd, paused, name, path, url) {
            if (!document.documentElement) return;
            id = id | 0;
            const run = kind === 'start' || kind === 'prog';
            const key = id > 0 ? 'i' + id : 'p' + String(path || name);
            let d = dlMap.get(key);
            if (!d) {
                if (kind === 'cancel') return;
                d = { id, key, name: '', path: '', url: '', rec: 0, tot: 0, spd: 0, paused: false, state: 'run', chip: null, hidden: false, tm: 0 };
                dlMap.set(key, d);
            } else if (run && d.state !== 'run') return; // запоздавший прогресс после завершения
            d.rec = +rec || 0; d.tot = +tot || 0; d.spd = +spd || 0; d.paused = !!paused;
            if (name) d.name = String(name);
            if (path) d.path = String(path);
            if (url) d.url = String(url);
            clearTimeout(d.tm);
            d.state = run ? 'run' : (kind === 'ok' || kind === 'cancel' ? kind : 'err');
            if (!d.chip && !d.hidden) { d.chip = dlMkRow(d); dlLayout(); }
            if (d.chip && !document.hidden) dlPaint(d, d.chip);
            if (run) { if (dlHub.live) dlHub.live(d); return; }
            d.tm = setTimeout(() => dlRetire(d), d.state === 'ok' ? 4200 : (d.state === 'cancel' ? 700 : 5000));
        });
        // В фоновой вкладке строки не перерисовываем (экономия), при возврате догоняем одним проходом
        document.addEventListener('visibilitychange', () => {
            if (document.hidden) return;
            dlMap.forEach((d) => { if (d.chip) dlPaint(d, d.chip); });
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
            scrollTimer = setTimeout(() => {
                scrollTimer = null;
                if (stateKey() === k) runIdle(saveState);
            }, 1500);
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
            if (!e.isTrusted) return;
            const t = e.target;
            // Не вызываем saveState при вводе/вставке в contenteditable (Gemini и др.) или поля без id/name
            if (!t || (t.tagName !== 'TEXTAREA' && t.tagName !== 'INPUT') || (!t.id && !t.name)) return;
            if (ui && ui.container.contains(t)) return;
            if (inputTimer) clearTimeout(inputTimer);
            inputTimer = setTimeout(() => {
                inputTimer = null;
                runIdle(saveState);
            }, 2000);
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

        // ---- Меню правой кнопки по вкладке (Rust принимает "TAB_CMD|<команда>|<индекс>") ----
        function navCue() {
            try { sessionStorage.setItem('__ul_sw_bar', String(Date.now())); } catch (e) {}
            if (ui && ui.startProgress) ui.startProgress();
        }
        function ctxOutside(e) { if (ctxMenu && !ctxMenu.contains(e.target)) closeTabCtx(); }
        function ctxKey(e) {
            if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeTabCtx(); }
        }
        function closeTabCtx() {
            if (!ctxMenu) return;
            const m = ctxMenu;
            ctxMenu = null;
            try { m.remove(); } catch (e) {}
            window.removeEventListener('pointerdown', ctxOutside, true);
            window.removeEventListener('keydown', ctxKey, true);
            window.removeEventListener('blur', closeTabCtx);
            window.removeEventListener('resize', closeTabCtx);
            window.removeEventListener('wheel', closeTabCtx, true);
            setTimeout(() => { if (ui && !ui.container.matches(':hover')) ui.hide(); }, 0);
        }
        function showTabCtx(i, ev) {
            closeTabCtx();
            if (!document.documentElement) return;
            const n = tabsState.list.length;
            const pn = tabsState.pinned | 0;
            const isPinned = i < pn;
            const isMuted = audioMuted.has(i);
            const isCurrent = (i === (tabsState.active | 0));
            const canPiP = isCurrent ? (!!document.pictureInPictureElement || !!findBestPiPVideo()) : (isPinned || audioPlaying.has(i));
            const inPiP = isCurrent && !!document.pictureInPictureElement;
            const items = [
                [isPinned ? 'Открепить вкладку' : 'Закрепить вкладку', () => ipcPost('TAB_CMD|' + (isPinned ? 'unpin' : 'pin') + '|' + i)],
                [isMuted ? 'Включить звук вкладки' : 'Отключить звук вкладки', () => ipcPost('MUTE_TAB|' + i)],
            ];
            if (canPiP) {
                items.push([
                    inPiP ? '\u29C9 Выйти из «Картинка в картинке»' : '\u29C9 Картинка в картинке (PiP)',
                    () => { if (isCurrent) togglePiP(); else ipcPost('TAB_CMD|pip|' + i); }
                ]);
            }
            items.push(
                null,
                ['Новая вкладка справа', () => { navCue(); ipcPost('TAB_CMD|new_right|' + i); }],
                ['Дублировать вкладку', () => { navCue(); ipcPost('TAB_CMD|dup|' + i); }],
                ['Копировать адрес', () => ipcPost('TAB_CMD|copy_url|' + i)],
                null,
                ['Закрыть вкладку', () => closeTab(i, tabEls[i])]
            );
            if (n > 1) items.push(['Закрыть другие вкладки', () => ipcPost('TAB_CMD|close_others|' + i)]);
            if (i < n - 1 && n - 1 >= pn) items.push(['Закрыть вкладки справа', () => ipcPost('TAB_CMD|close_right|' + i)]);
            openCtx(items, ev);
        }

        // Универсальное меню правой кнопки: items = [[текст, действие, {danger|dim}], null (разделитель), ...]
        function openCtx(items, ev) {
            closeTabCtx();
            if (!document.documentElement) return;
            const menu = document.createElement('div');
            menu.style.cssText = 'position:fixed;left:0;top:0;visibility:hidden;z-index:2147483647;min-width:220px;padding:6px 0;background:#202124;color:#e8eaed;border:1px solid #3c4043;border-radius:8px;box-shadow:0 6px 18px rgba(0,0,0,.55);font:13px/1.2 "Segoe UI",system-ui,sans-serif;user-select:none;zoom:1 !important;';
            items.forEach((it) => {
                if (!it) {
                    const sep = document.createElement('div');
                    sep.style.cssText = 'height:1px;background:#3c4043;margin:5px 0;';
                    menu.appendChild(sep);
                    return;
                }
                const row = document.createElement('div');
                row.textContent = it[0];
                row.style.cssText = 'padding:8px 16px;cursor:pointer;white-space:nowrap;';
                const opt = it[2] || null;
                if (opt && opt.dim) {
                    row.style.cssText = 'padding:8px 16px;white-space:nowrap;color:#9aa0a6;cursor:default;';
                    menu.appendChild(row);
                    return;
                }
                if (opt && opt.danger) row.style.color = '#f28b82';
                row.onmouseenter = () => { row.style.background = '#303134'; };
                row.onmouseleave = () => { row.style.background = ''; };
                row.onclick = (e) => {
                    if (!e.isTrusted) return;
                    closeTabCtx();
                    it[1]();
                };
                menu.appendChild(row);
            });
            menu.addEventListener('contextmenu', (e) => { e.preventDefault(); e.stopPropagation(); });
            document.documentElement.appendChild(menu);
            ctxMenu = menu;
            const r = menu.getBoundingClientRect();
            const x = Math.max(4, Math.min(ev.clientX, window.innerWidth - r.width - 4));
            const y = Math.max(4, Math.min(ev.clientY, window.innerHeight - r.height - 4));
            menu.style.left = x + 'px';
            menu.style.top = y + 'px';
            menu.style.visibility = 'visible';
            window.addEventListener('pointerdown', ctxOutside, true);
            window.addEventListener('keydown', ctxKey, true);
            window.addEventListener('blur', closeTabCtx);
            window.addEventListener('resize', closeTabCtx);
            window.addEventListener('wheel', closeTabCtx, { capture: true, passive: true });
        }

        // Rust просит скопировать адрес вкладки в буфер обмена (пункт «Копировать адрес»)
        lockGlobal('__ulCopyText', function(text) {
            const t = String(text == null ? '' : text).slice(0, 4096);
            const done = () => { if (window.__ulDlNotice) window.__ulDlNotice('ok', 'Адрес скопирована', ''); };
            const fallback = () => {
                try {
                    const ta = document.createElement('textarea');
                    ta.value = t;
                    ta.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0;';
                    document.documentElement.appendChild(ta);
                    ta.select();
                    const ok = document.execCommand('copy');
                    ta.remove();
                    if (ok) done();
                } catch (e) {}
            };
            try {
                if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(t).then(done, fallback);
                else fallback();
            } catch (e) { fallback(); }
        });

        // ---- Горячие клавиши (по e.code, чтобы работали и в русской раскладке) ----
        function switchToTab(idx) {
            const n = tabsState.list.length;
            if (idx < 0 || idx >= n || idx === tabsState.active) return;
            optimisticSelectTab(idx);
            ipcPost('SWITCH_TAB|' + idx);
        }
        window.addEventListener('keydown', (e) => {
            if (!e.isTrusted) return;
            const ctrl = e.ctrlKey || e.metaKey;
            const code = e.code;
            const act = tabsState.active | 0;
            const n = tabsState.list.length;
            let handled = true;

            if (code === 'Escape') {
                handled = !!(ui && ui.closeOverlays());
            } else if (code === 'F11') {
                ipcPost('WIN_FULLSCREEN');
            } else if (code === 'F5' || (ctrl && code === 'KeyR')) {
                navCue(); location.reload();
            } else if (code === 'F6' || (ctrl && code === 'KeyL') || (e.altKey && code === 'KeyD')) {
                if (!ui) return;
                ui.show(); ui.input.focus(); ui.input.select();
            } else if (code === 'F3' && ui && ui.isFindOpen()) {
                ui.findStep(e.shiftKey);
            } else if (e.altKey && !ctrl && !e.shiftKey && code === 'KeyP') {
                togglePiP();
            } else if (e.altKey && !ctrl && (code === 'ArrowLeft' || code === 'ArrowRight')) {
                if (ui) ui.navStep(code === 'ArrowLeft' ? -1 : 1);
                else ipcPost(code === 'ArrowLeft' ? 'NAV_BACK' : 'NAV_FWD');
            } else if (ctrl && !e.altKey) {
                if (code === 'KeyT' && !e.shiftKey) { navCue(); ipcPost('new_tab'); }
                else if (code === 'KeyT' && e.shiftKey) { navCue(); ipcPost('REOPEN_TAB'); }
                else if (code === 'KeyN' && e.shiftKey) { ipcPost('INCOGNITO'); }
                else if (code === 'KeyW' && !e.shiftKey) { closeTab(act, tabEls[act]); }
                else if (code === 'Tab') {
                    if (e.shiftKey) switchToTab((act - 1 + n) % n); else ipcPost('next_tab');
                }
                else if (code === 'KeyM' && !e.shiftKey) { ipcPost('MUTE_TAB|' + act); }
                else if (code === 'KeyF' && !e.shiftKey) { if (!ui) return; ui.show(); ui.openFind(); }
                else if (code === 'KeyD' && !e.shiftKey) { if (!ui) return; ui.show(); ui.toggleBookmark(); }
                else if (code === 'KeyH' && !e.shiftKey) { if (!ui) return; ui.openHistory(); }
                else if (code === 'KeyJ' && !e.shiftKey) { if (!ui) return; ui.openDownloads(); }
                else if (code === 'Comma') { if (!ui) return; ui.openSettings(); }
                else if (code === 'KeyX' && e.shiftKey) { startPick(); }
                else if (code === 'KeyZ' && e.shiftKey) { undoRule(); }
                else if (code === 'Digit0' || code === 'Numpad0') { stepPageZoom(0); }
                else if (code === 'Equal' || code === 'NumpadAdd') { stepPageZoom(1); }
                else if (code === 'Minus' || code === 'NumpadSubtract') { stepPageZoom(-1); }
                else if (/^Digit[1-9]$/.test(code) && !e.shiftKey) {
                    const d = Number(code.charAt(5));
                    switchToTab(d === 9 ? n - 1 : d - 1);
                }
                else handled = false;
            } else {
                handled = false;
            }
            if (handled) { e.preventDefault(); e.stopPropagation(); }
        }, true);

        // Боковые кнопки мыши «Назад / Вперёд»: WebView2 по умолчанию сам листает общую историю WebView,
        // что смешало бы переходы разных вкладок, поэтому перехватываем и отдаём шаг Rust (TabHist вкладки).
        function onMouseNav(e) {
            if (!e.isTrusted || (e.button !== 3 && e.button !== 4)) return;
            e.preventDefault(); e.stopPropagation();
            if (e.type !== 'mouseup') return;
            if (ui) ui.navStep(e.button === 3 ? -1 : 1);
            else ipcPost(e.button === 3 ? 'NAV_BACK' : 'NAV_FWD');
        }
        window.addEventListener('mousedown', onMouseNav, true);
        window.addEventListener('mouseup', onMouseNav, true);
        window.addEventListener('auxclick', onMouseNav, true);

        // ---- Создание верхней панели ----
        function mountUI() {
            if (ui) return;
            const de = document.documentElement;
            if (!de) { requestAnimationFrame(mountUI); return; }
            try {
                buildUI();
                de.appendChild(ui.trigger);
                de.appendChild(ui.container);
                de.appendChild(ui.grips);
            } catch (err) {
                try { console.error('[Falco] не удалось создать панель:', err); } catch (e2) {}
            }
        }
        // Если сайт пересоздал <html> и панель выпала из документа - возвращаем её
        function keepUI() {
            if (!ui || !document.documentElement) return;
            if (!ui.container.isConnected) document.documentElement.appendChild(ui.container);
            if (!ui.trigger.isConnected) document.documentElement.appendChild(ui.trigger);
            if (!ui.grips.isConnected) document.documentElement.appendChild(ui.grips);
        }
        document.addEventListener('DOMContentLoaded', keepUI);
        window.addEventListener('load', keepUI);
        mountUI();
    } catch (e) {
        try { console.error('[Falco] browser.js:', e); } catch (e2) {}
    }
})();