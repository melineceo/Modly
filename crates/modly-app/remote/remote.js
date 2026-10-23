/* Пульт Modly на телефоне.
   Ключ приходит в #части адреса (из QR-кода в Modly), хранится только на этом телефоне и
   отправляется в заголовке Authorization. Всё, что пришло от мода, выводится только как текст. */
(function () {
    'use strict';

    var KEY_NAME = 'modly-remote-key';
    var key = '';
    var m = /(?:^#|&)k=([A-Za-z0-9_-]{32})(?:&|$)/.exec(location.hash);
    if (m) {
        key = m[1];
        try { localStorage.setItem(KEY_NAME, key); } catch (e) { /* приватный режим */ }
        history.replaceState(null, '', location.pathname); // убрать ключ из адресной строки
    } else {
        try { key = localStorage.getItem(KEY_NAME) || ''; } catch (e) { key = ''; }
    }

    var $ = function (id) { return document.getElementById(id); };
    var lang = 'ru';
    var t = function (ru, en) { return lang === 'en' ? en : ru; };
    var state = null;
    var shape = '';          // структура панели: перерисовываем целиком, только если она изменилась
    var rows = {};           // id элемента → функция обновления значения
    var busy = {};           // элемент, который человек сейчас трогает: его значение не перезаписываем
    var failures = 0;

    function el(tag, cls, text) {
        var n = document.createElement(tag);
        if (cls) { n.className = cls; }
        if (text !== undefined && text !== null) { n.textContent = String(text); }
        return n;
    }

    function send(cmd) {
        return fetch('/api/command', {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json' },
            body: JSON.stringify(cmd),
            cache: 'no-store'
        }).then(function () { setTimeout(poll, 120); }).catch(function () {});
    }

    function fmt(v, step) {
        // шаг приходит как float (0.1 → 0.10000000149…): знаков после запятой не больше 4, лишние нули убираем;
        // без шага (плавный ползунок) — два знака
        var d = 2;
        if (step) {
            var s = String(step);
            d = s.indexOf('.') >= 0 ? Math.min(4, s.split('.')[1].length) : 0;
        }
        var out = Number(v).toFixed(d);
        return out.indexOf('.') >= 0 ? out.replace(/0+$/, '').replace(/\.$/, '') : out;
    }

    // ------------------------------------------------------------------ элементы

    function head(c, right) {
        var h = el('div', 'head');
        var tx = el('div', 'text');
        tx.appendChild(el('b', null, c.label));
        if (c.description) { tx.appendChild(el('small', null, c.description)); }
        h.appendChild(tx);
        if (right) { h.appendChild(right); }
        return h;
    }

    function rowToggle(c, row) {
        var sw = el('button', 'switch');
        sw.type = 'button';
        sw.setAttribute('role', 'switch');
        sw.setAttribute('aria-label', c.label);
        row.appendChild(head(c, sw));
        sw.addEventListener('click', function () {
            var on = !sw.classList.contains('on');
            sw.classList.toggle('on', on);
            sw.setAttribute('aria-checked', on ? 'true' : 'false');
            send({ op: 'set', id: c.id, value: on });
        });
        return function (c2) {
            sw.classList.toggle('on', !!c2.checked);
            sw.setAttribute('aria-checked', c2.checked ? 'true' : 'false');
        };
    }

    function rowButton(c, row) {
        var b = el('button', 'btn ' + (c.tone && c.tone !== 'default' ? c.tone : ''), c.label);
        b.type = 'button';
        var armed = 0;
        b.addEventListener('click', function () {
            if (c.confirm && !armed) {
                armed = setTimeout(function () { armed = 0; b.classList.remove('armed'); b.textContent = c.label; }, 3000);
                b.classList.add('armed');
                b.textContent = c.confirm || t('Нажмите ещё раз', 'Tap again');
                return;
            }
            clearTimeout(armed);
            armed = 0;
            b.classList.remove('armed');
            b.textContent = c.label;
            send({ op: 'invoke', id: c.id });
        });
        row.appendChild(b);
        var note = el('div', 'result');
        row.appendChild(note);
        return function (c2) {
            // ответ мода на нажатие («Убито врагов: 12») или подсказка под кнопкой
            var text = c2.text || c2.description || '';
            note.textContent = text;
            note.hidden = !text;
        };
    }

    function rowSlider(c, row) {
        var val = el('span', 'value');
        row.appendChild(head(c, val));
        var r = el('input');
        r.type = 'range';
        r.min = c.min; r.max = c.max; r.step = c.step || 'any';
        r.setAttribute('aria-label', c.label);
        row.appendChild(r);
        var ends = el('div', 'range-ends');
        ends.appendChild(el('span', null, fmt(c.min, c.step)));
        ends.appendChild(el('span', null, fmt(c.max, c.step)));
        row.appendChild(ends);
        var last = 0;
        r.addEventListener('input', function () {
            busy[c.id] = Date.now();
            val.textContent = fmt(r.value, c.step) + (c.unit ? ' ' + c.unit : '');
            var now = Date.now();
            if (now - last > 120) { last = now; send({ op: 'number', id: c.id, value: Number(r.value) }); }
        });
        r.addEventListener('change', function () {
            busy[c.id] = Date.now();
            send({ op: 'number', id: c.id, value: Number(r.value) });
        });
        return function (c2) {
            if (busy[c.id] && Date.now() - busy[c.id] < 1500) { return; }
            r.value = c2.value;
            val.textContent = fmt(c2.value, c2.step) + (c2.unit ? ' ' + c2.unit : '');
        };
    }

    function rowNumber(c, row) {
        row.appendChild(head(c, null));
        var st = el('div', 'stepper');
        var minus = el('button', null, '−');
        var plus = el('button', null, '+');
        var inp = el('input');
        inp.type = 'number';
        inp.inputMode = 'decimal';
        inp.setAttribute('aria-label', c.label);
        minus.type = plus.type = 'button';
        st.appendChild(minus); st.appendChild(inp); st.appendChild(plus);
        row.appendChild(st);
        var step = Number(c.step) || 1;
        var clamp = function (v) { return Math.min(Number(c.max), Math.max(Number(c.min), v)); };
        var push = function (v) { busy[c.id] = Date.now(); inp.value = fmt(v, step); send({ op: 'number', id: c.id, value: v }); };
        minus.addEventListener('click', function () { push(clamp(Number(inp.value) - step)); });
        plus.addEventListener('click', function () { push(clamp(Number(inp.value) + step)); });
        inp.addEventListener('focus', function () { busy[c.id] = Date.now() + 600000; });
        inp.addEventListener('change', function () { if (inp.value !== '') { push(clamp(Number(inp.value))); } });
        inp.addEventListener('blur', function () { busy[c.id] = Date.now(); });
        if (c.presets && c.presets.length) {
            var pr = el('div', 'presets');
            c.presets.forEach(function (p) {
                var b = el('button', null, p.label);
                b.type = 'button';
                b.addEventListener('click', function () { push(Number(p.value)); });
                pr.appendChild(b);
            });
            row.appendChild(pr);
        }
        return function (c2) {
            if (busy[c.id] && Date.now() - busy[c.id] < 1500) { return; }
            inp.value = fmt(c2.value, step);
        };
    }

    function rowText(c, row) {
        row.appendChild(head(c, null));
        var line = el('div', 'textline');
        var inp = el('input');
        inp.type = 'text';
        inp.maxLength = 2000;
        inp.placeholder = c.placeholder || '';
        inp.setAttribute('aria-label', c.label);
        var ok = el('button', null, 'OK');
        ok.type = 'button';
        line.appendChild(inp); line.appendChild(ok);
        row.appendChild(line);
        var go = function () { busy[c.id] = Date.now(); send({ op: 'set', id: c.id, value: inp.value }); inp.blur(); };
        ok.addEventListener('click', go);
        inp.addEventListener('keydown', function (e) { if (e.key === 'Enter') { go(); } });
        inp.addEventListener('focus', function () { busy[c.id] = Date.now() + 600000; });
        inp.addEventListener('blur', function () { busy[c.id] = Date.now(); });
        return function (c2) {
            if (busy[c.id] && Date.now() - busy[c.id] < 1500) { return; }
            inp.value = c2.text;
        };
    }

    function rowOptions(c, row) {
        row.appendChild(head(c, null));
        var box = el('div', 'options');
        var buttons = {};
        (c.options || []).forEach(function (o) {
            var b = el('button', null, o.label);
            b.type = 'button';
            b.addEventListener('click', function () { send({ op: 'pick', id: c.id, option: o.value }); });
            buttons[o.value] = b;
            box.appendChild(b);
        });
        row.appendChild(box);
        return function (c2) {
            (c2.options || []).forEach(function (o) {
                if (buttons[o.value]) { buttons[o.value].classList.toggle('on', !!o.selected); }
            });
        };
    }

    function rowStat(c, row) {
        var val = el('span', 'value');
        row.appendChild(head(c, val));
        return function (c2) { val.textContent = c2.text + (c2.unit ? ' ' + c2.unit : ''); };
    }

    function rowProgress(c, row) {
        var val = el('span', 'value');
        row.appendChild(head(c, val));
        var bar = el('div', 'progress');
        var fill = el('div');
        bar.appendChild(fill);
        row.appendChild(bar);
        return function (c2) {
            var span = Number(c2.max) - Number(c2.min) || 1;
            var p = Math.max(0, Math.min(1, (Number(c2.value) - Number(c2.min)) / span));
            fill.style.width = (p * 100).toFixed(1) + '%';
            val.textContent = c2.text || Math.round(p * 100) + '%';
        };
    }

    function rowNote(c, row) {
        var n = el('div', 'note ' + (c.tone || ''), c.label);
        row.appendChild(n);
        return function (c2) { n.textContent = c2.label; };
    }

    var builders = {
        toggle: rowToggle, button: rowButton, slider: rowSlider, number: rowNumber, text: rowText,
        segmented: rowOptions, chips: rowOptions, stat: rowStat, progress: rowProgress, note: rowNote
    };

    // ------------------------------------------------------------------ отрисовка

    function shapeOf(s) {
        return JSON.stringify([s.page, s.pages.map(function (p) { return p.title; }), s.sections.map(function (sec) {
            return [sec.title, sec.controls.map(function (c) { return [c.id, c.kind, c.label, (c.options || []).map(function (o) { return o.value + o.label; })]; })];
        })]);
    }

    function render(s) {
        var main = $('main');
        rows = {};
        while (main.firstChild) { main.removeChild(main.firstChild); }
        var pages = $('pages');
        while (pages.firstChild) { pages.removeChild(pages.firstChild); }
        if (s.pages.length > 1) {
            s.pages.forEach(function (p, i) {
                var b = el('button', i === s.page ? 'on' : '', p.title);
                b.type = 'button';
                b.addEventListener('click', function () { send({ op: 'page', index: i }); });
                pages.appendChild(b);
            });
        }
        s.sections.forEach(function (sec) {
            var box = el('section', 'section');
            if (sec.title) { box.appendChild(el('h2', null, sec.title)); }
            if (sec.description) { box.appendChild(el('p', null, sec.description)); }
            sec.controls.forEach(function (c) {
                if (c.kind === 'separator') {
                    box.appendChild(el('div', 'sep'));
                    return;
                }
                var make = builders[c.kind];
                if (!make) { return; }
                var row = el('div', 'row');
                rows[c.id] = { update: make(c, row), row: row };
                box.appendChild(row);
            });
            main.appendChild(box);
        });
    }

    function update(s) {
        s.sections.forEach(function (sec) {
            sec.controls.forEach(function (c) {
                var r = rows[c.id];
                if (r) {
                    r.update(c);
                    r.row.classList.toggle('disabled', !c.enabled);
                }
            });
        });
    }

    function showEmpty(title, text) {
        var main = $('main');
        rows = {};
        shape = '';
        while (main.firstChild) { main.removeChild(main.firstChild); }
        var box = el('div', 'empty');
        box.appendChild(el('div', 'empty-icon', '⚡'));
        box.appendChild(el('b', null, title));
        if (text) { box.appendChild(el('p', null, text)); }
        main.appendChild(box);
        var pages = $('pages');
        while (pages.firstChild) { pages.removeChild(pages.firstChild); }
    }

    function apply(s) {
        state = s;
        lang = s.lang === 'en' ? 'en' : 'ru';
        document.documentElement.lang = lang;
        $('dot').classList.toggle('on', !!s.connected);
        $('title').textContent = s.connected ? s.title : 'Modly';
        $('subtitle').textContent = s.connected ? s.subtitle : t('Пульт', 'Remote');
        var banner = $('banner');
        var msg = s.paused ? t('Игра на паузе: команды выполнятся, когда она снова станет активной.', 'The game is paused: commands apply when it becomes active again.') : s.status;
        banner.hidden = !s.connected || !msg;
        banner.className = 'banner ' + (s.paused ? 'warning' : s.status_tone || '');
        banner.textContent = msg || '';
        if (!s.connected) {
            showEmpty(t('Мод не подключён', 'No mod connected'), t('Запустите игру с модом — его переключатели появятся здесь. Пульт показывает тот мод, что открыт в Modly.', 'Start a game with a mod — its controls show up here. The remote shows the mod open in Modly.'));
            return;
        }
        var sh = shapeOf(s);
        if (sh !== shape) {
            shape = sh;
            render(s);
        }
        update(s);
    }

    // без ключа опрашивать нечего: остаётся подсказка, пока человек не отсканирует QR-код заново
    function stop(title, text) {
        key = '';
        state = null;
        $('dot').classList.remove('on');
        $('banner').hidden = true;
        $('title').textContent = 'Modly';
        $('subtitle').textContent = t('Пульт', 'Remote');
        showEmpty(title, text);
    }

    function poll() {
        if (!key) { return; }
        fetch('/api/state', { headers: { 'Authorization': 'Bearer ' + key }, cache: 'no-store' }).then(function (r) {
            if (r.status === 401) {
                try { localStorage.removeItem(KEY_NAME); } catch (e) { /* нет */ }
                stop(t('Ключ больше не действует', 'The key no longer works'), t('В Modly сбросили доступ. Отсканируйте QR-код заново.', 'Access was reset in Modly. Scan the QR code again.'));
                return null;
            }
            return r.ok ? r.json() : null;
        }).then(function (s) {
            if (s) { failures = 0; apply(s); }
        }).catch(function () {
            failures++;
            if (failures === 3) {
                $('dot').classList.remove('on');
                showEmpty(t('Нет связи с Modly', 'Can’t reach Modly'), t('Телефон и компьютер должны быть в одной сети Wi-Fi, а пульт — включён в Modly.', 'The phone and the PC must be on the same Wi-Fi, and the remote must be on in Modly.'));
            }
        });
    }

    // новая ссылка из QR-кода в той же вкладке меняет только #часть — страница сама не перезагружается
    window.addEventListener('hashchange', function () {
        if (/(?:^#|&)k=/.test(location.hash)) { location.reload(); }
    });

    if (key) {
        poll();
    } else {
        stop(t('Нужен ключ', 'A key is needed'), t('Откройте в Modly «Настройки → Пульт с телефона» и отсканируйте QR-код.', 'Open Settings → Phone remote in Modly and scan the QR code.'));
    }
    setInterval(function () {
        if (document.visibilityState === 'visible') { poll(); }
    }, 700);
    document.addEventListener('visibilitychange', function () {
        if (document.visibilityState === 'visible') { poll(); }
    });
})();
