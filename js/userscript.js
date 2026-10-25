(function () {
    try {
        var M = __M__, I = __I__, E = __E__, EX = __EX__, RT = "__RT__", NF = __NF__;
        if (NF && window !== window.top) return;
        var esc = function (s) { return s.replace(/[.+?^${}()|[\]\\]/g, '\\$&'); };
        var g2r = function (s) {
            if (s.length > 2 && s.charAt(0) === '/' && s.charAt(s.length - 1) === '/') return new RegExp(s.slice(1, -1), 'i');
            return new RegExp('^' + esc(s).replace(/\*/g, '.*') + '$', 'i');
        };
        var m2r = function (p) {
            if (p === '<all_urls>') return /^(https?|file):\/\//i;
            var m = /^(\*|https?|file):\/\/([^\/]*)(\/.*)?$/.exec(p);
            if (!m) return g2r(p);
            var h = m[2], hr;
            if (h === '*') hr = '[^/]+';
            else if (h.indexOf('*.') === 0) hr = '(?:[^/]+\\.)?' + esc(h.slice(2));
            else hr = esc(h).replace(/\*/g, '[^/]*');
            return new RegExp('^' + (m[1] === '*' ? 'https?' : m[1]) + ':\\/\\/' + hr + esc(m[3] || '/').replace(/\*/g, '.*') + '$', 'i');
        };
        var any = function (list) {
            for (var i = 0; i < list.length; i++) if (list[i].test(location.href)) return true;
            return false;
        };
        var inc = M.map(m2r).concat(I.map(g2r)), exc = EX.map(m2r).concat(E.map(g2r));
        if (!inc.length || !any(inc) || any(exc)) return;

        var NAME = __NAMEJSON__, P = 'ulgm:' + NAME + ':';
        var GM_getValue = function (k, d) { try { var v = localStorage.getItem(P + k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } };
        var GM_setValue = function (k, v) { try { localStorage.setItem(P + k, JSON.stringify(v)); } catch (e) {} };
        var GM_deleteValue = function (k) { try { localStorage.removeItem(P + k); } catch (e) {} };
        var GM_addStyle = function (css) {
            var s = document.createElement('style'); s.textContent = css;
            (document.head || document.documentElement).appendChild(s); return s;
        };
        var GM_log = function () { console.log.apply(console, arguments); };
        var GM_registerMenuCommand = function () {};
        var GM_setClipboard = function (t) { try { navigator.clipboard.writeText(String(t)); } catch (e) {} };
        var GM_info = { script: { name: NAME }, scriptHandler: 'Falco' };
        var GM = {
            getValue: function (k, d) { return Promise.resolve(GM_getValue(k, d)); },
            setValue: function (k, v) { GM_setValue(k, v); return Promise.resolve(); },
            deleteValue: function (k) { GM_deleteValue(k); return Promise.resolve(); },
            addStyle: function (c) { return Promise.resolve(GM_addStyle(c)); },
            log: GM_log, info: GM_info
        };

        var run = function () {
            try {
                (function (unsafeWindow, GM_getValue, GM_setValue, GM_deleteValue, GM_addStyle, GM_log, GM_registerMenuCommand, GM_setClipboard, GM_info, GM) {
__CODE__
                }).call(window, window, GM_getValue, GM_setValue, GM_deleteValue, GM_addStyle, GM_log, GM_registerMenuCommand, GM_setClipboard, GM_info, GM);
            } catch (e) { console.error('[Falco userscript] ' + NAME, e); }
        };
        var go = RT === 'document-idle' ? function () { setTimeout(run, 0); } : run;
        if (RT === 'document-start') run();
        else if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', go, { once: true });
        else go();
    } catch (e) {}
})();